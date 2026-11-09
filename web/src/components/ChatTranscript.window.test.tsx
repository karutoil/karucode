import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/preact';
import { useRef } from 'preact/hooks';
import type { JSX } from 'preact';
import { ChatTranscript } from './ChatTranscript';
import type { ChatItem, MessageItem } from '../state/chat';
import { NOW } from '../test/fixtures';
import { resetTheme, updateTheme } from '../theme';

function message(overrides: Partial<MessageItem>): MessageItem {
  return {
    kind: 'message',
    id: 'm',
    at: NOW,
    role: 'assistant',
    content: '',
    reasoning: '',
    streaming: false,
    runId: 'r_x',
    agentId: null,
    ...overrides,
  };
}

/** `turns` question/answer pairs: 2 entries each in the transcript. */
function conversation(turns: number): ChatItem[] {
  const items: ChatItem[] = [];
  for (let index = 0; index < turns; index += 1) {
    items.push(
      message({ id: `u${index}`, role: 'user', content: `question ${index}`, at: NOW + index * 2 }),
      message({ id: `a${index}`, content: `answer ${index}`, at: NOW + index * 2 + 1 }),
    );
  }
  return items;
}

function Harness({ items }: { items: ChatItem[] }): JSX.Element {
  const viewport = useRef<HTMLDivElement>(null);
  return (
    <div id="probe-viewport" ref={viewport}>
      <ChatTranscript
        items={items}
        resolvingApprovals={{}}
        onApprovalDecision={() => undefined}
        viewportRef={viewport}
      />
    </div>
  );
}

const realRect = HTMLElement.prototype.getBoundingClientRect;
const realClientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
const realScrollHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight');

function box(top: number, bottom: number): DOMRect {
  return {
    top,
    bottom,
    height: bottom - top,
    left: 0,
    right: 800,
    width: 800,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

beforeEach(() => {
  localStorage.clear();
  resetTheme();
  act(() => updateTheme({ toolStyle: 'rows', grouping: 'none' }));
  // jsdom has no layout: give the probe viewport a viewport and every row a
  // measurable 100px box so the window has something real to work with.
  HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect(this: HTMLElement) {
    if (this.id === 'probe-viewport') return box(0, 600);
    if (this.parentElement?.classList.contains('transcript')) return box(0, 100);
    return realRect.call(this);
  };
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.id === 'probe-viewport' ? 600 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, 'scrollHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.id === 'probe-viewport' ? 1_000_000 : 0;
    },
  });
});

afterEach(() => {
  cleanup();
  HTMLElement.prototype.getBoundingClientRect = realRect;
  if (realClientHeight) Object.defineProperty(HTMLElement.prototype, 'clientHeight', realClientHeight);
  if (realScrollHeight) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', realScrollHeight);
});

function rows(container: ParentNode): Element[] {
  return [...container.querySelectorAll('.transcript > *')];
}

describe('windowed transcript', () => {
  it('mounts the tail on open instead of the whole conversation', async () => {
    // 100 turns is 200 entries. All of them in the DOM is what made a fully
    // loaded history expensive to open.
    const { container } = render(<Harness items={conversation(100)} />);
    await waitFor(() => expect(rows(container).length).toBeGreaterThan(0));
    expect(rows(container).length).toBeLessThan(60);
    expect(container.textContent).toContain('answer 99');
    expect(container.textContent).not.toContain('question 0');
    // Everything above the window is represented by padding, so the scrollbar
    // still says how much history exists.
    const transcript = container.querySelector('.transcript') as HTMLElement;
    expect(parseFloat(transcript.style.paddingTop || '0')).toBeGreaterThan(0);
  });

  it('moves the window to the head when the reader scrolls to the top', async () => {
    const { container } = render(<Harness items={conversation(100)} />);
    const viewport = container.querySelector('#probe-viewport') as HTMLElement;
    await waitFor(() => expect(rows(container).length).toBeGreaterThan(0));
    viewport.scrollTop = 0;
    fireEvent.scroll(viewport);
    await waitFor(() => expect(container.textContent).toContain('question 0'));
    expect(container.textContent).not.toContain('answer 99');
    // Below the head window there is padding for the rest of the conversation.
    const transcript = container.querySelector('.transcript') as HTMLElement;
    expect(parseFloat(transcript.style.paddingBottom || '0')).toBeGreaterThan(0);
    expect(parseFloat(transcript.style.paddingTop || '0')).toBe(0);
  });

  it('renders every entry when no viewport is available', () => {
    // The embedded renderers and every unit test take this path; a window with
    // no measured viewport would hide most of the transcript for good.
    const { container } = render(
      <ChatTranscript
        items={conversation(12)}
        resolvingApprovals={{}}
        onApprovalDecision={() => undefined}
      />,
    );
    expect(rows(container).length).toBe(25);
  });
});
