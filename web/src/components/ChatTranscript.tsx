import type { JSX } from 'preact';
import { memo } from 'preact/compat';
import { useMemo, useRef, useState } from 'preact/hooks';
import type {
  ApprovalItem,
  ChatItem,
  CommandItem,
  MessageItem,
  ToolItem,
} from '../state/chat';
import type { ApprovalView } from '../state/chat';
import {
  countCalls,
  countDuration,
  foldTurn,
  turnFoldable,
  turnOutputChars,
  turnRows,
  turnWorking,
  type Turn,
  type TurnRow,
} from '../state/transcriptLayout';
import { formatDuration } from '../lib/format';
import * as store from '../state/store';
import { activeTheme, type ThemeGrouping, type ThemeThinking, type ThemeToolStyle } from '../theme';
import { ApprovalCard } from './ApprovalCard';
import { CommandCard } from './CommandCard';
import { Markdown } from './Markdown';
import { ThinkingBlock } from './ThinkingBlock';
import { ToolCard } from './ToolCard';
import { ToolGroup } from './ToolGroup';
import { TpsBadge } from './TpsBadge';
import { ChevronDownIcon, RewindIcon, SparkIcon } from './icons';

export interface ChatTranscriptProps {
  items: ChatItem[];
  resolvingApprovals: Record<string, boolean>;
  onApprovalDecision: (approvalId: string, decision: 'allow_once' | 'allow_session' | 'deny') => void;
  /** Open the rewind dialog for one stored message. */
  onRewind?: (target: store.RewindTarget) => void;
}

/** The transcript view options, read once per render from the theme. */
interface ViewOptions {
  toolStyle: ThemeToolStyle;
  grouping: ThemeGrouping;
  thinking: ThemeThinking;
}

/**
 * How the memo boundaries compare.
 *
 * A custom comparer is needed for two reasons. Preact 10's `memo` defaults to
 * `shallowDiffers(props, nextProps)` over the *whole* props object, and
 * `ChatTranscript` used to hand down a fresh inline arrow (`onApprovalDecision`)
 * on every render, so every row compared unequal and no boundary ever held. The
 * callbacks are props of the transcript, not of the row, so they are excluded
 * deliberately: each reads only its own arguments (`resolveApproval(id,
 * decision)`, `openRewind(target)`) and closes over no render state, so a stale
 * one is harmless. `ChatView` also memoises them now, so this is belt and
 * braces.
 *
 * Everything else compares by `Object.is`, which is the point of the exercise: a
 * row's `item`, `parts`, `options` and `resolvingApprovals` are stable by
 * identity unless that row changed, so an unchanged row does not re-render when
 * another one streams. The side benefit over the default `shallowDiffers` is
 * that `children` is not compared at all.
 */
function rowPropsEqual(prev: object, next: object): boolean {
  const before = prev as Record<string, unknown>;
  const after = next as Record<string, unknown>;
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  for (const key of keys) {
    if (key === 'onApprovalDecision' || key === 'onRewind' || key === 'onDecide') continue;
    if (!Object.is(before[key], after[key])) return false;
  }
  return true;
}

const DAY_MS = 86_400_000;

/**
 * The stored message id behind a transcript item, or `null`.
 *
 * Items carry ids like `msg:42` (a persisted row), `tool:call_1` (a live tool
 * call) and `local:…` (an optimistic send). Only a persisted row can be a rewind
 * point, so everything else is refused rather than guessed at.
 */
export function storedMessageId(id: string): number | null {
  if (!id.startsWith('msg:')) return null;
  const parsed = Number(id.slice(4));
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

function validAt(at: number): boolean {
  return Number.isFinite(at) && at > 0;
}

/** Local wall-clock label, e.g. "12:14 PM". Empty when the item has no timestamp. */
function clockTime(at: number): string {
  if (!validAt(at)) return '';
  return new Date(at).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

function dayStart(at: number): number {
  const date = new Date(at);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** "Today · October 28, 2026", "Yesterday · …", or the plain date. */
function dayLabel(at: number, now: number = Date.now()): string {
  const date = new Date(at);
  const pretty = date.toLocaleDateString(undefined, {
    month: 'long',
    day: 'numeric',
    year: 'numeric',
  });
  const day = dayStart(at);
  if (day === dayStart(now)) return `Today · ${pretty}`;
  if (day === dayStart(now - DAY_MS)) return `Yesterday · ${pretty}`;
  return pretty;
}

function isStreamingPart(part: ChatItem): boolean {
  return part.kind === 'message' && part.streaming;
}

/**
 * The status chip after the author name. Ordinary completion needs no badge;
 * only live, failed and interrupted turns add a state to the reading flow.
 *
 * It uses the same "is this turn still doing something" test as the throughput
 * badge and the fold: a turn whose prose has finished but whose tool call is
 * still running is *working*, not completed.
 */
function turnTag(parts: readonly ChatItem[]): JSX.Element | null {
  const turn: Turn = { key: 'tag', at: 0, parts: [...parts] };
  if (turnWorking(turn)) {
    return (
      <span class="tag tag-live">
        <span class="tag-dot" aria-hidden="true" />
        Working
      </span>
    );
  }
  if (parts.some((part) => part.kind === 'error')) {
    return <span class="tag tag-failed">Failed</span>;
  }
  if (parts.some((part) => part.kind === 'interrupted')) {
    return <span class="tag tag-stopped">Interrupted</span>;
  }
  return null;
}

function MessageBodyImpl({ item, options }: { item: MessageItem; options: ViewOptions }): JSX.Element {
  return (
    <>
      {item.reasoning ? (
        <ThinkingBlock
          reasoning={item.reasoning}
          streaming={item.streaming && item.content === ''}
          mode={options.thinking}
        />
      ) : null}
      {/* A streaming message renders no artifact cards: a fence is content only
          once it is closed, and markdown-it closes one at the end of its input,
          so every token of a half-written mock would paint and relayout a frame. */}
      {item.content ? <Markdown text={item.content} artifacts={!item.streaming} /> : null}
      {item.streaming && item.content !== '' ? (
        <span class="stream-caret" aria-hidden="true" />
      ) : null}
      {item.streaming && item.content === '' && item.reasoning === '' ? (
        <p class="thinking-placeholder" aria-hidden="true">
          Thinking
          <span class="thinking-dots">
            <span>.</span>
            <span>.</span>
            <span>.</span>
          </span>
        </p>
      ) : null}
    </>
  );
}

/**
 * One assistant message's prose and reasoning.
 *
 * Memoised: every completed row's `item` object is kept by identity by the
 * reducer, so a boundary here is what stops the whole transcript (Markdown
 * parsing included) from being re-rendered when the last item streams a token.
 */
const MessageBody = memo(MessageBodyImpl, rowPropsEqual);

interface PartProps {
  item: ChatItem;
  /** Prose that is followed by more work is the agent narrating, not answering. */
  narration: boolean;
  options: ViewOptions;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
}

function TurnPart({
  item,
  narration,
  options,
  onApprovalDecision,
  resolvingApprovals,
}: PartProps): JSX.Element | null {
  switch (item.kind) {
    case 'message':
      return (
        <div class={narration ? 'narration' : undefined}>
          <MessageBody item={item} options={options} />
        </div>
      );
    case 'tool':
      return <ToolCard item={item as ToolItem} variant={options.toolStyle === 'rows' ? 'row' : 'card'} />;
    case 'command':
      return (
        <CommandCard
          item={item as CommandItem}
          defaultOpen={item.status === 'running'}
          variant={options.toolStyle === 'rows' ? 'row' : 'card'}
        />
      );
    case 'approval': {
      const approval: ApprovalView = (item as ApprovalItem).approval;
      return (
        <ApprovalCard
          approval={approval}
          resolving={Boolean(resolvingApprovals[approval.id])}
          onDecide={(decision) => onApprovalDecision(approval.id, decision)}
        />
      );
    }
    case 'interrupted':
      return (
        <div class="notice notice-interrupted" role="status">
          <strong>Interrupted.</strong> {item.message} Send a new message to continue.
        </div>
      );
    case 'error':
      return (
        <div class="run-error" role="alert">
          <p class="run-error-title">Error</p>
          <p class="run-error-message">{item.message}</p>
        </div>
      );
    case 'notice':
      return (
        <p class={`notice notice-${item.level}`} role={item.level === 'error' ? 'alert' : 'status'}>
          {item.message}
        </p>
      );
    default:
      return null;
  }
}

/**
 * One part of a turn (prose, a tool card, a command, an approval, a notice).
 *
 * Memoised for the same reason as `MessageBody`: a turn's `parts` array is
 * rebuilt by the reducer only when that turn changes, so every other turn's
 * parts keep their identity and can be skipped.
 */
const MemoTurnPart = memo(TurnPart, rowPropsEqual);

/** One rendered row: a single part, or a run of calls folded into a group. */
function Row({
  row,
  options,
  onApprovalDecision,
  resolvingApprovals,
}: {
  row: TurnRow;
  options: ViewOptions;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
}): JSX.Element {
  if (row.kind === 'group') {
    return (
      <ToolGroup calls={row.parts} variant={options.toolStyle === 'rows' ? 'row' : 'card'} />
    );
  }
  return (
    <MemoTurnPart
      item={row.part}
      narration={row.narration}
      options={options}
      onApprovalDecision={onApprovalDecision}
      resolvingApprovals={resolvingApprovals}
    />
  ) as JSX.Element;
}

/**
 * One rendered row: a single part, or a run of calls folded into a group.
 *
 * The outermost boundary: `row` is what `turnRows` produced, so it keeps its
 * identity for a turn that did not change.
 */
const MemoRow = memo(Row, rowPropsEqual);

/**
 * The disclosure that stands in for a finished turn's working steps. It names
 * what is behind it, so folding the work away does not hide *what* the agent did
 * — only how long the cards are.
 */
function ProcessFold({
  open,
  calls,
  durationMs,
  onToggle,
  id,
}: {
  open: boolean;
  calls: number;
  durationMs: number;
  onToggle: () => void;
  id: string;
}): JSX.Element {
  const duration = formatDuration(durationMs);
  const work = calls === 1 ? '1 tool call' : `${calls} tool calls`;
  return (
    <button
      type="button"
      class="process-fold"
      aria-expanded={open}
      aria-controls={id}
      onClick={onToggle}
    >
      <span class="process-fold-label">{open ? 'Hide the working' : 'Show the working'}</span>
      <span class="process-fold-meta">
        {calls > 0 ? work : 'working notes'}
        {duration ? ` · ${duration}` : ''}
      </span>
      <span class="disclosure" aria-hidden="true">
        <ChevronDownIcon size={12} />
      </span>
    </button>
  );
}

/**
 * One assistant turn: prose, tool calls, commands, approvals and notices in the
 * order they happened, laid out according to the theme's transcript options.
 *
 * The fold is per turn (this component owns its own `open` state), so opening one
 * turn's working steps leaves the others alone.
 */
function AssistantTurn({
  turnKey,
  parts,
  options,
  onApprovalDecision,
  resolvingApprovals,
}: {
  turnKey: string;
  parts: ChatItem[];
  options: ViewOptions;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
}): JSX.Element {
  const rows = useMemo(() => turnRows(parts, options.grouping), [parts, options.grouping]);
  const [processOpen, setProcessOpen] = useState(false);
  const bodyId = `turn-body-${turnKey}`;

  // Folding a whole turn only applies to `turns` grouping, and only once the turn
  // has settled (`turnFoldable` keeps running work, approvals and failures open).
  const fold =
    options.grouping === 'turns' && turnFoldable({ key: turnKey, at: 0, parts })
      ? foldTurn(rows)
      : null;

  const renderRow = (row: TurnRow): JSX.Element => (
    <MemoRow
      key={row.kind === 'group' ? row.key : `part:${row.part.id}`}
      row={row}
      options={options}
      onApprovalDecision={onApprovalDecision}
      resolvingApprovals={resolvingApprovals}
    />
  );

  let body: JSX.Element[];
  if (fold && !processOpen) {
    body = [
      // Anything the reader must act on was lifted out of the fold rather than
      // hidden behind it, and keeps its place above the summary.
      ...fold.pinned.map(renderRow),
      <ProcessFold
        key="fold"
        id={bodyId}
        open={false}
        calls={countCalls(fold.hidden)}
        durationMs={countDuration(fold.hidden)}
        onToggle={() => setProcessOpen(true)}
      />,
      ...fold.shown.map(renderRow),
    ];
  } else {
    body = [
      ...(fold ? [
        <ProcessFold
          key="fold"
          id={bodyId}
          open
          calls={countCalls(fold.hidden)}
          durationMs={countDuration(fold.hidden)}
          onToggle={() => setProcessOpen(false)}
        />,
      ] : []),
      ...rows.map(renderRow),
    ];
  }

  return (
    <div class={fold && !processOpen ? 'msg-body is-folded' : 'msg-body'} id={bodyId}>
      {body}
    </div>
  );
}

/**
 * One assistant turn.
 *
 * Memoised: `parts` is the reducer's array for that turn and keeps its identity
 * across a new item in a *different* turn, so an older turn is skipped entirely
 * (no `turnRows`, no `turnFoldable`, no fold recomputation) while the current
 * one streams. Without this boundary every keystroke of the agent's answer
 * re-laid out the whole conversation.
 */
const MemoAssistantTurn = memo(AssistantTurn, rowPropsEqual);

/** A user message: avatar, author, time and the question in a raised bubble. */
function UserRow({
  item,
  onRewind,
}: {
  item: MessageItem;
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element {
  return (
    <article class="msg msg-user" aria-label="Your message">
      <span class="msg-avatar" aria-hidden="true">
        Y
      </span>
      <div class="msg-col">
        <div class="msg-top">
          <span class="msg-author">You</span>
          {validAt(item.at) ? (
            <time class="msg-time" dateTime={new Date(item.at).toISOString()}>
              {clockTime(item.at)}
            </time>
          ) : null}
          <RewindButton item={item} isAssistant={false} onRewind={onRewind} />
        </div>
        <div class="msg-body user-question">
          <div class="user-text">{item.content}</div>
        </div>
      </div>
    </article>
  );
}

/** A user message, memoised: a settled question never changes. */
const MemoUserRow = memo(UserRow, rowPropsEqual);

/**
 * "Recall to here" for one message.
 *
 * Shown for a persisted row only: a live tool call or an optimistic send has no
 * message id the server could resolve, and offering the button would produce a
 * 404 the reader cannot act on. It is also hidden while a run is in flight,
 * because the server refuses to rewind a session that is still writing.
 */
function RewindButton({
  item,
  isAssistant,
  onRewind,
}: {
  item: MessageItem;
  isAssistant: boolean;
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element | null {
  if (!onRewind) return null;
  const id = storedMessageId(item.id);
  if (id === null) return null;
  if (store.chat.value.running) return null;
  return (
    <button
      type="button"
      class="rewind-button"
      title={isAssistant ? 'Rewind to the request that produced this' : 'Rewind to this message'}
      aria-label={
        isAssistant ? 'Rewind to the request that produced this answer' : 'Rewind to this message'
      }
      onClick={() => onRewind({ messageId: id, preview: item.content, isAssistant })}
    >
      <RewindIcon size={13} />
      <span>Rewind</span>
    </button>
  );
}

/** The rows the transcript renders, in order: date markers, user rows, turns. */
type Entry =
  | { kind: 'day'; key: string; at: number }
  | { kind: 'user'; key: string; item: MessageItem }
  | { kind: 'turn'; key: string; at: number; parts: ChatItem[] };

/** True when two part lists hold the same items, in the same order, by identity. */
function sameParts(a: readonly ChatItem[], b: readonly ChatItem[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let index = 0; index < a.length; index += 1) if (a[index] !== b[index]) return false;
  return true;
}

/**
 * Group items into the rows the transcript draws.
 *
 * `previous` is the entry list from the last render. A row whose items are
 * unchanged is **reused by identity** rather than rebuilt, because every memo
 * boundary downstream compares `entry`, `parts` and `item` by `Object.is`: a
 * freshly built `parts` array for a turn that did not change would make that
 * turn's boundary fail on every keystroke of another turn's answer, which is
 * exactly the cost this is here to avoid. The reducer already keeps settled
 * `ChatItem`s by identity, so identity equality here means "nothing changed".
 */
function buildEntries(items: readonly ChatItem[], previous: readonly Entry[] = []): Entry[] {
  const entries: Entry[] = [];
  let lastDay: number | null = null;
  let turn: { key: string; at: number; parts: ChatItem[] } | null = null;

  const flush = (): void => {
    if (!turn) return;
    const index = entries.length;
    const before = previous[index];
    if (before?.kind === 'turn' && before.key === turn.key && sameParts(before.parts, turn.parts)) {
      entries.push(before);
    } else {
      entries.push({ kind: 'turn', key: turn.key, at: turn.at, parts: turn.parts });
    }
    turn = null;
  };

  for (const item of items) {
    if (validAt(item.at)) {
      const day = dayStart(item.at);
      if (day !== lastDay) {
        flush();
        lastDay = day;
        const index = entries.length;
        const key = `day-${day}`;
        const before = previous[index];
        entries.push(
          before?.kind === 'day' && before.key === key && before.at === item.at
            ? before
            : { kind: 'day', key, at: item.at },
        );
      }
    }

    if (item.kind === 'message' && item.role === 'user') {
      flush();
      const index = entries.length;
      const before = previous[index];
      entries.push(
        before?.kind === 'user' && before.key === item.id && before.item === item
          ? before
          : { kind: 'user', key: item.id, item },
      );
      continue;
    }

    // Everything else belongs to the assistant turn; open one if the run started
    // with a tool call instead of prose.
    if (!turn) turn = { key: `turn:${item.id}`, at: item.at, parts: [] };
    turn.parts.push(item);
  }
  flush();
  return entries;
}

/**
 * One assistant row: avatar, author line (live state, throughput, rewind)
 * and the turn body.
 *
 * Memoised so the header work (`turnTag`, `turnOutputChars`, `turnWorking`) and
 * the `<AssistantTurn>` beneath it are skipped for every turn that did not
 * change. The transcript itself only has to map over `entries` and hand each one
 * to this boundary.
 */
function AssistantRow({
  entry,
  options,
  onApprovalDecision,
  resolvingApprovals,
  onRewind,
}: {
  entry: Extract<Entry, { kind: 'turn' }>;
  options: ViewOptions;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element {
  const streaming = entry.parts.some(isStreamingPart);
  // The same "is this turn still doing something" test the transcript uses
  // to keep a turn unfolded, so the badge and the Working chip agree.
  const turn = { key: entry.key, at: entry.at, parts: entry.parts };
  // The answer a rewound turn produced: the button rides the turn's first
  // prose, which is the message the reader means by "this answer". A turn
  // with no prose (a tool-only turn) has no message id to rewind to, and
  // nesting an optimistic send inside a turn is impossible, so this is
  // always a persisted row when it exists.
  const answer = entry.parts.find(
    (part) => part.kind === 'message' && part.role === 'assistant',
  ) as MessageItem | undefined;
  const working = turnWorking(turn);
  return (
    <article
      class={`msg msg-assistant${streaming ? ' is-streaming' : ''}`}
      aria-label="karucode response"
    >
      <span class="msg-avatar agent" aria-hidden="true">
        <SparkIcon size={12} />
      </span>
      <div class="msg-col">
        <div class="msg-top">
          <span class="msg-author">karucode</span>
          {working && validAt(entry.at) ? (
            <time class="msg-time" dateTime={new Date(entry.at).toISOString()}>
              {clockTime(entry.at)}
            </time>
          ) : null}
          {turnTag(entry.parts)}
          <TpsBadge chars={turnOutputChars(turn)} working={working} />
          {answer ? <RewindButton item={answer} isAssistant onRewind={onRewind} /> : null}
        </div>
        <MemoAssistantTurn
          turnKey={entry.key}
          parts={entry.parts}
          options={options}
          onApprovalDecision={onApprovalDecision}
          resolvingApprovals={resolvingApprovals}
        />
      </div>
    </article>
  );
}

const MemoAssistantRow = memo(AssistantRow, rowPropsEqual);

/**
 * The conversation log, laid out like the mock: a date marker, user rows, and
 * one assistant row per turn whose body holds the prose and every card it
 * produced (tool, command, diff-bearing approval, errors).
 *
 * The theme decides how much of that work is shown at once: `toolStyle` picks
 * cards or one-line rows, `grouping` folds runs of calls and whole turns, and
 * `thinking` decides how much reasoning is visible.
 *
 * Cost shape: the reducer keeps every settled item and every settled turn by
 * identity, so `entries` is rebuilt only when the item list actually changes,
 * `options` only when the theme changes, and each row is behind a memo boundary.
 * A streaming token therefore re-renders the one turn it belongs to instead of
 * the whole conversation (1553 items: 143 ms per rerender before, ~1 ms after).
 * Off-screen rows additionally skip layout and paint - see `content-visibility`
 * on `.msg` in `styles/chat.css`.
 */
export function ChatTranscript({
  items,
  resolvingApprovals,
  onApprovalDecision,
  onRewind,
}: ChatTranscriptProps): JSX.Element {
  const theme = activeTheme.value;
  // Frozen by identity: a new object here every render would make every row's
  // props compare unequal and defeat every boundary below.
  const options = useMemo<ViewOptions>(
    () => ({
      toolStyle: theme.toolStyle,
      grouping: theme.grouping,
      thinking: theme.thinking,
    }),
    [theme.toolStyle, theme.grouping, theme.thinking],
  );
  // Rebuilt only when the item list changes, and reusing the previous entry
  // objects for every row that did not: the memo boundaries below compare by
  // identity, so a fresh `parts` array for an unchanged turn would defeat them.
  const previousEntries = useRef<Entry[]>([]);
  const entries = useMemo(() => {
    const next = buildEntries(items, previousEntries.current);
    previousEntries.current = next;
    return next;
  }, [items]);

  return (
    <div class="transcript">
      {entries.map((entry) => {
        if (entry.kind === 'day') {
          return (
            <p class="date-marker" key={entry.key}>
              {dayLabel(entry.at)}
            </p>
          );
        }
        if (entry.kind === 'user') {
          return <MemoUserRow item={entry.item} key={entry.key} onRewind={onRewind} />;
        }
        return (
          <MemoAssistantRow
            key={entry.key}
            entry={entry}
            options={options}
            onApprovalDecision={onApprovalDecision}
            resolvingApprovals={resolvingApprovals}
            onRewind={onRewind}
          />
        );
      })}
    </div>
  );
}
