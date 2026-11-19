import { apiRequest, withQuery } from '../client';
import type {
  QueuedAccepted,
  QueuedAttachment,
  QueuedKind,
  QueuesResponse,
  RecallResponse,
  RewindPreviewResponse,
  RewindRequest,
  RewindResponse,
  SendMessageResponse,
} from '../types/messages';
import type { CallOptions } from './common';

export function sendMessage(
  sessionId: string,
  content: string,
  options: CallOptions = {},
): Promise<SendMessageResponse> {
  return apiRequest<SendMessageResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
    { ...options, method: 'POST', body: { content } },
  );
}

/**
 * Queue a message against the run that is already working.
 *
 * `content` is what the model will see (the draft with one block per attachment,
 * exactly as `sendMessage` would have sent it), and `draft`/`attachments` are what
 * a recall puts back in the composer. The server answers `202` with the entry, so
 * the pending panel can draw it without waiting for the `queued_messages` frame.
 *
 * Both kinds are refused with `409` when no run is in flight: a message with no
 * run to steer *is* a plain send.
 */
export function sendQueuedMessage(
  sessionId: string,
  mode: QueuedKind,
  message: { content: string; draft?: string; attachments?: QueuedAttachment[] },
  options: CallOptions = {},
): Promise<QueuedAccepted> {
  return apiRequest<QueuedAccepted>(
    withQuery(`/api/sessions/${encodeURIComponent(sessionId)}/messages`, { mode }),
    {
      ...options,
      method: 'POST',
      body: {
        content: message.content,
        ...(message.draft !== undefined ? { draft: message.draft } : {}),
        ...(message.attachments && message.attachments.length > 0
          ? { attachments: message.attachments }
          : {}),
      },
    },
  );
}

/** The pending messages for a session, without loading its transcript. */
export function getQueues(sessionId: string, options: CallOptions = {}): Promise<QueuesResponse> {
  return apiRequest<QueuesResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/queues`,
    options,
  );
}

/**
 * Take a queued message back out, so it never reaches the model.
 *
 * The response carries the entry itself: the draft goes back in the composer and
 * the chips are redrawn. A message that already landed is `404` - the caller then
 * refreshes rather than reviving something the run has applied.
 */
export function recallQueued(
  sessionId: string,
  queuedId: string,
  options: CallOptions = {},
): Promise<RecallResponse> {
  return apiRequest<RecallResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/queues/${encodeURIComponent(queuedId)}`,
    { ...options, method: 'DELETE' },
  );
}

export function cancelRun(sessionId: string, options: CallOptions = {}): Promise<{ cancelled: boolean }> {
  return apiRequest<{ cancelled: boolean }>(
    `/api/sessions/${encodeURIComponent(sessionId)}/cancel`,
    { ...options, method: 'POST', body: {} },
  );
}

// ---------------------------------------------------------------------------
// Rewind
// ---------------------------------------------------------------------------

/**
 * What a rewind to `messageId` would do. Safe to call on hover: it hides
 * nothing and writes nothing.
 */
export function previewRewind(
  sessionId: string,
  messageId: number,
  options: CallOptions = {},
): Promise<RewindPreviewResponse> {
  return apiRequest<RewindPreviewResponse>(
    withQuery(`/api/sessions/${encodeURIComponent(sessionId)}/rewind`, {
      message: String(messageId),
    }),
    options,
  );
}

/**
 * Rewind a session to `messageId`.
 *
 * `mode: 'none'` hides the tail and stops (the caller then edits the prompt in
 * the composer); `retry` re-sends the original prompt; `note` sends `note`
 * instead. Both of the latter hide the chosen prompt as well, so the
 * replacement does not follow the request it replaces.
 */
export function rewindSession(
  sessionId: string,
  request: RewindRequest,
  options: CallOptions = {},
): Promise<RewindResponse> {
  return apiRequest<RewindResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/rewind`,
    { ...options, method: 'POST', body: request },
  );
}
