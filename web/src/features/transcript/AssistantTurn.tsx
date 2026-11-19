import type { JSX } from 'preact';
import { memo } from 'preact/compat';
import { useMemo, useState } from 'preact/hooks';
import { ApprovalCard } from '../../components/ApprovalCard';
import { CommandCard } from '../../components/CommandCard';
import { Markdown } from '../../components/Markdown';
import { SlashCard } from '../../components/SlashCard';
import { ThinkingBlock } from '../../components/ThinkingBlock';
import { ToolCard } from '../../components/ToolCard';
import { ToolGroup } from '../../components/ToolGroup';
import { TpsBadge } from '../../components/TpsBadge';
import { ChevronDownIcon } from '../../components/icons';
import { formatDuration } from '../../lib/format';
import type {
  ApprovalItem,
  ApprovalView,
  ChatItem,
  CommandItem,
  MessageItem,
  SlashItem,
  ToolItem,
} from '../../state/chat';
import {
  countCalls,
  countDuration,
  foldTurn,
  turnFoldable,
  turnRows,
  type TurnRow,
} from '../../state/transcriptLayout';
import { InterruptedNotice, RunErrorNotice, SystemNotice } from './SystemNotices';
import { rowPropsEqual, type ChatTranscriptProps, type ViewOptions } from './types';

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
          so every token of a half-written mock would paint and relayout a frame.
          `streaming` also makes the re-render per token proportional to the
          block being written, not to the whole answer (lib/streamingMarkdown). */}
      {item.content ? (
        <Markdown text={item.content} artifacts={!item.streaming} streaming={item.streaming} />
      ) : null}
      {item.streaming && item.content !== '' ? (
        <>
          <span class="stream-caret" aria-hidden="true" />
          {/* Prose that is still arriving carries its own throughput, right
              where the text stops. It goes when this block stops streaming, so
              it cannot outlive the paragraph it describes. */}
          <TpsBadge chars={item.content.length} working source="text" />
        </>
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
  /** Rewind a stored call in this turn; see {@link ToolRewind}. */
  onRewind?: ChatTranscriptProps['onRewind'];
}

function TurnPart({
  item,
  narration,
  options,
  onApprovalDecision,
  resolvingApprovals,
  onRewind,
}: PartProps): JSX.Element | null {
  switch (item.kind) {
    case 'message':
      return (
        <div class={narration ? 'narration' : undefined}>
          <MessageBody item={item} options={options} />
        </div>
      );
    case 'tool':
      return (
        <ToolCard
          item={item as ToolItem}
          variant={options.toolStyle === 'rows' ? 'row' : 'card'}
          onRewind={onRewind}
        />
      );
    case 'command':
      return (
        <CommandCard
          item={item as CommandItem}
          defaultOpen={item.status === 'running'}
          variant={options.toolStyle === 'rows' ? 'row' : 'card'}
          onRewind={onRewind}
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
      return <InterruptedNotice item={item} />;
    case 'error':
      return <RunErrorNotice item={item} />;
    case 'notice':
      return <SystemNotice item={item} />;
    case 'slash':
      return <SlashCard item={item as SlashItem} />;
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
  onRewind,
}: {
  row: TurnRow;
  options: ViewOptions;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element {
  if (row.kind === 'group') {
    return (
      <ToolGroup
        calls={row.parts}
        variant={options.toolStyle === 'rows' ? 'row' : 'card'}
        defaultOpen={options.working === 'open'}
        onRewind={onRewind}
      />
    );
  }
  return (
    <MemoTurnPart
      item={row.part}
      narration={row.narration}
      options={options}
      onApprovalDecision={onApprovalDecision}
      resolvingApprovals={resolvingApprovals}
      onRewind={onRewind}
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
 * The fold is per turn (this component owns its own open state), so opening one
 * turn's working steps leaves the others alone. The theme's `working` option only
 * decides where that state starts — `open` for the built-in themes, `folded` for
 * a stored theme that predates the option.
 */
function AssistantTurn({
  turnKey,
  parts,
  options,
  onApprovalDecision,
  resolvingApprovals,
  onRewind,
}: {
  turnKey: string;
  parts: ChatItem[];
  options: ViewOptions;
  onApprovalDecision: ChatTranscriptProps['onApprovalDecision'];
  resolvingApprovals: ChatTranscriptProps['resolvingApprovals'];
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element {
  const rows = useMemo(() => turnRows(parts, options.grouping), [parts, options.grouping]);
  /**
   * The theme's `working` option says where a finished turn *starts*; a click on
   * this turn's summary is a choice for this turn alone, so opening one turn's
   * working never expands another. `null` means "nobody has chosen here yet", so
   * a later theme change still repaints the turns the reader left alone.
   */
  const [processChoice, setProcessChoice] = useState<boolean | null>(null);
  const processOpen = processChoice ?? (options.working === 'open');
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
      onRewind={onRewind}
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
        onToggle={() => setProcessChoice(true)}
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
          onToggle={() => setProcessChoice(false)}
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
export const MemoAssistantTurn = memo(AssistantTurn, rowPropsEqual);
