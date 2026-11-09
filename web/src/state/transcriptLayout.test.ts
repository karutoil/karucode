import { describe, expect, it } from 'vitest';
import type { ChatItem, CommandItem, MessageItem, ToolItem } from './chat';
import {
  answerIndex,
  callTotals,
  callsSummary,
  countCalls,
  countDuration,
  firstLine,
  foldTurn,
  isCall,
  isNarration,
  splitTurns,
  turnFoldable,
  turnWorking,
  turnRows,
} from './transcriptLayout';
import { NOW } from '../test/fixtures';

let sequence = 0;

function assistant(content: string, overrides: Partial<MessageItem> = {}): MessageItem {
  sequence += 1;
  return {
    kind: 'message',
    id: `m${sequence}`,
    at: NOW,
    role: 'assistant',
    content,
    reasoning: '',
    streaming: false,
    runId: 'r_x',
    agentId: null,
    ...overrides,
  };
}

function user(content = 'do the thing'): MessageItem {
  sequence += 1;
  return assistant(content, { id: `u${sequence}`, role: 'user' });
}

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  sequence += 1;
  return {
    kind: 'tool',
    id: `t${sequence}`,
    at: NOW,
    callId: `call_${sequence}`,
    name: 'read_file',
    args: { path: 'src/main.rs' },
    status: 'ok',
    output: 'fn main() {}',
    truncated: false,
    durationMs: 100,
    runId: 'r_x',
    ...overrides,
  };
}

function command(overrides: Partial<CommandItem> = {}): CommandItem {
  sequence += 1;
  return {
    kind: 'command',
    id: `c${sequence}`,
    at: NOW,
    callId: `cmd_${sequence}`,
    command: 'cargo test',
    cwd: '.',
    stdout: 'ok',
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

describe('splitTurns', () => {
  it('starts a new turn after every user message', () => {
    const turns = splitTurns([user('first'), assistant('a'), user('second'), tool(), assistant('b')]);
    expect(turns.map((turn) => turn.parts.length)).toEqual([1, 2]);
  });

  it('ignores a leading assistant part with no user message before it', () => {
    const turns = splitTurns([tool()]);
    expect(turns).toHaveLength(1);
    expect(turns[0]?.parts).toHaveLength(1);
  });

  it('returns nothing for an empty transcript', () => {
    expect(splitTurns([])).toEqual([]);
  });
});

describe('isCall', () => {
  it('treats tool calls and shell commands alike and prose as prose', () => {
    expect(isCall(tool())).toBe(true);
    expect(isCall(command())).toBe(true);
    expect(isCall(assistant('hi'))).toBe(false);
    expect(isCall({ kind: 'notice', id: 'n', at: NOW, level: 'info', message: 'x' })).toBe(false);
  });
});

describe('isNarration', () => {
  it('marks prose that is followed by more work in the same turn', () => {
    const parts: ChatItem[] = [assistant('Let me read the config.'), tool()];
    expect(isNarration(parts, 0)).toBe(true);
  });

  it('does not mark the final answer', () => {
    const parts: ChatItem[] = [tool(), assistant('The tests pass.')];
    expect(isNarration(parts, 1)).toBe(false);
  });

  it('treats an earlier message as narration when another answer follows it', () => {
    const parts: ChatItem[] = [assistant('One moment.'), assistant('All done.')];
    expect(isNarration(parts, 0)).toBe(true);
    expect(isNarration(parts, 1)).toBe(false);
  });

  it('treats prose followed by an approval or an error as narration', () => {
    const approval: ChatItem = {
      kind: 'approval',
      id: 'ap',
      at: NOW,
      approval: {
        id: 'ap',
        kind: 'shell_command',
        title: 'Run cargo test',
        detail: '',
        payload: null,
        session_id: 's',
        run_id: 'r_x',
        agent_id: null,
        status: 'pending',
        decision: null,
        created_at: NOW,
      },
    };
    const error: ChatItem = {
      kind: 'error',
      id: 'e',
      at: NOW,
      message: 'boom',
      errorKind: 'stream',
      runId: 'r_x',
    };
    expect(isNarration([assistant('I need to run the tests.'), approval], 0)).toBe(true);
    expect(isNarration([assistant('That broke.'), error], 0)).toBe(true);
  });

  it('never marks an empty reasoning-only message', () => {
    const parts: ChatItem[] = [assistant('', { reasoning: 'thinking' }), tool()];
    expect(isNarration(parts, 0)).toBe(false);
  });
});

describe('firstLine', () => {
  it('flattens whitespace and cuts on a sentence boundary', () => {
    expect(firstLine('Reading the file.   Then the tests.')).toBe('Reading the file.');
  });

  it('falls back to a hard cut with an ellipsis', () => {
    const long = 'x'.repeat(300);
    expect(firstLine(long, 40)).toHaveLength(41);
    expect(firstLine(long, 40).endsWith('…')).toBe(true);
  });

  it('returns an empty string for blank prose', () => {
    expect(firstLine('   \n  ')).toBe('');
  });
});

describe('answerIndex', () => {
  it('finds the last assistant message with content', () => {
    const parts: ChatItem[] = [assistant('first'), tool(), assistant('final')];
    expect(answerIndex(parts)).toBe(2);
  });

  it('is -1 for a turn that only ran tools', () => {
    expect(answerIndex([tool(), command()])).toBe(-1);
  });
});

describe('turnFoldable', () => {
  it('refuses to fold a streaming turn', () => {
    const [turn] = splitTurns([user(), assistant('half', { streaming: true })]);
    expect(turn && turnFoldable(turn)).toBe(false);
  });

  it('refuses to fold a turn with a running call', () => {
    const [turn] = splitTurns([user(), tool({ status: 'running' }), assistant('done')]);
    expect(turn && turnFoldable(turn)).toBe(false);
  });

  it('refuses to fold a turn with a pending approval or a failure', () => {
    const pending = splitTurns([
      user(),
      {
        kind: 'approval',
        id: 'ap',
        at: NOW,
        approval: {
          id: 'ap',
          kind: 'shell_command',
          title: 'x',
          detail: '',
          payload: null,
          session_id: 's',
          run_id: 'r_x',
          agent_id: null,
          status: 'pending',
          decision: null,
          created_at: NOW,
        },
      },
    ])[0];
    const failed = splitTurns([
      user(),
      { kind: 'error', id: 'e', at: NOW, message: 'boom', errorKind: null, runId: 'r_x' },
    ])[0];
    expect(pending && turnFoldable(pending)).toBe(false);
    expect(failed && turnFoldable(failed)).toBe(false);
  });

  it('folds a settled turn', () => {
    const [turn] = splitTurns([user(), tool(), assistant('done')]);
    expect(turn && turnFoldable(turn)).toBe(true);
  });
  it('folds a turn whose approval was already answered', () => {
    const [turn] = splitTurns([
      user(),
      {
        kind: 'approval',
        id: 'ap',
        at: NOW,
        approval: {
          id: 'ap',
          kind: 'shell_command',
          title: 'x',
          detail: '',
          payload: null,
          session_id: 's',
          run_id: 'r_x',
          agent_id: null,
          status: 'resolved',
          decision: 'allow_once',
          created_at: NOW,
        },
      },
      assistant('done'),
    ]);
    expect(turn && turnFoldable(turn)).toBe(true);
  });
});

describe('turnWorking', () => {
  it('counts a streaming turn as working', () => {
    const [turn] = splitTurns([user(), assistant('half', { streaming: true })]);
    expect(turn && turnWorking(turn)).toBe(true);
  });

  it('counts a turn with a running call as working, even with no stream', () => {
    // The prose is finished; cargo test is not. A "Completed" chip here was
    // exactly the lie this rule removes.
    const [turn] = splitTurns([user(), assistant('running it'), tool({ status: 'running' })]);
    expect(turn && turnWorking(turn)).toBe(true);
  });

  it('counts a pending approval as working, because the run is blocked on it', () => {
    const [turn] = splitTurns([
      user(),
      {
        kind: 'approval',
        id: 'ap',
        at: NOW,
        approval: {
          id: 'ap',
          kind: 'shell_command',
          title: 'x',
          detail: '',
          payload: null,
          session_id: 's',
          run_id: 'r_x',
          agent_id: null,
          status: 'pending',
          decision: null,
          created_at: NOW,
        },
      },
    ]);
    expect(turn && turnWorking(turn)).toBe(true);
  });

  it('is done once every call has settled', () => {
    const [turn] = splitTurns([user(), assistant('done'), tool()]);
    expect(turn && turnWorking(turn)).toBe(false);
  });
});

describe('turnRows', () => {
  const parts: ChatItem[] = [
    assistant('Reading a few files.'),
    tool({ name: 'read_file' }),
    tool({ name: 'grep' }),
    command(),
    assistant('All good.'),
  ];

  it('keeps every part in order when grouping is off', () => {
    const rows = turnRows(parts, 'none');
    expect(rows).toHaveLength(5);
    expect(rows.every((row) => row.kind === 'part')).toBe(true);
  });

  it('folds a run of consecutive calls when grouping asks for it', () => {
    const rows = turnRows(parts, 'tools');
    expect(rows.map((row) => row.kind)).toEqual(['part', 'group', 'part']);
    const group = rows[1];
    expect(group?.kind === 'group' && group.parts).toHaveLength(3);
  });

  it('never groups a lone call', () => {
    const rows = turnRows([assistant('One check.'), tool()], 'tools');
    expect(rows.map((row) => row.kind)).toEqual(['part', 'part']);
  });

  it('does not group across an approval, notice or message', () => {
    const rows = turnRows(
      [
        tool(),
        tool(),
        { kind: 'notice', id: 'n', at: NOW, level: 'warn', message: 'context is nearly full' },
        tool(),
        tool(),
      ],
      'tools',
    );
    expect(rows.map((row) => row.kind)).toEqual(['group', 'part', 'group']);
  });

  it('marks narration on the rows it returns', () => {
    const rows = turnRows(parts, 'none');
    expect(rows[0]?.kind === 'part' && rows[0].narration).toBe(true);
    const last = rows[rows.length - 1];
    expect(last?.kind === 'part' && last.narration).toBe(false);
  });
});

describe('callTotals', () => {
  it('counts outcomes, adds durations and lists distinct names', () => {
    const totals = callTotals([
      tool({ name: 'read_file', durationMs: 100 }),
      tool({ name: 'read_file', status: 'error', durationMs: 50 }),
      command({ durationMs: 900 }),
      tool({ name: 'grep', status: 'stopped', durationMs: null }),
    ]);
    expect(totals.count).toBe(4);
    expect(totals.ok).toBe(2);
    expect(totals.failed).toBe(1);
    expect(totals.stopped).toBe(1);
    expect(totals.durationMs).toBe(1050);
    expect(totals.names).toEqual(['read_file', 'run_command', 'grep']);
  });

  it('summarises a group by its first few names', () => {
    expect(callsSummary(callTotals([tool({ name: 'read_file' })]))).toBe('read_file');
    expect(
      callsSummary(
        callTotals([
          tool({ name: 'read_file' }),
          tool({ name: 'grep' }),
          tool({ name: 'edit' }),
          tool({ name: 'bash' }),
        ]),
      ),
    ).toBe('read_file, grep, edit +1 more');
  });
});

describe('countCalls and countDuration', () => {
  it('counts calls inside groups and on their own', () => {
    const rows = turnRows([tool(), tool(), assistant('done'), command()], 'tools');
    expect(countCalls(rows)).toBe(3);
    expect(countDuration(rows)).toBe(1100);
  });
});

describe('foldTurn', () => {
  it('hides the working steps and keeps the answer', () => {
    const rows = turnRows([assistant('Working.'), tool(), assistant('Done.')], 'tools');
    const folded = foldTurn(rows);
    expect(folded?.hidden).toHaveLength(2);
    expect(folded?.shown).toHaveLength(1);
    const shown = folded?.shown[0];
    expect(shown?.kind === 'part' && shown.part.kind === 'message' && shown.part.content).toBe('Done.');
    expect(folded?.pinned).toEqual([]);
  });

  it('has nothing to fold when the turn is only an answer', () => {
    const rows = turnRows([assistant('Done.')], 'tools');
    expect(foldTurn(rows)).toBeNull();
  });

  it('has nothing to fold towards when there is no answer', () => {
    const rows = turnRows([tool(), tool()], 'tools');
    expect(foldTurn(rows)).toBeNull();
  });

  it('lifts a notice or error out of the fold instead of hiding it', () => {
    const rows = turnRows(
      [
        tool(),
        { kind: 'notice', id: 'n', at: NOW, level: 'warn', message: 'Context is nearly full.' },
        { kind: 'error', id: 'e', at: NOW, message: 'partial failure', errorKind: null, runId: 'r_x' },
        assistant('I finished what I could.'),
      ],
      'tools',
    );
    const folded = foldTurn(rows);
    // The two non-call rows split the lone call out of a group, so what is hidden
    // is the single call and what is lifted out is the notice and the error.
    expect(folded?.hidden.map((row) => row.kind)).toEqual(['part']);
    expect(folded?.pinned.map((row) => (row.kind === 'part' ? row.part.kind : 'group'))).toEqual([
      'notice',
      'error',
    ]);
    expect(folded?.shown).toHaveLength(1);
  });

  it('hides a group of calls together', () => {
    const rows = turnRows([tool(), tool(), assistant('Done.')], 'tools');
    const folded = foldTurn(rows);
    expect(folded?.hidden.map((row) => row.kind)).toEqual(['group']);
  });

  it('keeps everything after the answer visible', () => {
    const rows = turnRows([tool(), assistant('Answer.'), tool({ name: 'bash' })], 'tools');
    const folded = foldTurn(rows);
    expect(folded?.shown).toHaveLength(2);
    expect(folded?.hidden).toHaveLength(1);
  });
});
