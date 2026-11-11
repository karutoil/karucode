import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { ChatView, projectMentionLoader } from './ChatView';
import type { ChatItem } from '../state/chat';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as terminalState from '../state/terminals';
import * as workspaceTabs from '../state/workspaceTabs';
import { configFixture, resetAppState } from '../test/fixtures';

/* ------------------------------------------------------------------ *
 * Layout stubs. jsdom has no layout engine, so the three metrics the
 * sticky-bottom hook reads are faked on one element plus a ResizeObserver
 * we can fire by hand.
 * ------------------------------------------------------------------ */

interface Box {
  scrollHeight: number;
  clientHeight: number;
}

const VIEWPORT = 400;

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  readonly targets = new Set<Element>();
  private readonly callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    ResizeObserverStub.instances.push(this);
  }

  observe(target: Element): void {
    this.targets.add(target);
  }

  unobserve(target: Element): void {
    this.targets.delete(target);
  }

  disconnect(): void {
    this.targets.clear();
  }

  static reset(): void {
    for (const instance of ResizeObserverStub.instances) instance.disconnect();
    ResizeObserverStub.instances = [];
  }

  /** Pretends every observed box changed size (content grew or reflowed). */
  static fire(): void {
    for (const instance of ResizeObserverStub.instances) {
      instance.callback([], instance as unknown as ResizeObserver);
    }
  }
}

/**
 * Gives `element` real scroll behaviour: `scrollTop` clamps to the maximum,
 * exactly like the browser, so tests can assert the view really reached the end.
 */
function fakeScrollBox(element: HTMLElement, box: Box): void {
  let top = 0;
  const max = () => Math.max(0, box.scrollHeight - box.clientHeight);
  Object.defineProperty(element, 'scrollHeight', {
    configurable: true,
    get: () => box.scrollHeight,
  });
  Object.defineProperty(element, 'clientHeight', {
    configurable: true,
    get: () => box.clientHeight,
  });
  Object.defineProperty(element, 'scrollTop', {
    configurable: true,
    get: () => top,
    set: (value: number) => {
      top = Math.max(0, Math.min(value, max()));
    },
  });
}

function scrollElement(container: ParentNode): HTMLElement {
  const element = container.querySelector<HTMLElement>('.chat-scroll');
  if (!element) throw new Error('.chat-scroll not rendered');
  return element;
}

/** Puts the view at the bottom and reports the scroll so the hook observes it. */
async function scrollTo(element: HTMLElement, top: number): Promise<void> {
  element.scrollTop = top;
  fireEvent.scroll(element);
  // The listener is rAF-throttled: give the frame a chance to run.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 32));
  });
}

/* ------------------------------------------------------------------ *
 * Component harness
 * ------------------------------------------------------------------ */

function messageItem(id: string, role: 'user' | 'assistant', content: string): ChatItem {
  return {
    kind: 'message',
    id,
    at: 1,
    role,
    content,
    reasoning: '',
    streaming: false,
    runId: 'r_x',
    agentId: null,
  };
}

const baseItems: ChatItem[] = [
  messageItem('m1', 'user', 'Find the login bug.'),
  messageItem('m2', 'assistant', 'The login handler is unfinished.'),
];

function renderChat(items: ChatItem[] = baseItems, sessionId = 's_ab12') {
  store.currentSessionId.value = sessionId;
  store.config.value = configFixture;
  store.health.value = {
    status: 'ok',
    version: '0.1.0',
    uptime_secs: 12,
    workspace: '/home/me/project',
    projects: [],
    auth_required: false,
    provider_configured: true,
    db: 'ok',
  };
  store.sessions.value = [
    {
      id: sessionId,
      title: 'Fix login bug',
      workspace: '/home/me/project',
      project_id: null,
      created_at: 1,
      updated_at: 2,
      message_count: items.length,
      last_run_status: 'completed',
      running: false,
    },
  ];
  store.chat.value = {
    ...store.chat.value,
    sessionId,
    items,
    running: false,
  };
  const result = render(<ChatView />);
  const element = scrollElement(result.container);
  return { ...result, element };
}

/** Simulates late reflow: content grows without a new text delta, then layout observers fire. */
async function growContent(element: HTMLElement, box: Box, delta: number): Promise<void> {
  box.scrollHeight += delta;
  await act(async () => {
    ResizeObserverStub.fire();
    await new Promise((resolve) => setTimeout(resolve, 32));
  });
  expect(element.scrollHeight).toBe(box.scrollHeight);
}

function jumpButton(): HTMLElement | null {
  return screen.queryByRole('button', { name: /Jump to latest/ });
}

describe('ChatView sticky bottom', () => {
  beforeEach(() => {
    resetAppState();
    ResizeObserverStub.reset();
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
  });

  afterEach(() => {
    resetAppState();
    ResizeObserverStub.reset();
    vi.unstubAllGlobals();
  });

  it('keeps the view at the bottom when late content reflow grows the transcript', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1000, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    // Pinned at the bottom, nothing to jump to.
    await scrollTo(element, box.scrollHeight - box.clientHeight);
    expect(jumpButton()).toBeNull();

    // Markdown finishes rendering / a tool card expands: no text delta at all.
    await growContent(element, box, 480);

    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    expect(jumpButton()).toBeNull();
  });

  it('never fights a reader who scrolled up, and offers "Jump to latest"', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1000, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, 120);
    expect(await screen.findByRole('button', { name: /Jump to latest/ })).not.toBeNull();

    await growContent(element, box, 600);

    expect(element.scrollTop).toBe(120);
    expect(jumpButton()).not.toBeNull();
  });

  // The reported bug. A browser frame runs scroll events first, then rAF, then
  // layout observers. So when one big tool card is followed and the next is
  // committed before that frame's measurement, the scroll event caused by the
  // follow measures a fresh scrollHeight against the scrollTop the follow left
  // behind: a gap the size of the second card. Reading that gap as "the reader
  // scrolled away" is what stopped auto-following for good.
  it('keeps following when a big card lands between a follow and its measurement', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1000, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, box.scrollHeight - box.clientHeight);
    expect(jumpButton()).toBeNull();

    // Card one mounts; the observer follows it and writes scrollTop.
    await growContent(element, box, 600);
    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);

    // Card two is committed before the browser can measure that follow, so the
    // height grows with no further observer callback.
    box.scrollHeight += 640;
    // The follow's own scroll event finally dispatches and its handler measures.
    fireEvent.scroll(element);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 32));
    });

    // It caught up with the card rather than giving up on the run.
    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    expect(jumpButton()).toBeNull();

    // And following still works for everything after it.
    await growContent(element, box, 500);
    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    expect(jumpButton()).toBeNull();
  });

  it('gives up following when the reader scrolls up after a catch-up', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1000, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, box.scrollHeight - box.clientHeight);
    // A big card arrives and is followed.
    await growContent(element, box, 600);
    // Then the reader scrolls up by hand while more output streams in.
    await scrollTo(element, 200);

    await growContent(element, box, 900);

    expect(element.scrollTop).toBe(200);
    expect(jumpButton()).not.toBeNull();
  });

  it('scrolls to the bottom and hides the button when "Jump to latest" is clicked', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1200, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, 100);
    const button = await screen.findByRole('button', { name: /Jump to latest/ });
    fireEvent.click(button);

    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    await waitFor(() => expect(jumpButton()).toBeNull());
    // Focus stays in the transcript instead of falling back to the document.
    expect(document.activeElement).toBe(element);
  });

  it('re-pins when the user sends a message', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1600, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, 200);
    expect(await screen.findByRole('button', { name: /Jump to latest/ })).not.toBeNull();

    // What `actions.sendChatMessage` does after appending the optimistic message.
    await act(async () => {
      store.sentMessageTick.value += 1;
      store.chat.value = {
        ...store.chat.value,
        items: [...store.chat.value.items, messageItem('m3', 'user', 'Show me the diff')],
      };
    });

    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    await waitFor(() => expect(jumpButton()).toBeNull());
  });

  it('jumps to the bottom instantly when switching sessions', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 2000, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, 300);
    expect(await screen.findByRole('button', { name: /Jump to latest/ })).not.toBeNull();

    const scrollToSpy = vi.fn();
    Object.defineProperty(element, 'scrollTo', { configurable: true, value: scrollToSpy });

    await act(async () => {
      store.sessions.value = [
        {
          id: 's_cd34',
          title: 'Add workspace tree',
          workspace: '/home/me/project',
          project_id: null,
          created_at: 1,
          updated_at: 2,
          message_count: 1,
          last_run_status: null,
          running: false,
        },
      ];
      store.currentSessionId.value = 's_cd34';
      store.chat.value = {
        ...store.chat.value,
        sessionId: 's_cd34',
        items: [messageItem('s2m1', 'user', 'Add a file tree')],
      };
    });

    // Synchronous, not animated: the view is already at the bottom in this commit.
    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    expect(scrollToSpy).not.toHaveBeenCalled();
    await waitFor(() => expect(jumpButton()).toBeNull());
  });

  it('jumps without animation when the reader prefers reduced motion', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1400, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    const scrollToSpy = vi.fn((options: ScrollToOptions) => {
      element.scrollTop = options.top ?? 0;
    });
    Object.defineProperty(element, 'scrollTo', { configurable: true, value: scrollToSpy });

    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query.includes('prefers-reduced-motion'),
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }));

    await scrollTo(element, 120);
    fireEvent.click(await screen.findByRole('button', { name: /Jump to latest/ }));

    expect(element.scrollTop).toBe(box.scrollHeight - box.clientHeight);
    expect(scrollToSpy).not.toHaveBeenCalled();
  });
});
describe('ChatView workspace tabs', () => {
  beforeEach(() => {
    resetAppState();
    terminalState.resetTerminalState();
    ResizeObserverStub.reset();
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
  });

  afterEach(() => {
    resetAppState();
    terminalState.resetTerminalState();
    ResizeObserverStub.reset();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function tab(name: RegExp): HTMLElement {
    const found = screen.getByRole('tab', { name });
    return found;
  }

  it('places views in the one main header and keeps the other panes unloaded', () => {
    const { container } = renderChat();
    const header = container.querySelector('header.main-head');
    expect(header?.querySelector('[role="tablist"][aria-label="Workspace views"]')).not.toBeNull();
    expect(header?.querySelector('.header-buttons')).not.toBeNull();
    expect(container.querySelector('.workspace-tabbar')).toBeNull();

    expect(tab(/^Conversation/).getAttribute('aria-selected')).toBe('true');
    expect(tab(/^Source control/).getAttribute('aria-selected')).toBe('false');
    expect(document.getElementById('workspace-panel-chat')?.hidden).toBe(false);
    // Lazy: a git probe only runs once the user asks for the source-control view.
    expect(document.getElementById('workspace-panel-changes')).toBeNull();
    expect(screen.getByLabelText('Message karucode')).not.toBeNull();
  });

  it('offers no terminal view: there is no shell backend to drive one', () => {
    renderChat();

    expect(screen.queryByRole('tab', { name: /terminal/i })).toBeNull();
    expect(document.getElementById('workspace-panel-terminal')).toBeNull();
    // Command output lives in the transcript itself.
    expect(
      within(screen.getByRole('tablist', { name: 'Workspace views' }))
        .getAllByRole('tab')
        .map((element) => element.textContent?.trim()),
    ).toEqual([
      'Conversation',
      'Source control',
    ]);
  });

  it('mounts the source-control manager on first visit and keeps it afterwards', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(JSON.stringify({ error: { code: 'not_found', message: 'no git' } }), { status: 404 })),
    );
    renderChat();
    fireEvent.click(tab(/^Source control/));

    const panel = document.getElementById('workspace-panel-changes');
    expect(panel).not.toBeNull();
    expect(panel?.hidden).toBe(false);
    // The page variant is the full manager: header, commit box and diff region.
    expect(panel?.querySelector('.gitm-page')).not.toBeNull();
    expect(panel?.querySelector('.gitm-kicker')?.textContent).toBe('Source control');
    expect(panel?.querySelector('.gitm-title')?.textContent).toBe('Working tree');
    // A failed probe is reported with a retry, not an empty tree.
    await waitFor(() => expect(panel?.querySelector('.inline-error')).not.toBeNull());

    fireEvent.click(tab(/^Conversation/));
    expect(document.getElementById('workspace-panel-changes')?.hidden).toBe(true);
    expect(document.getElementById('workspace-tab-changes')).not.toBeNull();
  });

  it('moves between views with the arrow keys', () => {
    renderChat();
    const conversation = tab(/^Conversation/);
    conversation.focus();
    fireEvent.keyDown(conversation, { key: 'ArrowRight' });
    expect(tab(/^Source control/).getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(tab(/^Source control/));

    // Two tabs: the ends are Conversation and Source control, and it wraps.
    fireEvent.keyDown(tab(/^Source control/), { key: 'ArrowRight' });
    expect(tab(/^Conversation/).getAttribute('aria-selected')).toBe('true');

    fireEvent.keyDown(tab(/^Conversation/), { key: 'End' });
    expect(tab(/^Source control/).getAttribute('aria-selected')).toBe('true');

    fireEvent.keyDown(tab(/^Source control/), { key: 'Home' });
    expect(tab(/^Conversation/).getAttribute('aria-selected')).toBe('true');
  });

  it('copies the conversation as Markdown from the header action', async () => {
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    renderChat();

    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy conversation' }));

    await vi.waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const copied = writeText.mock.calls[0]?.[0] ?? '';
    expect(copied).toContain('Fix login bug');
    expect(copied).toContain('Find the login bug.');
    expect(copied).toContain('The login handler is unfinished.');
    await vi.waitFor(() => expect(store.toasts.value.length).toBeGreaterThan(0));
    expect(store.toasts.value[0]?.message).toContain('copied');
  });

  it('disables the copy action when the session has no transcript', () => {
    renderChat([]);
    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    const copy = screen.getByRole('menuitem', { name: 'Copy conversation' }) as HTMLButtonElement;
    expect(copy.disabled).toBe(true);
  });

  it('mounts the shared mock empty state for a session with no messages', () => {
    const { container } = renderChat([]);

    // One component serves both the empty and the no-session case; here it is
    // mounted with the `empty` variant, whose prompt behaviour (create-then-fill
    // for `no-session`) is covered by EmptyState.test.tsx.
    expect(container.querySelector('.empty-state')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'What are we building?' })).not.toBeNull();
    expect(screen.getByText(/Start with an idea or a task/)).not.toBeNull();
    expect(container.querySelectorAll('.quick-prompt')).toHaveLength(3);
  });

  it('treats an unsent draft as the open conversation', () => {
    // "New session" writes nothing until the first message, so the draft has no
    // session id. It must still render as the open conversation - the header names
    // it, the starter prompts fill the box - and not as "nothing is open".
    store.currentSessionId.value = null;
    store.config.value = configFixture;
    store.health.value = {
      status: 'ok',
      version: '0.1.0',
      uptime_secs: 12,
      workspace: '/home/me/project',
      projects: [],
      auth_required: false,
      provider_configured: true,
      db: 'ok',
    };
    actions.startDraftSession();

    const { container } = render(<ChatView />);
    expect(container.querySelector('.empty-state')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'New session' })).not.toBeNull();

    const box = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    expect(box.placeholder).toBe('Ask karucode anything, or describe a change...');

    fireEvent.click(screen.getByRole('button', { name: /Explore the codebase/ }));
    expect(box.value).toBe('Explore this codebase and suggest the highest-impact improvements.');
  });

  it('renders the project / session breadcrumb and the activity chip', () => {
    store.projects.value = [
      {
        id: 'p_karucode',
        name: 'KaruCode',
        path: '/home/me/project',
        created_at: 1,
        updated_at: 1,
        session_count: 1,
      },
    ];
    store.currentProjectId.value = 'p_karucode';
    renderChat();

    // One line, as in the mock: the project names the workspace, the session
    // title names the conversation, and the activity word rides beside it as a
    // capsule. The `sessions` segment the old two-row header carried is gone -
    // it repeated what the session-list toggle in the same bar already means.
    expect(screen.getByText('KaruCode')).not.toBeNull();
    expect(screen.getByText('ready')).not.toBeNull();
    expect(screen.getByRole('heading', { level: 1, name: 'Fix login bug' })).not.toBeNull();
  });

  it('hydrates the server-backed project width at the real result separator', () => {
    store.projects.value = [
      {
        id: 'p_karucode',
        name: 'KaruCode',
        path: '/home/me/project',
        created_at: 1,
        updated_at: 1,
        session_count: 1,
        result_panel_width: 640,
      },
    ];
    store.currentProjectId.value = 'p_karucode';
    const { container } = renderChat();

    const separator = screen.getByRole('separator', { name: 'Resize results panel' });
    expect(separator.getAttribute('aria-valuenow')).toBe('640');
    expect(separator.getAttribute('aria-controls')).toBe('workbench-results');
    expect(container.querySelector<HTMLElement>('.center-body')?.style.getPropertyValue('--workbench-result-w')).toBe('640px');
    expect(document.getElementById('workbench-results')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Collapse inspector' }));
    expect(screen.queryByRole('separator', { name: 'Resize results panel' })).toBeNull();
    expect(container.querySelector<HTMLElement>('.center-body')?.style.getPropertyValue('--workbench-result-w')).toBe('');
  });

  it('pops the results panel out on demand instead of splitting the conversation', () => {
    // The Appearance preference: Results is not a column at all. The conversation
    // keeps the whole pane, and the panel arrives as an overlay when asked for.
    store.inspectorMode.value = 'popout';
    store.inspectorCollapsed.value = true;
    const { container } = renderChat();

    const body = container.querySelector<HTMLDivElement>('.center-body');
    expect(body?.classList.contains('has-result')).toBe(false);
    expect(screen.queryByRole('separator', { name: 'Resize results panel' })).toBeNull();
    const header = within(document.querySelector('.header-buttons') as HTMLElement);
    const open = header.getByRole('button', { name: 'Open results' });
    expect(open.getAttribute('aria-expanded')).toBe('false');
    expect(open.getAttribute('aria-controls')).toBe('workbench-results');

    fireEvent.click(open);

    // Open is an overlay: still no split, but the card and its drag handle exist.
    expect(store.inspectorCollapsed.value).toBe(false);
    expect(body?.classList.contains('has-result')).toBe(false);
    const panel = document.getElementById('workbench-results');
    expect(panel?.getAttribute('data-results-popout')).toBe('true');
    const separator = screen.getByRole('separator', { name: 'Resize results panel' });
    expect(separator.classList.contains('result-resizer-popout')).toBe(true);
    expect(separator.getAttribute('aria-controls')).toBe('workbench-results');
    // The card needs a concrete width for its own geometry and the handle's.
    expect(body?.style.getPropertyValue('--workbench-result-w')).toMatch(/^\d+px$/);

    fireEvent.click(header.getByRole('button', { name: 'Close results' }));
    expect(store.inspectorCollapsed.value).toBe(true);
    expect(screen.queryByRole('separator', { name: 'Resize results panel' })).toBeNull();
    expect(body?.style.getPropertyValue('--workbench-result-w')).toBe('');
  });

  it('gives the popped-out panel the docked panel’s saved width and keyboard resize', () => {
    store.projects.value = [
      {
        id: 'p_karucode',
        name: 'KaruCode',
        path: '/home/me/project',
        created_at: 1,
        updated_at: 1,
        session_count: 1,
        result_panel_width: 640,
      },
    ];
    store.currentProjectId.value = 'p_karucode';
    store.inspectorMode.value = 'popout';
    store.inspectorCollapsed.value = false;
    const { container } = renderChat();

    const separator = screen.getByRole('separator', { name: 'Resize results panel' });
    expect(separator.classList.contains('result-resizer-popout')).toBe(true);
    expect(separator.getAttribute('aria-valuenow')).toBe('640');
    expect(container.querySelector<HTMLElement>('.center-body')?.style.getPropertyValue('--workbench-result-w')).toBe('640px');

    // Same gesture as the dock: Left grows the right-hand pane by 10, Shift by 50,
    // Home/End pick the bounds.
    fireEvent.keyDown(separator, { key: 'ArrowLeft' });
    expect(separator.getAttribute('aria-valuenow')).toBe('650');
    fireEvent.keyDown(separator, { key: 'ArrowLeft', shiftKey: true });
    expect(separator.getAttribute('aria-valuenow')).toBe('700');
    fireEvent.keyDown(separator, { key: 'Home' });
    expect(separator.getAttribute('aria-valuenow')).toBe('280');
  });

  it('returns to the conversation when results are opened from a full-width view', () => {
    store.inspectorMode.value = 'popout';
    store.inspectorCollapsed.value = true;
    renderChat();

    // Source control is a full-width view: the inspector is hidden behind it, so
    // the header's way into Results has to bring the panel's surface back first.
    fireEvent.click(screen.getByRole('tab', { name: /^Source control/ }));
    const header = within(document.querySelector('.header-buttons') as HTMLElement);
    fireEvent.click(header.getByRole('button', { name: 'Open results' }));

    expect(store.inspectorCollapsed.value).toBe(false);
    expect(screen.getByRole('tab', { name: /^Conversation/ }).getAttribute('aria-selected')).toBe('true');
    expect(document.getElementById('workbench-results')?.getAttribute('data-results-popout')).toBe('true');
    workspaceTabs.resetWorkspaceTabs();
  });

  it('keeps the docked split when the preference says docked', () => {
    // The default must not drift: same markup, same split, no pop-out handle.
    renderChat();
    expect(store.inspectorMode.value).toBe('docked');
    expect(document.querySelector('.center-body')?.classList.contains('has-result')).toBe(true);
    const separator = screen.getByRole('separator', { name: 'Resize results panel' });
    expect(separator.classList.contains('result-resizer-popout')).toBe(false);
    expect(within(document.querySelector('.header-buttons') as HTMLElement).queryByRole('button', { name: 'Open results' })).toBeNull();
  });

  it('draws the context reading as a ring with no label, in the session header', () => {
    store.chat.value = { ...store.chat.value, lastPromptTokens: 64_000 };
    renderChat();

    // Context lives in the header, not in the results panel's own header, so it is
    // still there when that panel is closed.
    fireEvent.click(screen.getByRole('button', { name: 'Collapse inspector' }));
    const button = screen.getByRole('button', { name: /Session context — 50\.0% of the window used/ });
    expect(within(button).queryByText('Context')).toBeNull();
    const fill = button.querySelector('.context-header-ring-fill');
    expect(fill).not.toBeNull();
    const circumference = 2 * Math.PI * 8;
    // Half the window is used, so half the ring is drawn.
    expect(Number(fill?.getAttribute('stroke-dashoffset'))).toBeCloseTo(circumference / 2, 3);
    expect(within(button).getByText('50.0%')).not.toBeNull();
    expect(button.getAttribute('data-tone')).toBe('calm');

    fireEvent.click(button);
    expect(document.getElementById('workbench-context')).not.toBeNull();
  });

  it('lets a plugin hide the session header’s Context button', () => {
    renderChat();
    act(() => {
      store.config.value = {
        ...configFixture,
        plugin_ui: {
          contributions: [],
          hidden: [{ target: 'inspector.context', plugins: ['zen'] }],
          slots: [],
          hideable: [],
        },
      };
    });

    // `inspector.context` names this button now; the results header has no
    // built-in control to lose, so the hide lands where the person can see it.
    expect(screen.queryByRole('button', { name: /Session context/ })).toBeNull();
    expect(screen.getByRole('button', { name: 'Notifications' })).not.toBeNull();
  });

  it('gives every workspace tab a 15px glyph and shows the changed-file count', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            is_repo: true,
            branch: 'main',
            error: null,
            files: [
              { path: 'src/a.ts', status: 'M', staged: false },
              { path: 'src/b.ts', status: 'A', staged: true },
            ],
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        ),
      ),
    );
    renderChat();

    for (const name of [/^Conversation/, /^Source control/]) {
      const icon = tab(name).querySelector('.tab-icon svg');
      expect(icon?.getAttribute('width')).toBe('15');
    }

    // The count pill is real data (git status), not the mock's demo `3`.
    await waitFor(() =>
      expect(tab(/^Source control/).querySelector('.tab-count')?.textContent).toBe('2'),
    );
  });

  it('keeps activity in the main header after removing the second strip', () => {
    renderChat();
    const activity = document.querySelector('.main-head .head-activity');
    expect(activity?.textContent).toBe('ready');
    expect(document.querySelector('.tab-note')).toBeNull();
    expect(document.querySelector('.main-head .header-buttons')).not.toBeNull();
  });

  it('retains terminal availability and the server-owned tab count in the header', () => {
    vi.spyOn(terminalState, 'loadTerminals').mockResolvedValue();
    terminalState.terminalsEnabled.value = true;
    terminalState.terminals.value = [{
      id: 't_one', title: 'shell', cwd: '/tmp/one', project: 'p_one', position: 0,
      created_at: 1, updated_at: 1, last_used_at: 1, running: true, pid: 123, attached: 0,
    }];
    renderChat();

    const terminal = tab(/^Terminal 1$/);
    expect(terminal.getAttribute('aria-selected')).toBe('false');
    expect(terminal.closest('.main-head')).not.toBeNull();
    expect(terminal.querySelector('.tab-count')?.textContent).toBe('1');
  });

  it('gives a long branch chip a label and a title that survive truncation', async () => {
    // The branch name comes from the repository and can be arbitrarily long; on
    // a phone the visible text is the icon alone, so the accessible name cannot
    // depend on it.
    const branch = 'feature/header-cascade-and-branch-truncation-on-a-phone';
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response(JSON.stringify({ is_repo: true, branch, error: null, files: [] }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    );
    renderChat();

    const chip = await screen.findByRole('button', {
      name: `Branch ${branch} — open Changes`,
    });
    expect(chip.classList.contains('branch-chip')).toBe(true);
    const name = chip.querySelector('.branch-chip-name');
    expect(name?.textContent).toBe(branch);
    expect(chip.getAttribute('title')).toBe(`On branch ${branch} — open Changes`);
    // The visible text is truncated by CSS; the title keeps the full value.
    expect(name?.getAttribute('title')).toBe(branch);
  });

  it('only offers the session-list button while the rail is collapsed', () => {
    renderChat();
    const toggle = (): HTMLElement => screen.getByRole('button', { name: 'Toggle session list' });
    // Rendered for narrow viewports and for desktop once `Ctrl+B` hides the rail;
    // CSS hides it otherwise, and the class is what carries that rule.
    expect(toggle().classList.contains('sessions-toggle')).toBe(true);
    expect(toggle().querySelector('svg')?.getAttribute('width')).toBe('15');
  });
});

describe('ChatView composer dock', () => {
  beforeEach(() => {
    resetAppState();
    ResizeObserverStub.reset();
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
  });

  afterEach(() => {
    resetAppState();
    ResizeObserverStub.reset();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  const panel = (): HTMLElement => {
    const element = document.getElementById('workspace-panel-chat');
    if (!element) throw new Error('#workspace-panel-chat not rendered');
    return element;
  };

  it('keeps the composer inside the chat panel, not in its own band', () => {
    const { container } = renderChat();
    const wrap = container.querySelector('.composer-wrap');
    const body = container.querySelector('.chat-body');
    expect(wrap).not.toBeNull();
    // The dock is a sibling of the scroller inside the panel that positions it;
    // it used to be a third flex row of `.chat-panel` (the attached band).
    expect(wrap?.parentElement).toBe(panel());
    expect(panel().contains(body)).toBe(true);
    // The scroll host no longer contains the composer: it floats over it.
    expect(body?.contains(wrap)).toBe(false);
  });

  it('is opaque while the reader is at the newest message', () => {
    const { element } = renderChat();
    fakeScrollBox(element, { scrollHeight: 1600, clientHeight: VIEWPORT });
    element.scrollTop = 1600 - VIEWPORT;
    fireEvent.scroll(element);

    expect(panel().getAttribute('data-reading-history')).toBe('false');
  });

  it('goes translucent when the reader scrolls into history', async () => {
    const { element } = renderChat();
    fakeScrollBox(element, { scrollHeight: 1600, clientHeight: VIEWPORT });

    await scrollTo(element, 200);

    // This attribute is what `.composer` keys its 42%-opacity + backdrop-blur
    // state off (styles/composer.test.ts pins the CSS side).
    expect(panel().getAttribute('data-reading-history')).toBe('true');
  });

  it('turns opaque again when the reader returns to the bottom', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1600, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, 200);
    expect(panel().getAttribute('data-reading-history')).toBe('true');

    await scrollTo(element, box.scrollHeight - box.clientHeight);
    expect(panel().getAttribute('data-reading-history')).toBe('false');
  });

  it('reports the dock height so the transcript can reserve room for it', async () => {
    // jsdom has no layout: give the dock a height and let the observer fire.
    Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
      configurable: true,
      get(this: HTMLElement) {
        return this.classList.contains('composer-wrap') ? 148 : 0;
      },
    });
    try {
      const { container } = renderChat();
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });

      const host = panel();
      expect(host.style.getPropertyValue('--composer-h')).toBe('148px');
      // The scroller's `padding-bottom: calc(var(--composer-h, 0px) + 18px)` and
      // the jump button's `bottom` both read it from this element.
      expect(container.querySelector('.composer-wrap')).not.toBeNull();
    } finally {
      Reflect.deleteProperty(HTMLElement.prototype, 'offsetHeight');
    }
  });

  it('keeps the jump-to-latest button available while translucent', async () => {
    const { element } = renderChat();
    fakeScrollBox(element, { scrollHeight: 1600, clientHeight: VIEWPORT });

    await scrollTo(element, 200);

    // The two states share a condition: a reader in history gets both the
    // see-through dock and the way back to the newest message.
    expect(panel().getAttribute('data-reading-history')).toBe('true');
    expect(jumpButton()).not.toBeNull();
  });
});

describe('control chat project mentions', () => {
  it('lists registered projects as mention candidates', async () => {
    const tree = await projectMentionLoader([
      { id: 'p1', name: 'web-platform' },
      { id: 'p2', name: 'atlas-api' },
    ])(6);
    expect(tree.entries.map((entry) => entry.path)).toEqual(['web-platform', 'atlas-api']);
    expect(tree.entries.every((entry) => entry.kind === 'file')).toBe(true);
  });
});
