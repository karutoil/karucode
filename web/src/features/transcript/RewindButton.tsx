import type { JSX } from 'preact';
import { RewindIcon } from '../../components/icons';
import type { CommandItem, MessageItem, ToolItem } from '../../state/chat';
import { storedMessageId } from './entries';
import type { ChatTranscriptProps } from './types';

/**
 * "Recall to here" for one message.
 *
 * Shown for a row the server can resolve: a read row (`msg:12`) or a live row
 * that has already learned its stored id (`withStoredMessageIds`). An optimistic
 * send or a placeholder still streaming has neither, and offering the button
 * would produce a 404 the reader cannot act on.
 *
 * It stays on screen during a run even though the server refuses to rewind a
 * session that is still writing: hiding it made the affordance appear and
 * disappear as the run's state changed, and a control that is not there when the
 * reader looks for it reads as a missing feature rather than as a refusal. The
 * click says why in the dialog instead (`actions.openRewind`).
 */
export function RewindButton({
  item,
  isAssistant,
  onRewind,
  class: className,
}: {
  item: MessageItem;
  isAssistant: boolean;
  onRewind?: ChatTranscriptProps['onRewind'];
  /** Extra classes, e.g. the anchored position a tool call puts it in. */
  class?: string;
}): JSX.Element | null {
  if (!onRewind) return null;
  const id = storedMessageId(item.id) ?? item.messageId ?? null;
  if (id === null) return null;
  return (
    <button
      type="button"
      class={className ? `rewind-button ${className}` : 'rewind-button'}
      title={isAssistant ? 'Rewind to the request that produced this' : 'Rewind to this message'}
      aria-label={
        isAssistant ? 'Rewind to the request that produced this answer' : 'Rewind to this message'
      }
      onClick={(e) => {
        e.stopPropagation();
        onRewind({ messageId: id, preview: item.content, isAssistant });
      }}
    >
      <RewindIcon size={13} />
      <span>Rewind</span>
    </button>
  );
}

/**
 * "Recall to here" for one tool call.
 *
 * The call's own message is the tool *result* row, which is a persisted message
 * and therefore a valid rewind point: rewinding to it keeps the call and its
 * output and drops everything the agent did afterwards. Live calls have no row
 * yet (`storedMessageId` refuses them), so the button appears once the call is
 * part of the stored transcript.
 *
 * This is the only control the reader has on a turn that is nothing but calls:
 * such a turn has no prose, so the row header above it carries no rewind button
 * of its own.
 */
export function ToolRewind({
  item,
  onRewind,
}: {
  item: ToolItem | CommandItem;
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element | null {
  if (!onRewind || !item.messageId) return null;
  const at = {
    kind: 'message',
    id: `msg:${item.messageId}`,
    at: item.at,
    role: 'assistant',
    content: '',
    reasoning: '',
    streaming: false,
    runId: item.runId,
    agentId: null,
  } satisfies MessageItem;
  return <RewindButton item={at} isAssistant onRewind={onRewind} class="rewind-tool" />;
}
