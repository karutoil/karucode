import type { JSX } from 'preact';
import type {
  ApprovalItem,
  ChatItem,
  CommandItem,
  MessageItem,
  ToolItem,
} from '../state/chat';
import type { ApprovalView } from '../state/chat';
import { ApprovalCard } from './ApprovalCard';
import { CommandCard } from './CommandCard';
import { Markdown } from './Markdown';
import { ThinkingBlock } from './ThinkingBlock';
import { ToolCard } from './ToolCard';

export interface ChatTranscriptProps {
  items: ChatItem[];
  resolvingApprovals: Record<string, boolean>;
  onApprovalDecision: (approvalId: string, decision: 'allow_once' | 'allow_session' | 'deny') => void;
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

/**
 * Everything the assistant did between two user messages is one turn in the
 * mock: a single avatar, one author line, then prose, tool cards, diffs and
 * approvals flowing inside the same body column.
 */
interface Turn {
  key: string;
  at: number;
  parts: ChatItem[];
}

function isStreamingPart(part: ChatItem): boolean {
  return part.kind === 'message' && part.streaming;
}

/** The status chip after the author name — the mock's "Completed" tag. */
function turnTag(turn: Turn): JSX.Element | null {
  if (turn.parts.some(isStreamingPart)) {
    return (
      <span class="tag tag-live">
        <span class="tag-dot" aria-hidden="true" />
        Working
      </span>
    );
  }
  if (turn.parts.some((part) => part.kind === 'error')) {
    return <span class="tag tag-failed">Failed</span>;
  }
  if (turn.parts.some((part) => part.kind === 'interrupted')) {
    return <span class="tag tag-stopped">Interrupted</span>;
  }
  return (
    <span class="tag">
      <span class="tag-dot tag-dot-idle" aria-hidden="true" />
      Completed
    </span>
  );
}

function MessageBody({ item }: { item: MessageItem }): JSX.Element {
  return (
    <>
      {item.reasoning ? (
        <ThinkingBlock reasoning={item.reasoning} streaming={item.streaming && item.content === ''} />
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

function TurnPart({
  part,
  onApprovalDecision,
  resolvingApprovals,
}: {
  part: ChatItem;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
}): JSX.Element | null {
  switch (part.kind) {
    case 'message':
      return <MessageBody item={part} />;
    case 'tool':
      return <ToolCard item={part as ToolItem} />;
    case 'command':
      return <CommandCard item={part as CommandItem} defaultOpen={part.status === 'running'} />;
    case 'approval': {
      const approval: ApprovalView = (part as ApprovalItem).approval;
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
          <strong>Interrupted.</strong> {part.message} Send a new message to continue.
        </div>
      );
    case 'error':
      return (
        <div class="run-error" role="alert">
          <p class="run-error-title">Error</p>
          <p class="run-error-message">{part.message}</p>
        </div>
      );
    case 'notice':
      return (
        <p class={`notice notice-${part.level}`} role={part.level === 'error' ? 'alert' : 'status'}>
          {part.message}
        </p>
      );
    default:
      return null;
  }
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

/**
 * The conversation log, laid out like the mock: a date marker, user rows, and
 * one assistant row per turn whose body holds the prose and every card it
 * produced (tool, command, diff-bearing approval, errors).
 */
export function ChatTranscript({
  items,
  resolvingApprovals,
  onApprovalDecision,
}: ChatTranscriptProps): JSX.Element {
  const rows: JSX.Element[] = [];
  let lastDay: number | null = null;
  let turn: Turn | null = null;

  const flushTurn = (): void => {
    if (!turn) return;
    const current = turn;
    turn = null;
    const streaming = current.parts.some(isStreamingPart);
    rows.push(
      <article
        class={`msg msg-assistant${streaming ? ' is-streaming' : ''}`}
        key={current.key}
        aria-label="karucode response"
      >
        <span class="msg-avatar agent" aria-hidden="true">
          k.
        </span>
        <div class="msg-col">
          <div class="msg-top">
            <span class="msg-author">karucode</span>
            {validAt(current.at) ? (
              <time class="msg-time" dateTime={new Date(current.at).toISOString()}>
                {clockTime(current.at)}
              </time>
            ) : null}
            {turnTag(current)}
          </div>
          <div class="msg-body">
            {current.parts.map((part, index) =>
              part.kind === 'message' ? (
                <MessageBody key={part.id ?? index} item={part} />
              ) : (
                <TurnPart
                  key={part.id ?? index}
                  part={part}
                  onApprovalDecision={onApprovalDecision}
                  resolvingApprovals={resolvingApprovals}
                />
              ),
            )}
          </div>
        </div>
      </article>,
    );
  };

  for (const item of items) {
    if (validAt(item.at)) {
      const day = dayStart(item.at);
      if (day !== lastDay) {
        flushTurn();
        lastDay = day;
        rows.push(
          <p class="date-marker" key={`day-${day}`}>
            {dayLabel(item.at)}
          </p>,
        );
      }
    }

    if (item.kind === 'message' && item.role === 'user') {
      flushTurn();
      rows.push(<UserRow item={item} key={item.id} />);
      continue;
    }

    // Everything else belongs to the assistant turn; open one if the run started
    // with a tool call instead of prose.
    if (!turn) turn = { key: `turn:${item.id}`, at: item.at, parts: [] };
    turn.parts.push(item);
  }
  flushTurn();

  return <div class="transcript">{rows}</div>;
}
