/* Transcript layout model.
 *
 * `ChatTranscript` used to walk `ChatItem[]` and emit one assistant row per turn
 * inline. The view options (group runs of tool calls, fold a finished turn's
 * working steps, show narration differently) turn that walk into a real decision
 * tree, so it lives here: pure functions over `ChatItem[]` that a component can
 * render and a unit test can assert without a DOM.
 *
 * Two ideas drive every rule below:
 *
 * 1. **Never hide what the user must act on.** A running call, a pending
 *    approval, an error, an interrupted run and a notice stay visible whatever
 *    the grouping. Folding is for finished work.
 * 2. **Narration is not noise.** An assistant message followed by more work in
 *    the same turn is the agent saying what it is about to do; the transcript
 *    renders it as a quiet note above the work instead of a full paragraph, so
 *    the answer stands out by contrast rather than by hiding anything.
 */

import type { ChatItem, CommandItem, MessageItem, ToolItem } from './chat';
import type { ThemeGrouping } from '../theme/types';

/** A tool call: both a `tool` card and a shell `command` are calls. */
export type CallItem = ToolItem | CommandItem;

export function isCall(item: ChatItem): item is CallItem {
  return item.kind === 'tool' || item.kind === 'command';
}

/** One user message and everything the assistant did before the next one. */
export interface Turn {
  key: string;
  at: number;
  parts: ChatItem[];
}

/**
 * Split the item list into turns. A date change also starts a new row (the
 * transcript draws a date marker between them) but does not split the turn.
 */
export function splitTurns(items: readonly ChatItem[]): Turn[] {
  const turns: Turn[] = [];
  let turn: Turn | null = null;

  for (const item of items) {
    if (item.kind === 'message' && item.role === 'user') {
      turn = null;
      continue;
    }
    if (!turn) {
      turn = { key: `turn:${item.id}`, at: item.at, parts: [] };
      turns.push(turn);
    }
    turn.parts.push(item);
  }

  return turns;
}

/** True while the turn is still producing: any part is streaming. */
export function turnStreaming(turn: Turn): boolean {
  return turn.parts.some((part) => part.kind === 'message' && part.streaming);
}

export function turnFailed(turn: Turn): boolean {
  return turn.parts.some((part) => part.kind === 'error');
}

export function turnInterrupted(turn: Turn): boolean {
  return turn.parts.some((part) => part.kind === 'interrupted');
}

/** An approval that still needs an answer — always rendered, always visible. */
function isPendingApproval(item: ChatItem): boolean {
  return item.kind === 'approval' && item.approval.status === 'pending';
}

function isRunning(item: ChatItem): boolean {
  return isCall(item) && item.status === 'running';
}

/**
 * True while the turn is doing anything at all: streaming an answer, thinking,
 * or waiting on a call.
 *
 * This is the condition for the **Working** chip and for keeping a turn
 * unfolded. It is deliberately broader than [`turnStreaming`]: a turn that has
 * finished its prose and moved on to a long `run_command` is still working.
 *
 * Throughput is *not* derived from this any more: each block that produces text
 * carries its own badge (`components/TpsBadge`), so the rate sits beside the
 * reasoning, the prose or the call it describes rather than in the turn header.
 */
export function turnWorking(turn: Turn): boolean {
  return turn.parts.some(
    (part) =>
      (part.kind === 'message' && part.streaming) ||
      isRunning(part) ||
      isPendingApproval(part),
  );
}

/**
 * A turn's working steps can only be folded once nothing about them can still
 * change. A streaming answer, a running call or a pending approval keeps the
 * turn open, so the user watches the work as it happens instead of a summary
 * line that hides the step they are waiting on.
 */
export function turnFoldable(turn: Turn): boolean {
  return !turnWorking(turn) && !turnFailed(turn) && !turnInterrupted(turn);
}

/** The part of a turn that is prose the user asked for, not narration. */
function isAnswer(part: ChatItem): part is MessageItem {
  return part.kind === 'message' && part.role === 'assistant' && part.content.trim() !== '';
}

/**
 * The index of the final answer in a turn: the last assistant message that
 * carried content. Everything before it is working material, everything from it
 * on is what the run concluded. `-1` when the turn produced no prose at all
 * (a turn that only ran tools).
 */
export function answerIndex(parts: readonly ChatItem[]): number {
  for (let index = parts.length - 1; index >= 0; index -= 1) {
    const part = parts[index];
    if (part && isAnswer(part)) return index;
  }
  return -1;
}

/**
 * True when an assistant message is narration: there is more work (a call, a
 * command, an approval, an error) after it in the same turn. The agent's "let me
 * check the tests" note is not the answer, even though it is prose.
 */
export function isNarration(parts: readonly ChatItem[], index: number): boolean {
  const part = parts[index];
  if (!part || !isAnswer(part)) return false;
  for (let next = index + 1; next < parts.length; next += 1) {
    const candidate = parts[next];
    if (!candidate) continue;
    if (isCall(candidate)) return true;
    if (candidate.kind === 'approval' || candidate.kind === 'error' || candidate.kind === 'interrupted') {
      return true;
    }
    // Another assistant message with content is the next narration beat or the
    // final answer; either way *this* one is not the last word.
    if (isAnswer(candidate)) return true;
  }
  return false;
}

/** A narration line is one sentence of the agent's plan, collapsed to a line. */
export function firstLine(text: string, max = 160): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  if (flat.length === 0) return '';
  // Prefer ending on a sentence boundary when one arrives early enough: the
  // sentence is complete, so appending an ellipsis there would misrepresent it.
  const stop = /[.!?](\s|$)/.exec(flat);
  if (stop && stop.index > 12 && stop.index < max) return flat.slice(0, stop.index + 1);
  if (flat.length <= max) return flat;
  return `${flat.slice(0, max).trimEnd()}…`;
}

/** One rendered line of a turn: a single part, or a folded run of call parts. */
export type TurnRow =
  | { kind: 'part'; part: ChatItem; index: number; narration: boolean }
  | { kind: 'group'; key: string; parts: CallItem[] };

/**
 * Split a turn into rows, folding runs of consecutive tool calls when grouping
 * asks for it. Calls are only grouped with each other: a message, an approval or
 * a notice between two calls ends the run, so the transcript keeps the order the
 * work actually happened in.
 */
export function turnRows(parts: readonly ChatItem[], grouping: ThemeGrouping): TurnRow[] {
  const groupTools = grouping === 'tools' || grouping === 'turns';
  const rows: TurnRow[] = [];
  let run: CallItem[] = [];

  const flush = (): void => {
    if (run.length === 0) return;
    // A lone call reads better as itself than as a "group of one".
    if (run.length === 1) {
      const only = run[0] as CallItem;
      rows.push({ kind: 'part', part: only, index: parts.indexOf(only), narration: false });
    } else {
      const first = run[0] as CallItem;
      rows.push({ kind: 'group', key: `group:${first.id}`, parts: [...run] });
    }
    run = [];
  };

  parts.forEach((part, index) => {
    if (groupTools && isCall(part)) {
      run.push(part);
      return;
    }
    flush();
    rows.push({ kind: 'part', part, index, narration: isNarration(parts, index) });
  });
  flush();

  return rows;
}

/** Totals for a group's summary line. */
export interface CallTotals {
  count: number;
  ok: number;
  failed: number;
  running: number;
  stopped: number;
  durationMs: number;
  names: string[];
}

export function callTotals(calls: readonly CallItem[]): CallTotals {
  const totals: CallTotals = {
    count: calls.length,
    ok: 0,
    failed: 0,
    running: 0,
    stopped: 0,
    durationMs: 0,
    names: [],
  };
  for (const call of calls) {
    if (call.status === 'ok') totals.ok += 1;
    else if (call.status === 'error') totals.failed += 1;
    else if (call.status === 'running') totals.running += 1;
    else totals.stopped += 1;
    if (typeof call.durationMs === 'number' && Number.isFinite(call.durationMs)) {
      totals.durationMs += call.durationMs;
    }
    const name = call.kind === 'command' ? 'run_command' : call.name;
    if (!totals.names.includes(name)) totals.names.push(name);
  }
  return totals;
}

/** "read_file, grep +3 more" — the group's own idea of what it did. */
export function callsSummary(totals: CallTotals): string {
  const [first, ...rest] = totals.names;
  if (!first) return '';
  if (rest.length === 0) return first;
  return `${first}, ${rest.slice(0, 2).join(', ')}${rest.length > 2 ? ` +${rest.length - 2} more` : ''}`;
}

/** How many calls a set of rows contains, across groups and lone calls. */
export function countCalls(rows: readonly TurnRow[]): number {
  let calls = 0;
  for (const row of rows) {
    if (row.kind === 'group') calls += row.parts.length;
    else if (isCall(row.part)) calls += 1;
  }
  return calls;
}

/** Total measured time of the calls in a set of rows. */
export function countDuration(rows: readonly TurnRow[]): number {
  let durationMs = 0;
  for (const row of rows) {
    if (row.kind === 'group') durationMs += callTotals(row.parts).durationMs;
    else if (isCall(row.part) && typeof row.part.durationMs === 'number') {
      if (Number.isFinite(row.part.durationMs)) durationMs += row.part.durationMs;
    }
  }
  return durationMs;
}

/**
 * The two halves of a folded turn: `hidden` goes behind the summary line and
 * `shown` stays on the page. `pinned` holds the rows that were before the answer
 * yet must not be hidden (a notice, an error, an approval): they are lifted out
 * of the fold and rendered above it, in their original order.
 *
 * Returns `null` when there is nothing worth folding — no final answer to fold
 * towards, or a turn that is only its answer.
 */
export interface FoldedTurn {
  hidden: TurnRow[];
  pinned: TurnRow[];
  shown: TurnRow[];
}

export function foldTurn(rows: readonly TurnRow[]): FoldedTurn | null {
  // The answer is the *last* content message, not the first: every earlier prose
  // row is narration, and picking the first would fold the answer itself away.
  let answerRow = -1;
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const row = rows[index];
    if (row?.kind === 'part' && isAnswer(row.part)) {
      answerRow = index;
      break;
    }
  }
  if (answerRow === -1) return null;

  const before = rows.slice(0, answerRow);
  const shown = rows.slice(answerRow);
  const isPinned = (row: TurnRow): boolean =>
    row.kind === 'part' &&
    (row.part.kind === 'approval' ||
      row.part.kind === 'error' ||
      row.part.kind === 'notice' ||
      row.part.kind === 'interrupted');

  const hidden = before.filter((row) => !isPinned(row));
  if (hidden.length === 0) return null;

  return { hidden, pinned: before.filter(isPinned), shown };
}

