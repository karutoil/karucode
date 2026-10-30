import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/preact';
import { LiveAnnouncer } from './LiveAnnouncer';
import { emptyChatState, type ChatItem, type ChatState } from '../state/chat';
import { NOW } from '../test/fixtures';

function chat(overrides: Partial<ChatState> = {}): ChatState {
  return { ...emptyChatState('s_ab12'), ...overrides };
}

function tool(overrides: Partial<Extract<ChatItem, { kind: 'tool' }>> = {}): ChatItem {
  return {
    kind: 'tool',
    id: 'tool:call_1',
    at: NOW,
    callId: 'call_1',
    name: 'read_file',
    args: {},
    status: 'running',
    output: '',
    truncated: false,
    durationMs: null,
    runId: 'r_x',
    ...overrides,
  };
}

function approval(): ChatItem {
  return {
    kind: 'approval',
    id: 'approval:ap_1',
    at: NOW,
    approval: {
      id: 'ap_1',
      kind: 'shell_command',
      title: 'Run cargo test',
      detail: 'cargo test',
      payload: {},
      session_id: 's_ab12',
      run_id: 'r_x',
      agent_id: null,
      status: 'pending',
      decision: null,
      created_at: NOW,
    },
  };
}

/** The polite live region the transcript announces through. */
function liveRegion(): HTMLElement {
  const region = document.querySelector('[aria-live="polite"].sr-only');
  expect(region).not.toBeNull();
  return region as HTMLElement;
}

describe('LiveAnnouncer', () => {
  it('renders a polite live region that is silent on mount', () => {
    render(<LiveAnnouncer chat={chat()} />);
    const region = liveRegion();
    expect(region.getAttribute('aria-live')).toBe('polite');
    // Mounting the view is not news: nothing is announced until something happens.
    expect(region.textContent).toBe('');
  });

  it('announces the run starting and finishing, not every token', () => {
    const { rerender } = render(<LiveAnnouncer chat={chat()} />);
    expect(liveRegion().textContent).toBe('');

    rerender(<LiveAnnouncer chat={chat({ running: true, runStatus: 'running' })} />);
    expect(liveRegion().textContent).toBe('karucode is working.');

    // Streaming text arrives token by token; the region must not repeat it.
    rerender(
      <LiveAnnouncer
        chat={chat({
          running: true,
          runStatus: 'running',
          items: [
            {
              kind: 'message',
              id: 'stream:r_x',
              at: NOW,
              role: 'assistant',
              content: 'The login handler is',
              reasoning: '',
              streaming: true,
              runId: 'r_x',
              agentId: null,
            },
          ],
        })}
      />,
    );
    expect(liveRegion().textContent).toBe('karucode is working.');

    rerender(<LiveAnnouncer chat={chat({ running: false, runStatus: 'completed' })} />);
    expect(liveRegion().textContent).toBe('Run completed.');
  });

  it('announces a failed run as failed', () => {
    const { rerender } = render(<LiveAnnouncer chat={chat({ running: true, runStatus: 'running' })} />);
    rerender(<LiveAnnouncer chat={chat({ running: false, runStatus: 'failed' })} />);
    expect(liveRegion().textContent).toBe('Run failed.');
  });

  it('announces tool activity as it starts and finishes', () => {
    const { rerender } = render(<LiveAnnouncer chat={chat({ running: true, runStatus: 'running' })} />);

    rerender(<LiveAnnouncer chat={chat({ running: true, runStatus: 'running', items: [tool()] })} />);
    expect(liveRegion().textContent).toBe('Running read_file.');

    rerender(
      <LiveAnnouncer
        chat={chat({ running: true, runStatus: 'running', items: [tool({ status: 'ok', output: 'done' })] })}
      />,
    );
    expect(liveRegion().textContent).toBe('read_file finished.');
  });

  it('announces a command by the command line, not the tool name', () => {
    const { rerender } = render(<LiveAnnouncer chat={chat({ running: true, runStatus: 'running' })} />);
    rerender(
      <LiveAnnouncer
        chat={chat({
          running: true,
          runStatus: 'running',
          items: [
            {
              kind: 'command',
              id: 'cmd:call_2',
              at: NOW,
              callId: 'call_2',
              command: 'cargo test',
              cwd: '.',
              stdout: '',
              stderr: '',
              output: '',
              exitCode: null,
              status: 'running',
              truncated: false,
              durationMs: null,
              runId: 'r_x',
            },
          ],
        })}
      />,
    );
    expect(liveRegion().textContent).toBe('Running cargo test.');
  });

  it('announces an approval that is waiting', () => {
    const { rerender } = render(<LiveAnnouncer chat={chat({ running: true, runStatus: 'running' })} />);
    rerender(
      <LiveAnnouncer chat={chat({ running: true, runStatus: 'running', items: [approval()] })} />,
    );
    expect(liveRegion().textContent).toBe('Waiting for approval: Run cargo test.');
  });
});
