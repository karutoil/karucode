import type { JSX } from 'preact';
import { memo } from 'preact/compat';
import { SparkIcon } from '../../components/icons';
import type { MessageItem } from '../../state/chat';
import { turnWorking } from '../../state/transcriptLayout';
import { MemoAssistantTurn } from './AssistantTurn';
import { isStreamingPart, TurnHeader } from './TurnHeader';
import { rowPropsEqual, type ChatTranscriptProps, type Entry, type ViewOptions } from './types';

/**
 * One assistant row: avatar, author line (live state, time, rewind) and the
 * turn body.
 *
 * The row's header says *whether* the turn is still working (the chip) and keeps
 * nothing else live: throughput belongs to the block that is producing text, so
 * a reasoning block, a streaming message and a running call each carry their own
 * badge (see `TpsBadge`). A single turn-level readout sat above the whole turn,
 * where a long run scrolled it out of view.
 *
 * Memoised so the header work (`turnTag`, `turnWorking`) and the
 * `<AssistantTurn>` beneath it are skipped for every turn that did not change.
 * The transcript itself only has to map over `entries` and hand each one to this
 * boundary.
 */
function AssistantRow({
  entry,
  fresh,
  options,
  onApprovalDecision,
  resolvingApprovals,
  onRewind,
}: {
  entry: Extract<Entry, { kind: 'turn' }>;
  /** True when this turn just joined the transcript, rather than being revealed by scrolling. */
  fresh?: boolean;
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
  // A turn that only called tools (no prose at all) has no answer to rewind to,
  // so the row header would carry nothing. Each stored call then offers its own
  // rewind (`ToolRewind`), which is the same cut: this turn's messages.
  const working = turnWorking(turn);
  return (
    <article
      class={`msg msg-assistant${streaming ? ' is-streaming' : ''}${fresh ? ' is-fresh' : ''}`}
      aria-label="karucode response"
      data-entry-key={entry.key}
    >
      <span class="msg-avatar agent" aria-hidden="true">
        <SparkIcon size={12} />
      </span>
      <div class="msg-col">
        <TurnHeader
          at={entry.at}
          working={working}
          parts={entry.parts}
          answer={answer}
          onRewind={onRewind}
        />
        <MemoAssistantTurn
          turnKey={entry.key}
          parts={entry.parts}
          options={options}
          onApprovalDecision={onApprovalDecision}
          resolvingApprovals={resolvingApprovals}
          onRewind={onRewind}
        />
      </div>
    </article>
  );
}

export const MemoAssistantRow = memo(AssistantRow, rowPropsEqual);
