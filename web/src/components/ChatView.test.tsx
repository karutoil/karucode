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
    auth_required: false,
    provider_configured: true,
    db: 'ok',
  };
  store.sessions.value = [
    {
      id: sessionId,
      title: 'Fix login bug',
      workspace: '/home/me/project',
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