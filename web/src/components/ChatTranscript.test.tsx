import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/preact';
import { ChatTranscript } from './ChatTranscript';
import type { ApprovalView, ChatItem, CommandItem, MessageItem, ToolItem } from '../state/chat';
import { approvalFixture, NOW } from '../test/fixtures';
import { resetTheme, THEME_GROUPINGS, THEME_THINKING_MODES, THEME_TOOL_STYLES, THEME_WORKING_MODES, updateTheme } from '../theme';
import * as store from '../state/store';

/* `turnRows` is the per-turn layout pass. It is imported by `ChatTranscript`
 * rather than called through the module object, so replacing it here only works
 * because Vitest hoists this mock above the import above - hence `vi.hoisted`
 * for the counter. The transcript re-renders cost test below counts calls to it
 * as the observable evidence that a memo boundary held. */
const { layoutCalls } = vi.hoisted(() => ({ layoutCalls: [] as number[] }));

vi.mock('../state/transcriptLayout', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../state/transcriptLayout')>();
  return {
    ...actual,
    turnRows: (parts: Parameters<typeof actual.turnRows>[0], grouping: Parameters<typeof actual.turnRows>[1]) => {
      layoutCalls.push(parts.length);
      return actual.turnRows(parts, grouping);
    },
  };
});

function message(overrides: Partial<MessageItem> = {}): MessageItem {
  return {
    kind: 'message',
    id: 'm1',
    at: NOW,
    role: 'assistant',
    content: 'Mapped the registry.',
    reasoning: '',
    streaming: false,
    runId: 'r_x',
    agentId: null,
    ...overrides,
  };
}

function user(content = 'Build a plugin lifecycle.', overrides: Partial<MessageItem> = {}): MessageItem {
  return message({ id: 'u1', role: 'user', content, ...overrides });
}

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  return {
    kind: 'tool',
    id: 't1',
    at: NOW,
    callId: 'call_1',
    name: 'read_file',
    args: { path: 'src/main.rs' },
    status: 'ok',
    output: 'fn main() {}\n',
    truncated: false,
    durationMs: 12,
    runId: 'r_x',
    ...overrides,
  };
}

function command(overrides: Partial<CommandItem> = {}): CommandItem {
  return {
    kind: 'command',
    id: 'c1',
    at: NOW,
    callId: 'call_2',
    command: 'cargo test',
    cwd: '.',
    stdout: 'ok\n',
    stderr: '',
    output: '',
    exitCode: 0,
    status: 'ok',
    truncated: false,
    durationMs: 900,
    runId: 'r_x',
    ...overrides,
  };
}

function approvalView(overrides: Partial<ApprovalView> = {}): ApprovalView {
  return {
    id: approvalFixture.id,
    kind: approvalFixture.kind,
    title: approvalFixture.title,
    detail: approvalFixture.detail,
    payload: approvalFixture.payload,
    session_id: approvalFixture.session_id,
    run_id: approvalFixture.run_id,
    agent_id: null,
    status: 'pending',
    decision: null,
    created_at: NOW,
    ...overrides,
  };
}

function renderTranscript(
  items: ChatItem[],
  onApprovalDecision = vi.fn(),
  onRewind?: (target: { messageId: number; preview: string; isAssistant: boolean }) => void,
) {
  return render(
    <ChatTranscript
      items={items}
      resolvingApprovals={{}}
      onApprovalDecision={onApprovalDecision}
      onRewind={onRewind}
    />,
  );
}

describe('ChatTranscript', () => {
  beforeEach(() => {
    localStorage.clear();
    resetTheme();
    // These are component-structure tests, not default-preference tests. Keep
    // their cards/no-grouping fixture explicit as the built-in default evolves.
    act(() => updateTheme({ toolStyle: 'cards', grouping: 'none' }));
  });

  afterEach(() => resetTheme());

  it('renders one assistant row per turn, with the cards inside its body', () => {
    renderTranscript([user(), message(), tool(), message({ id: 'm2', content: 'Changes are ready.' })]);

    const turns = document.querySelectorAll('.msg-assistant');
    expect(turns).toHaveLength(1);
    expect(turns[0]?.querySelectorAll('.msg-avatar')).toHaveLength(1);
    // The tool card is part of the same body column as the prose.
    expect(turns[0]?.querySelector('.msg-body .tool-card')).not.toBeNull();
    expect(screen.getByText('Mapped the registry.')).not.toBeNull();
    expect(screen.getByText('Changes are ready.')).not.toBeNull();
  });

  it('starts a new turn after every user message', () => {
    renderTranscript([
      user('first'),
      message({ id: 'a1' }),
      user('second', { id: 'u2' }),
      message({ id: 'a2' }),
    ]);

    expect(document.querySelectorAll('.msg-assistant')).toHaveLength(2);
    expect(document.querySelectorAll('.msg-user')).toHaveLength(2);
    // The mock's row rhythm: avatar + author for each speaker.
    expect(screen.getAllByText('karucode')).toHaveLength(2);
    expect(screen.getAllByText('You')).toHaveLength(2);
  });

  it('labels the rows for screen readers', () => {
    renderTranscript([user(), message()]);
    expect(screen.getByLabelText('Your message')).not.toBeNull();
    expect(screen.getByLabelText('karucode response')).not.toBeNull();
  });

  it('shows a date marker before the first message of each day', () => {
    const today = Date.now();
    const yesterday = today - 86_400_000;
    renderTranscript([
      message({ id: 'a1', at: yesterday, content: 'old' }),
      user('new', { at: today }),
      message({ id: 'a2', at: today, content: 'new answer' }),
    ]);

    const markers = [...document.querySelectorAll('.date-marker')].map((node) => node.textContent ?? '');
    expect(markers).toHaveLength(2);
    expect(markers.some((text) => text.startsWith('Today ·'))).toBe(true);
    expect(markers.some((text) => text.startsWith('Yesterday ·'))).toBe(true);
  });

  it('leaves routine completed metadata out of the reading flow', () => {
    renderTranscript([user(), message()]);
    const top = document.querySelector('.msg-assistant .msg-top');
    expect(top?.textContent).toBe('karucode');
    expect(top?.querySelector('.model-pill')).toBeNull();
    expect(top?.querySelector('.msg-time')).toBeNull();
    expect(top?.querySelector('.tag')).toBeNull();
  });

  it('marks a live turn as Working', () => {
    renderTranscript([user(), message({ streaming: true, content: 'Half an ans' })]);
    const tag = document.querySelector('.msg-assistant .tag');
    expect(tag?.textContent).toContain('Working');
    expect(tag?.className).toContain('tag-live');
    expect(document.querySelector('.msg-assistant .msg-time')).not.toBeNull();
    expect(document.querySelector('.stream-caret')).not.toBeNull();
  });

  describe('throughput badge', () => {
    it('rides the streaming message it measures, not the turn header', () => {
      renderTranscript([user(), message({ streaming: true, content: 'x'.repeat(8_000) })]);
      // The header only says *whether* the turn works; the number belongs to the
      // block that is producing text, where the reader is already looking.
      expect(document.querySelector('.msg-assistant .msg-top .tps-badge')).toBeNull();
      const badge = document.querySelector('.msg-assistant .msg-body .tps-badge');
      expect(badge, 'the badge is mounted on the message').not.toBeNull();
      // 8 000 characters at four chars per token, sampled on mount. Exact below
      // 10k, so the count reads as itself rather than a rounded "2k".
      expect(badge?.querySelector('.tps-badge-count')?.textContent).toBe('2000');
    });

    it('gives a streaming thought its own badge, measured on the reasoning', () => {
      renderTranscript([
        user(),
        message({ streaming: true, content: '', reasoning: 'r'.repeat(2_000) }),
      ]);
      const thinking = document.querySelector('.thinking');
      expect(thinking?.querySelector('.tps-badge-count')?.textContent).toBe('500');
      // Exactly one badge on the turn: the prose block has no text yet, so it
      // claims nothing.
      expect(document.querySelectorAll('.tps-badge')).toHaveLength(1);
    });

    it('moves to a running call, measuring that call and not the turn', () => {
      // The prose is finished but the call is not: the badge on the card counts
      // the output the call is producing, which is the only stream still live.
      renderTranscript([
        user(),
        message({ content: 'Running the suite.' }),
        tool({ status: 'running', output: 'y'.repeat(4_000) }),
      ]);
      expect(document.querySelector('.msg-assistant .msg-top .tps-badge')).toBeNull();
      const card = document.querySelector('.tool-card');
      expect(card?.querySelector('.tps-badge-count')?.textContent).toBe('1000');
      // …and the turn is not reported as finished while a call is in flight.
      expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Working');
    });

    it('is gone once the turn has settled', () => {
      renderTranscript([user(), message(), tool()]);
      expect(document.querySelector('.tps-badge')).toBeNull();
    });

    it('is absent from the user row', () => {
      renderTranscript([user('hello')]);
      expect(document.querySelector('.msg-user .tps-badge')).toBeNull();
    });
  });

  it('reports a failed turn instead of claiming it completed', () => {
    renderTranscript([
      user(),
      message(),
      { kind: 'error', id: 'e1', at: NOW, message: 'provider exploded', errorKind: 'stream', runId: 'r_x' },
    ]);
    expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Failed');
    expect(screen.getByRole('alert').textContent).toContain('provider exploded');
  });

  it('reports an interrupted turn', () => {
    renderTranscript([
      user(),
      message(),
      { kind: 'interrupted', id: 'i1', at: NOW, runId: 'r_x', message: 'Stopped by you.' },
    ]);
    expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Interrupted');
    expect(screen.getByText(/Stopped by you\./)).not.toBeNull();
  });

  it('renders commands inside the turn and keeps running ones open', () => {
    renderTranscript([user(), command({ status: 'running', exitCode: null, durationMs: null })]);
    const card = document.querySelector('.msg-body .command-card');
    expect(card).not.toBeNull();
    expect(card?.getAttribute('data-open')).toBe('true');
  });

  it('keeps an approval card in the turn and reports the decision', () => {
    const onDecide = vi.fn();
    renderTranscript(
      [user(), message(), { kind: 'approval', id: 'ap', at: NOW, approval: approvalView() }],
      onDecide,
    );

    expect(document.querySelector('.msg-body .approval-card')).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Allow once' }));
    expect(onDecide).toHaveBeenCalledWith(approvalFixture.id, 'allow_once');
  });

  it('renders an assistant tool call that arrives without prose', () => {
    renderTranscript([tool()]);
    expect(document.querySelectorAll('.msg-assistant')).toHaveLength(1);
    expect(document.querySelector('.msg-body .tool-card')).not.toBeNull();
  });

  it('renders notices with the right live-region role', () => {
    renderTranscript([
      user(),
      { kind: 'notice', id: 'n1', at: NOW, level: 'warn', message: 'Context is nearly full.' },
    ]);
    expect(screen.getByRole('status').textContent).toContain('Context is nearly full.');
  });

  it('shows an empty transcript without inventing rows', () => {
    renderTranscript([]);
    expect(document.querySelectorAll('.msg')).toHaveLength(0);
    expect(document.querySelector('.date-marker')).toBeNull();
  });
});

describe('ChatTranscript view options', () => {
  function assistantWithReasoning(overrides: Partial<MessageItem> = {}): MessageItem {
    return message({
      id: 'mr',
      content: '',
      reasoning: 'The config lives in settings.rs.',
      ...overrides,
    });
  }

  beforeEach(() => {
    localStorage.clear();
    resetTheme();
    // These tests are about the folded idiom; the open default has its own
    // tests below and each one names the option it needs.
    act(() => updateTheme({ toolStyle: 'cards', grouping: 'none', working: 'folded' }));
  });

  afterEach(() => {
    resetTheme();
  });

  function setOptions(patch: { toolStyle?: 'cards' | 'rows'; grouping?: 'none' | 'tools' | 'turns'; working?: 'open' | 'folded'; thinking?: 'collapsed' | 'preview' | 'expanded' }) {
    act(() => {
      updateTheme(patch);
    });
  }

  it('uses compact rows, completed-turn grouping and open working as the built-in default', () => {
    act(() => resetTheme());
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    expect(document.documentElement.dataset.toolStyle).toBe('rows');
    expect(document.documentElement.dataset.grouping).toBe('turns');
    expect(document.documentElement.dataset.working).toBe('open');
    expect(screen.getByText('The pool size is 16.')).not.toBeNull();
    // The working is expanded out of the box: the narration and the call are on
    // screen, and the summary line is a header that folds them away again.
    expect(screen.getByText('Reading the config.')).not.toBeNull();
    expect(document.querySelector('.msg-body .tool-row')).not.toBeNull();
    expect(document.querySelector('.tool-card')).toBeNull();
    const fold = document.querySelector('.process-fold');
    expect(fold?.getAttribute('aria-expanded')).toBe('true');
    expect(fold?.textContent).toContain('Hide the working');

    fireEvent.click(fold as Element);
    expect(screen.queryByText('Reading the config.')).toBeNull();
    expect(document.querySelector('.msg-body .tool-row')).toBeNull();
    expect(document.querySelector('.process-fold')?.getAttribute('aria-expanded')).toBe('false');
  });

  it('draws tool calls as compact rows when the theme asks for it', () => {
    setOptions({ toolStyle: 'rows' });
    renderTranscript([user(), tool()]);

    expect(document.querySelector('.tool-card')).toBeNull();
    expect(document.querySelector('.msg-body .tool-row')).not.toBeNull();
    // The row still exposes its output behind the same disclosure contract.
    const head = document.querySelector('.tool-row-head');
    expect(head?.getAttribute('aria-expanded')).toBe('false');
  });

  it('leaves consecutive calls alone with grouping off', () => {
    renderTranscript([user(), tool({ id: 't1' }), tool({ id: 't2', callId: 'c2' }), message()]);
    expect(document.querySelectorAll('.tool-group')).toHaveLength(0);
    expect(document.querySelectorAll('.tool-card')).toHaveLength(2);
  });

  it('folds a run of consecutive calls into one line', () => {
    setOptions({ grouping: 'tools' });
    renderTranscript([
      user(),
      tool({ id: 't1', name: 'read_file' }),
      tool({ id: 't2', callId: 'c2', name: 'grep' }),
      message({ id: 'a2' }),
    ]);

    const group = document.querySelector('.tool-group');
    expect(group).not.toBeNull();
    expect(group?.querySelector('.tool-group-summary')?.textContent).toContain('read_file');
    expect(group?.querySelector('.tool-group-count')?.textContent).toBe('2 calls');
    // Collapsed by default because nothing is running and nothing failed.
    expect(group?.querySelector('.tool-group-head')?.getAttribute('aria-expanded')).toBe('false');
  });

  it('marks prose between calls as narration, and the final answer as not', () => {
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Let me read the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    const narrated = document.querySelectorAll('.narration');
    expect(narrated).toHaveLength(1);
    expect(narrated[0]?.textContent).toContain('Let me read the config.');
    expect(screen.getByText('The pool size is 16.').closest('.narration')).toBeNull();
  });

  it('folds a finished turn behind a summary of what it did', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool({ id: 't1' }),
      tool({ id: 't2', callId: 'c2', name: 'grep' }),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    const fold = document.querySelector('.process-fold');
    expect(fold).not.toBeNull();
    expect(fold?.getAttribute('aria-expanded')).toBe('false');
    expect(fold?.textContent).toContain('2 tool calls');
    // The answer survives the fold; the working steps do not.
    expect(screen.getByText('The pool size is 16.')).not.toBeNull();
    expect(screen.queryByText('Reading the config.')).toBeNull();
    expect(document.querySelector('.tool-group')).toBeNull();
    expect(document.querySelector('.tool-card')).toBeNull();
  });

  it('opens the folded turn from its summary line', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    fireEvent.click(document.querySelector('.process-fold') as Element);
    expect(screen.getByText('Reading the config.')).not.toBeNull();
    expect(document.querySelector('.tool-row, .tool-card')).not.toBeNull();
    expect(document.querySelector('.process-fold')?.getAttribute('aria-expanded')).toBe('true');
  });

  it('starts a finished turn open when the theme asks for open working', () => {
    setOptions({ grouping: 'turns', working: 'open' });
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    const fold = document.querySelector('.process-fold');
    expect(fold?.getAttribute('aria-expanded')).toBe('true');
    expect(fold?.textContent).toContain('Hide the working');
    expect(screen.getByText('Reading the config.')).not.toBeNull();
    expect(screen.getByText('The pool size is 16.')).not.toBeNull();
  });

  it('expands the calls inside a run, not just the run itself', () => {
    setOptions({ grouping: 'tools', working: 'open', toolStyle: 'rows' });
    renderTranscript([
      user(),
      tool({ id: 't1' }),
      tool({ id: 't2', callId: 'c2', name: 'grep' }),
      message({ id: 'fin', content: 'Both read.' }),
    ]);

    // A run asked to show its work shows the calls inside it, rather than
    // stopping one level short at another summary line.
    const head = document.querySelector('.tool-group-head') as HTMLButtonElement;
    expect(head.getAttribute('aria-expanded')).toBe('true');
    expect(document.querySelectorAll('.tool-row')).toHaveLength(2);

    // The group's own header still folds just this run.
    fireEvent.click(head);
    expect(document.querySelectorAll('.tool-row')).toHaveLength(0);
  });

  it('keeps one turn’s expansion to that turn', () => {
    setOptions({ grouping: 'turns', working: 'folded' });
    renderTranscript([
      user(),
      tool({ id: 't1' }),
      message({ id: 'a1', content: 'First answer.' }),
      user('second', { id: 'u2' }),
      tool({ id: 't2', callId: 'c2' }),
      message({ id: 'a2', content: 'Second answer.' }),
    ]);

    const folds = document.querySelectorAll('.process-fold');
    expect(folds).toHaveLength(2);
    fireEvent.click(folds[0] as Element);
    expect(document.querySelectorAll('.process-fold')[0]?.getAttribute('aria-expanded')).toBe('true');
    // The other turn is untouched: the fold is per row, never transcript-wide.
    expect(document.querySelectorAll('.process-fold')[1]?.getAttribute('aria-expanded')).toBe('false');
  });

  it('repaints a turn whose working the reader never touched', () => {
    setOptions({ grouping: 'turns', working: 'folded' });
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);
    expect(screen.queryByText('Reading the config.')).toBeNull();

    // Changing the option is not defeated by a choice nobody made.
    setOptions({ working: 'open' });
    expect(screen.getByText('Reading the config.')).not.toBeNull();
  });

  it('never folds a turn that is still working', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      tool(),
      message({ id: 'fin', content: 'Working on it', streaming: true }),
    ]);
    expect(document.querySelector('.process-fold')).toBeNull();
    expect(document.querySelector('.tool-row, .tool-card')).not.toBeNull();
  });

  it('never folds a turn that is waiting on an approval', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      tool(),
      message({ id: 'fin', content: 'I need permission.' }),
      { kind: 'approval', id: 'ap', at: NOW, approval: approvalView() },
    ]);
    expect(document.querySelector('.process-fold')).toBeNull();
    expect(screen.getByRole('button', { name: 'Allow once' })).not.toBeNull();
  });

  it('keeps a notice visible above the fold instead of hiding it', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      { kind: 'notice', id: 'n1', at: NOW, level: 'warn', message: 'Context is nearly full.' },
      tool(),
      message({ id: 'fin', content: 'Done, with caveats.' }),
    ]);
    expect(document.querySelector('.process-fold')).not.toBeNull();
    expect(screen.getByRole('status').textContent).toContain('Context is nearly full.');
  });

  it('passes the thinking mode to the reasoning block', () => {
    setOptions({ thinking: 'preview' });
    renderTranscript([user(), assistantWithReasoning()]);

    const preview = document.querySelector('.thinking-preview');
    expect(document.querySelector('.thinking')?.getAttribute('data-mode')).toBe('preview');
    expect(preview?.textContent).toContain('The config lives in settings.rs.');
  });

  it('shows reasoning open when the theme asks for it', () => {
    setOptions({ thinking: 'expanded' });
    renderTranscript([user(), assistantWithReasoning()]);
    expect(document.querySelector('.thinking')?.hasAttribute('open')).toBe(true);
  });
});

describe('ChatTranscript option matrix', () => {
  beforeEach(() => {
    localStorage.clear();
    resetTheme();
    act(() => updateTheme({ toolStyle: 'cards', grouping: 'none' }));
  });

  afterEach(() => {
    resetTheme();
  });

  function setOptions(patch: {
    toolStyle?: 'cards' | 'rows';
    grouping?: 'none' | 'tools' | 'turns';
    working?: 'open' | 'folded';
    thinking?: 'collapsed' | 'preview' | 'expanded';
  }) {
    act(() => {
      updateTheme(patch);
    });
  }

  /** Every toolStyle × grouping × working × thinking combination: 2 × 3 × 2 × 3 = 36. */
  const combinations = THEME_TOOL_STYLES.flatMap((toolStyle) =>
    THEME_GROUPINGS.flatMap((grouping) =>
      THEME_WORKING_MODES.flatMap((working) =>
        THEME_THINKING_MODES.map((thinking) => ({ toolStyle, grouping, working, thinking })),
      ),
    ),
  );

  /**
   * One turn that exercises every part the options touch: narration, reasoning,
   * two tools, a shell command and a final answer. `turnItems()` rebuilds it so a
   * test can hand a changed array to a re-render.
   */
  function turnItems(): ChatItem[] {
    return [
      user(),
      message({
        id: 'a1',
        content: 'Let me check the config.',
        reasoning: 'The config is in settings.rs.',
      }),
      tool({ id: 't1', name: 'read_file' }),
      tool({ id: 't2', callId: 'call_2', name: 'grep' }),
      command({ id: 'c1', callId: 'call_3' }),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ];
  }

  it('has all thirty-six combinations to cover', () => {
    expect(combinations).toHaveLength(36);
    const keys = combinations.map(
      ({ toolStyle, grouping, working, thinking }) => `${toolStyle}/${grouping}/${working}/${thinking}`,
    );
    expect(new Set(keys).size).toBe(36);
  });

  it.each(combinations)(
    '$toolStyle calls · $grouping grouping · $working working · $thinking reasoning',
    ({ toolStyle, grouping, working, thinking }) => {
      setOptions({ toolStyle, grouping, working, thinking });
      renderTranscript(turnItems());

      // ---- the answer is never lost ------------------------------------
      expect(screen.getByText('The pool size is 16.')).not.toBeNull();

      // ---- only `turns` folds, and it never folds the answer away -------
      const fold = document.querySelector<HTMLButtonElement>('.process-fold');
      if (grouping !== 'turns') {
        expect(fold).toBeNull();
      } else if (working === 'open') {
        // Open by default, but still a disclosure: the summary says what is on
        // screen and folding it away is one click, on this turn only.
        expect(fold, 'an open turn still names what it holds').not.toBeNull();
        expect(fold?.getAttribute('aria-expanded')).toBe('true');
        expect(fold?.textContent).toContain('Hide the working');
        expect(screen.getByText('Let me check the config.')).not.toBeNull();
        fireEvent.click(fold as HTMLButtonElement);
        expect(screen.queryByText('Let me check the config.')).toBeNull();
        expect(document.querySelector('.process-fold')?.textContent).toContain('Show the working');
        fireEvent.click(document.querySelector('.process-fold') as HTMLButtonElement);
        expect(screen.getByText('Let me check the config.')).not.toBeNull();
      } else {
        expect(fold, 'a finished turn folds its working steps').not.toBeNull();
        expect(fold?.getAttribute('aria-expanded')).toBe('false');
        // The narration is part of the working, so it is behind the summary.
        expect(screen.queryByText('Let me check the config.')).toBeNull();
        expect(fold?.textContent).toContain('Show the working');
        fireEvent.click(fold as HTMLButtonElement);
        expect(screen.getByText('Let me check the config.')).not.toBeNull();
      }

      // ---- the reasoning is available in the mode the theme asked for ---
      const reasoning = document.querySelector('.thinking');
      expect(reasoning?.getAttribute('data-mode')).toBe(thinking);
      if (thinking === 'expanded') {
        expect(reasoning?.hasAttribute('open')).toBe(true);
        expect(reasoning?.querySelector('.thinking-body')?.textContent).toContain(
          'The config is in settings.rs.',
        );
      } else if (thinking === 'preview') {
        expect(reasoning?.hasAttribute('open')).toBe(false);
        expect(reasoning?.querySelector('.thinking-preview')?.textContent).toBe(
          'The config is in settings.rs.',
        );
      } else {
        expect(reasoning?.hasAttribute('open')).toBe(false);
        expect(reasoning?.querySelector('.thinking-hint')?.textContent).toBe('Show reasoning');
      }

      // ---- the group starts in the state the working option asked for ----
      if (grouping === 'none') {
        expect(document.querySelector('.tool-group')).toBeNull();
      } else {
        const group = document.querySelector('.tool-group');
        expect(group, 'a run of calls folds into one group').not.toBeNull();
        // The summary names what ran, so folding does not hide *what* happened.
        expect(group?.querySelector('.tool-group-summary')?.textContent).toContain('read_file');
        expect(group?.querySelector('.tool-group-summary')?.textContent).toContain('run_command');
        const head = group?.querySelector('.tool-group-head') as HTMLButtonElement;
        // Open working opens the run too, so the calls themselves are on screen
        // rather than one more summary line the reader has to click through.
        expect(head.getAttribute('aria-expanded')).toBe(working === 'open' ? 'true' : 'false');
        if (working === 'folded') fireEvent.click(head);
      }

      const rows = toolStyle === 'rows';
      expect(document.querySelectorAll('.tool-row')).toHaveLength(rows ? 2 : 0);
      expect(document.querySelectorAll('.tool-card')).toHaveLength(rows ? 0 : 2);
      // A shell command follows the same style in every grouping: rows are not
      // just for tool calls.
      expect(document.querySelectorAll('.command-row')).toHaveLength(rows ? 1 : 0);
      expect(document.querySelectorAll('.command-card')).toHaveLength(rows ? 0 : 1);

      // Narration reads as a quiet note wherever it is not folded away.
      expect(document.querySelector('.narration')?.textContent).toContain('Let me check the config.');
    },
  );

  describe('running, error and pending-approval transitions', () => {
    function view(items: ChatItem[]) {
      const onApprovalDecision = vi.fn();
      return render(
        <ChatTranscript items={items} resolvingApprovals={{}} onApprovalDecision={onApprovalDecision} />,
      );
    }

    it('opens a collapsed tool group when a member starts running', () => {
      setOptions({ grouping: 'tools', working: 'folded' });
      const items = turnItems();
      const rendered = view(items);
      const head = () => document.querySelector('.tool-group-head') as HTMLButtonElement;
      expect(head().getAttribute('aria-expanded')).toBe('false');

      const next = items.map((item) =>
        item.kind === 'tool' && item.id === 't2' ? { ...item, status: 'running' as const, durationMs: null } : item,
      );
      rendered.rerender(
        <ChatTranscript items={next} resolvingApprovals={{}} onApprovalDecision={vi.fn()} />,
      );

      expect(head().getAttribute('aria-expanded')).toBe('true');
      expect(screen.getByText('1 running')).not.toBeNull();
      // The live call is rendered, not merely counted: two tools and a command.
      expect(document.querySelectorAll('.tool-card, .command-card')).toHaveLength(3);
    });

    it('opens a collapsed tool group when a member fails, output and all', () => {
      setOptions({ grouping: 'tools', working: 'folded' });
      const items = turnItems();
      const rendered = view(items);
      expect(
        (document.querySelector('.tool-group-head') as HTMLButtonElement).getAttribute('aria-expanded'),
      ).toBe('false');

      const next = items.map((item) =>
        item.kind === 'tool' && item.id === 't2'
          ? { ...item, status: 'error' as const, output: 'permission denied' }
          : item,
      );
      rendered.rerender(
        <ChatTranscript items={next} resolvingApprovals={{}} onApprovalDecision={vi.fn()} />,
      );

      expect(screen.getByText('1 failed')).not.toBeNull();
      expect(document.querySelector('.tool-group')?.getAttribute('data-tone')).toBe('is-error');
      // The card style shows the failure's output in the expanded group.
      expect(document.querySelector('.msg-assistant .msg-body')?.textContent).toContain(
        'permission denied',
      );
    });

    it('never folds a failed turn and keeps the error as an alert', () => {
      setOptions({ grouping: 'turns' });
      renderTranscript([
        user(),
        message({ id: 'a1', content: 'Trying the build.' }),
        tool(),
        {
          kind: 'error',
          id: 'e1',
          at: NOW,
          message: 'provider exploded',
          errorKind: 'stream',
          runId: 'r_x',
        },
      ]);

      expect(document.querySelector('.process-fold')).toBeNull();
      expect(screen.getByRole('alert').textContent).toContain('provider exploded');
      expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Failed');
    });

    it('keeps a pending approval out of the fold and actionable', () => {
      setOptions({ grouping: 'turns' });
      renderTranscript([
        user(),
        tool(),
        message({ id: 'fin', content: 'I need permission to continue.' }),
        { kind: 'approval', id: 'ap', at: NOW, approval: approvalView() },
      ]);

      // A pending approval is work, so the turn stays open...
      expect(document.querySelector('.process-fold')).toBeNull();
      // ...and the decision is one click away.
      expect(screen.getByRole('button', { name: 'Allow once' })).not.toBeNull();
      expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Working');
    });

    it.each(THEME_TOOL_STYLES)('keeps a running shell command open in %s', (toolStyle) => {
      setOptions({ toolStyle });
      renderTranscript([user(), command({ status: 'running', exitCode: null, durationMs: null })]);

      const surface = document.querySelector(toolStyle === 'rows' ? '.command-row' : '.command-card');
      expect(surface?.getAttribute('data-open')).toBe('true');
      // Its output is on screen while it runs: that is the point of opening it.
      expect(document.querySelector('.cmd-output')?.textContent).toContain('ok');
    });
  });
});

describe('ChatTranscript rewind affordance', () => {
  it('offers a rewind on a persisted user message and names its id', () => {
    const onRewind = vi.fn();
    renderTranscript([user('Build a plugin lifecycle.', { id: 'msg:412' })], vi.fn(), onRewind);

    const button = screen.getByRole('button', { name: 'Rewind to this message' });
    fireEvent.click(button);
    expect(onRewind).toHaveBeenCalledWith({
      messageId: 412,
      preview: 'Build a plugin lifecycle.',
      isAssistant: false,
    });
  });

  it('offers a rewind on the answer, which means the request behind it', () => {
    const onRewind = vi.fn();
    renderTranscript(
      [user('go', { id: 'msg:1' }), message({ id: 'msg:2', content: 'Done.' })],
      vi.fn(),
      onRewind,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Rewind to the request that produced this answer' }));
    expect(onRewind).toHaveBeenCalledWith({
      messageId: 2,
      preview: 'Done.',
      isAssistant: true,
    });
  });

  it('offers nothing for an item the server cannot resolve', () => {
    const onRewind = vi.fn();
    // `local:` is an optimistic send and `stream:` is a live placeholder; neither
    // has a row the rewind endpoint could look up.
    renderTranscript([user('pending', { id: 'local:1' })], vi.fn(), onRewind);
    expect(screen.queryByRole('button', { name: /Rewind/ })).toBeNull();
  });

  it('hides the affordance while the agent is still writing', () => {
    store.chat.value = { ...store.chat.value, running: true };
    renderTranscript([user('go', { id: 'msg:1' })], vi.fn(), vi.fn());
    expect(screen.queryByRole('button', { name: /Rewind/ })).toBeNull();
  });

  it('offers nothing when the transcript is read-only', () => {
    renderTranscript([user('go', { id: 'msg:1' })]);
    expect(screen.queryByRole('button', { name: /Rewind/ })).toBeNull();
  });
});

/* Re-render cost.
 *
 * A long session is thousands of items, and a run streams a token at a time, so
 * the transcript re-renders hundreds of times per answer. Before the memo
 * boundaries every one of those re-renders re-parsed every Markdown body and
 * re-laid-out every card: measured in jsdom on 1553 items, 18,650 DOM nodes,
 * ~1173 ms for the first render and ~143 ms per rerender.
 *
 * The boundaries are only real if identity holds - the reducer keeps settled
 * items and turns by identity, `options` is memoised on the theme fields, and
 * the callbacks are excluded from the comparison - so this measures the
 * behaviour rather than the source. A `shouldComponentUpdate` spy on the memo
 * wrapper's prototype is the cheapest way to observe "did this row re-render";
 * jsdom cannot measure layout, so the wall-clock number here is a *render*
 * measurement, and the absolute figure is machine-dependent. What is asserted is
 * the ratio, and that the settled rows are not re-rendered at all.
 */
describe('ChatTranscript rerender cost', () => {
  /** `count` items alternating user question and assistant answer, plus a live tail. */
  function longConversation(turns: number): ChatItem[] {
    const items: ChatItem[] = [];
    for (let index = 0; index < turns; index += 1) {
      items.push(user(`question ${index}`, { id: `msg:u${index}`, at: NOW + index * 2 }));
      items.push(
        message({
          id: `msg:a${index}`,
          at: NOW + index * 2 + 1,
          content: `Answer ${index}. `.repeat(40),
        }),
      );
    }
    return items;
  }

  it('skips settled rows when only the streaming tail changes', () => {
    const settled = longConversation(40);
    // `turnRows` is the per-turn layout pass (grouping runs of calls, marking
    // narration). It is memoised *inside* `AssistantTurn`, so it runs again only
    // when that turn re-renders - which is exactly what the boundary prevents.
    // Counting it is the direct observation, where DOM identity is not: Preact
    // keeps a node when the props are equal, so a re-render is invisible in the
    // tree but very visible in the cost.
    layoutCalls.length = 0;
    const onApprovalDecision = vi.fn();
    const view = render(
      <ChatTranscript items={settled} resolvingApprovals={{}} onApprovalDecision={onApprovalDecision} />,
    );
    expect(layoutCalls).toHaveLength(40);

    // Stream one more token into a new tail row. Every settled item is the same
    // object the reducer already handed us.
    const tail = message({ id: 'stream:r1:1:1', at: NOW + 10_000, content: 'Streaming', streaming: true });
    layoutCalls.length = 0;
    view.rerender(
      <ChatTranscript
        items={[...settled, { ...tail, content: 'Streaming more text arrived just now.' }]}
        resolvingApprovals={{}}
        onApprovalDecision={onApprovalDecision}
      />,
    );

    // The new text is rendered - the boundaries did not swallow content...
    expect(screen.getByText('Streaming more text arrived just now.')).not.toBeNull();
    // ...but only the *new* turn was laid out. Before the boundaries this was 41.
    expect(layoutCalls).toHaveLength(1);
  });

  it('re-renders a settled conversation far cheaper than the first render', () => {
    // A run streams a token at a time, so this is the hot path: a scenario with a
    // user row plus a long answer, per turn, and one new token per rerender.
    const settled = longConversation(300);
    const approvals = {};
    const onApprovalDecision = vi.fn();
    const streaming = (suffix: string): ChatItem[] => [
      ...settled,
      message({
        id: 'stream:r1:1:1',
        at: NOW + 10_000,
        streaming: true,
        content: `Answer in progress ${suffix}`,
      }),
    ];

    const started = performance.now();
    const view = render(
      <ChatTranscript
        items={streaming('')}
        resolvingApprovals={approvals}
        onApprovalDecision={onApprovalDecision}
      />,
    );
    const firstRenderMs = performance.now() - started;

    const rerenders = 10;
    const rerenderStart = performance.now();
    for (let index = 0; index < rerenders; index += 1) {
      act(() => {
        view.rerender(
          <ChatTranscript
            items={streaming('.'.repeat(index))}
            resolvingApprovals={approvals}
            onApprovalDecision={onApprovalDecision}
          />,
        );
      });
    }
    const perRerenderMs = (performance.now() - rerenderStart) / rerenders;

    // The bound is a *ratio*, so it holds on any machine: with the memo
    // boundaries a streamed token re-renders one turn; without them it re-lays
    // out every turn, which is the first-render cost over again (measured in this
    // tree: 143 ms/rerender on 1552 items, against 10.9 ms with the boundaries).
    expect(perRerenderMs).toBeLessThan(firstRenderMs / 4);
  });

  it('streams a long answer at the same cost per token as a short one', () => {
    // The other half of the hot path, and the one that used to grow without
    // bound: `Markdown` re-parsed the *whole* message on every delta, so a token
    // cost more the longer the answer got (measured in the shipped bundle: 120
    // KiB of answer was ~50 ms per render, which is a 14 fps tab). With the
    // settled/live split a token costs the block being written, so the ratio
    // between the end of a long answer and its beginning stays flat. Before the
    // split this ratio tracked the answer's length (~30x by 150 KiB).
    const paragraph = 'A sentence of the answer that keeps going. '.repeat(6) + '\n\n';
    let content = paragraph.repeat(10);
    const items = (): ChatItem[] => [
      message({ id: 'stream:r1:1:1', at: NOW + 10_000, streaming: true, content }),
    ];
    const approvals = {};
    const onApprovalDecision = vi.fn();
    const view = render(
      <ChatTranscript items={items()} resolvingApprovals={approvals} onApprovalDecision={onApprovalDecision} />,
    );
    // The median of ten rerenders, not their mean: a scheduler stall or a GC
    // pause inside the loop is one sample, and averaging it in makes the
    // comparison depend on what else the machine was doing rather than on what a
    // token costs. That is what made this test flake under a parallel run.
    const perToken = (): number => {
      const samples: number[] = [];
      for (let index = 0; index < 10; index += 1) {
        const start = performance.now();
        content += 'token ';
        act(() => {
          view.rerender(
            <ChatTranscript
              items={items()}
              resolvingApprovals={approvals}
              onApprovalDecision={onApprovalDecision}
            />,
          );
        });
        samples.push(performance.now() - start);
      }
      samples.sort((a, b) => a - b);
      return samples[Math.floor(samples.length / 2)]!;
    };

    const early = perToken();
    while (content.length < 150_000) content += paragraph;
    act(() => {
      view.rerender(
        <ChatTranscript items={items()} resolvingApprovals={approvals} onApprovalDecision={onApprovalDecision} />,
      );
    });
    const late = perToken();

    expect(late).toBeLessThan(Math.max(early * 3, early + 2));
  });
});
