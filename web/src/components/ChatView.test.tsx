import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { ChatView } from './ChatView';
import type { ChatItem } from '../state/chat';
import * as store from '../state/store';
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
    ResizeObserverStub.reset();
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
  });

  afterEach(() => {
    resetAppState();
    ResizeObserverStub.reset();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function tab(name: RegExp): HTMLElement {
    const found = screen.getByRole('tab', { name });
    return found;
  }

  it('marks the conversation tab selected and keeps the other panes unloaded', () => {
    renderChat();

    expect(tab(/^Conversation/).getAttribute('aria-selected')).toBe('true');
    expect(tab(/^Changes/).getAttribute('aria-selected')).toBe('false');
    expect(document.getElementById('workspace-panel-chat')?.hidden).toBe(false);
    // Lazy: a git probe only runs once the user asks for the changes view.
    expect(document.getElementById('workspace-panel-changes')).toBeNull();
    expect(screen.getByLabelText('Message karucode')).not.toBeNull();
  });

  it('offers no terminal view: there is no shell backend to drive one', () => {
    renderChat();

    expect(screen.queryByRole('tab', { name: /terminal/i })).toBeNull();
    expect(document.getElementById('workspace-panel-terminal')).toBeNull();
    // Command output lives in the transcript and the inspector's Commands tab.
    expect(screen.getAllByRole('tab').map((element) => element.textContent?.trim())).toEqual([
      'Conversation',
      'Changes',
    ]);
  });

  it('mounts the changes pane on first visit and keeps it afterwards', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(JSON.stringify({ error: { code: 'not_found', message: 'no git' } }), { status: 404 })),
    );
    renderChat();
    fireEvent.click(tab(/^Changes/));

    const panel = document.getElementById('workspace-panel-changes');
    expect(panel).not.toBeNull();
    expect(panel?.hidden).toBe(false);
    // The page variant owns the mock's `.secondary-panel` chrome.
    expect(panel?.querySelector('.panel-kicker')?.textContent).toBe('Source control');
    expect(panel?.querySelector('.panel-title')?.textContent).toBe('Review changes');

    fireEvent.click(tab(/^Conversation/));
    expect(document.getElementById('workspace-panel-changes')?.hidden).toBe(true);
    expect(document.getElementById('workspace-tab-changes')).not.toBeNull();
  });

  it('moves between views with the arrow keys', () => {
    renderChat();
    const conversation = tab(/^Conversation/);
    conversation.focus();
    fireEvent.keyDown(conversation, { key: 'ArrowRight' });
    expect(tab(/^Changes/).getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(tab(/^Changes/));

    // Two tabs: the ends are Conversation and Changes, and it wraps.
    fireEvent.keyDown(tab(/^Changes/), { key: 'ArrowRight' });
    expect(tab(/^Conversation/).getAttribute('aria-selected')).toBe('true');

    fireEvent.keyDown(tab(/^Conversation/), { key: 'End' });
    expect(tab(/^Changes/).getAttribute('aria-selected')).toBe('true');

    fireEvent.keyDown(tab(/^Changes/), { key: 'Home' });
    expect(tab(/^Conversation/).getAttribute('aria-selected')).toBe('true');
  });

  it('copies the conversation as Markdown from the header action', async () => {
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    renderChat();

    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));

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
    const copy = screen.getByRole('button', { name: 'Copy' }) as HTMLButtonElement;
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

  it('renders the project → sessions → activity breadcrumb and the session title', () => {
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

    expect(screen.getByText('KaruCode')).not.toBeNull();
    expect(screen.getByText('sessions')).not.toBeNull();
    expect(screen.getByText('ready')).not.toBeNull();
    expect(screen.getByRole('heading', { level: 1, name: 'Fix login bug' })).not.toBeNull();
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

    for (const name of [/^Conversation/, /^Changes/]) {
      const icon = tab(name).querySelector('.tab-icon svg');
      expect(icon?.getAttribute('width')).toBe('15');
    }

    // The count pill is real data (git status), not the mock's demo `3`.
    await waitFor(() => expect(tab(/^Changes/).querySelector('.tab-count')?.textContent).toBe('2'));
  });

  it('marks an idle run with the mock\'s circle-check instead of a faint dot', () => {
    renderChat();
    const note = document.querySelector('.tab-note');
    expect(note?.textContent).toBe('Ready to work');
    expect(note?.querySelector('svg')).not.toBeNull();
    expect(note?.querySelector('.status-dot')).toBeNull();
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

    expect(panel().getAttribute('data-translucent')).toBe('false');
  });

  it('goes translucent when the reader scrolls into history', async () => {
    const { element } = renderChat();
    fakeScrollBox(element, { scrollHeight: 1600, clientHeight: VIEWPORT });

    await scrollTo(element, 200);

    // This attribute is what `.composer` keys its 42%-opacity + backdrop-blur
    // state off (styles/composer.test.ts pins the CSS side).
    expect(panel().getAttribute('data-translucent')).toBe('true');
  });

  it('turns opaque again when the reader returns to the bottom', async () => {
    const { element } = renderChat();
    const box: Box = { scrollHeight: 1600, clientHeight: VIEWPORT };
    fakeScrollBox(element, box);

    await scrollTo(element, 200);
    expect(panel().getAttribute('data-translucent')).toBe('true');

    await scrollTo(element, box.scrollHeight - box.clientHeight);
    expect(panel().getAttribute('data-translucent')).toBe('false');
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
    expect(panel().getAttribute('data-translucent')).toBe('true');
    expect(jumpButton()).not.toBeNull();
  });
});
