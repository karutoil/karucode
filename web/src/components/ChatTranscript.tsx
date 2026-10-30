import type { JSX } from 'preact';
import { useMemo, useState } from 'preact/hooks';
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
import { activeTheme, type ThemeGrouping, type ThemeThinking, type ThemeToolStyle } from '../theme';
import { ApprovalCard } from './ApprovalCard';
import { CommandCard } from './CommandCard';
import { Markdown } from './Markdown';
import { ThinkingBlock } from './ThinkingBlock';
import { ToolCard } from './ToolCard';
import { ToolGroup } from './ToolGroup';
import { TpsBadge } from './TpsBadge';
import { ChevronDownIcon } from './icons';

export interface ChatTranscriptProps {
  items: ChatItem[];
  resolvingApprovals: Record<string, boolean>;
  onApprovalDecision: (approvalId: string, decision: 'allow_once' | 'allow_session' | 'deny') => void;
}

/** The transcript view options, read once per render from the theme. */
interface ViewOptions {
  toolStyle: ThemeToolStyle;
  grouping: ThemeGrouping;
  thinking: ThemeThinking;
}

const DAY_MS = 86_400_000;

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
 * The status chip after the author name — the mock's "Completed" tag.
 *
 * It uses the same "is this turn still doing something" test as the throughput
 * badge and the fold: a turn whose prose has finished but whose tool call is
 * still running is *working*, not completed, and claiming otherwise put a
 * "Completed" chip beside a live token counter.
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
  return (
    <span class="tag">
      <span class="tag-dot tag-dot-idle" aria-hidden="true" />
      Completed
    </span>
  );
}

function MessageBody({ item, options }: { item: MessageItem; options: ViewOptions }): JSX.Element {
  return (
    <>
      {item.reasoning ? (
        <ThinkingBlock
          reasoning={item.reasoning}
          streaming={item.streaming && item.content === ''}
          mode={options.thinking}
        />
      ) : null}
      {item.content ? <Markdown text={item.content} /> : null}
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
      return <CommandCard item={item as CommandItem} defaultOpen={item.status === 'running'} />;
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
    <TurnPart
      item={row.part}
      narration={row.narration}
      options={options}
      onApprovalDecision={onApprovalDecision}
      resolvingApprovals={resolvingApprovals}
    />
  ) as JSX.Element;
}

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
    <Row
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

/** A user message: avatar, author, time and the question in a raised bubble. */
function UserRow({ item }: { item: MessageItem }): JSX.Element {
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
        </div>
        <div class="msg-body user-question">
          <div class="user-text">{item.content}</div>
        </div>
      </div>
    </article>
  );
}

/** The rows the transcript renders, in order: date markers, user rows, turns. */
type Entry =
  | { kind: 'day'; key: string; at: number }
  | { kind: 'user'; key: string; item: MessageItem }
  | { kind: 'turn'; key: string; at: number; parts: ChatItem[] };

function buildEntries(items: readonly ChatItem[]): Entry[] {
  const entries: Entry[] = [];
  let lastDay: number | null = null;
  let turn: { key: string; at: number; parts: ChatItem[] } | null = null;

  const flush = (): void => {
    if (!turn) return;
    entries.push({ kind: 'turn', key: turn.key, at: turn.at, parts: turn.parts });
    turn = null;
  };

  for (const item of items) {
    if (validAt(item.at)) {
      const day = dayStart(item.at);
      if (day !== lastDay) {
        flush();
        lastDay = day;
        entries.push({ kind: 'day', key: `day-${day}`, at: item.at });
      }
    }

    if (item.kind === 'message' && item.role === 'user') {
      flush();
      entries.push({ kind: 'user', key: item.id, item });
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
 * The conversation log, laid out like the mock: a date marker, user rows, and
 * one assistant row per turn whose body holds the prose and every card it
 * produced (tool, command, diff-bearing approval, errors).
 *
 * The theme decides how much of that work is shown at once: `toolStyle` picks
 * cards or one-line rows, `grouping` folds runs of calls and whole turns, and
 * `thinking` decides how much reasoning is visible.
 */
export function ChatTranscript({
  items,
  resolvingApprovals,
  onApprovalDecision,
}: ChatTranscriptProps): JSX.Element {
  const theme = activeTheme.value;
  const options: ViewOptions = {
    toolStyle: theme.toolStyle,
    grouping: theme.grouping,
    thinking: theme.thinking,
  };
  const entries = buildEntries(items);

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
        if (entry.kind === 'user') return <UserRow item={entry.item} key={entry.key} />;

        const streaming = entry.parts.some(isStreamingPart);
        // The same "is this turn still doing something" test the transcript uses
        // to keep a turn unfolded, so the badge and the Working chip agree.
        const turn = { key: entry.key, at: entry.at, parts: entry.parts };
        return (
          <article
            class={`msg msg-assistant${streaming ? ' is-streaming' : ''}`}
            key={entry.key}
            aria-label="karucode response"
          >
            <span class="msg-avatar agent" aria-hidden="true">
              k.
            </span>
            <div class="msg-col">
              <div class="msg-top">
                <span class="msg-author">karucode</span>
                {validAt(entry.at) ? (
                  <time class="msg-time" dateTime={new Date(entry.at).toISOString()}>
                    {clockTime(entry.at)}
                  </time>
                ) : null}
                {turnTag(entry.parts)}
                <TpsBadge chars={turnOutputChars(turn)} working={turnWorking(turn)} />
              </div>
              <AssistantTurn
                turnKey={entry.key}
                parts={entry.parts}
                options={options}
                onApprovalDecision={onApprovalDecision}
                resolvingApprovals={resolvingApprovals}
              />
            </div>
          </article>
        );
      })}
    </div>
  );
}
