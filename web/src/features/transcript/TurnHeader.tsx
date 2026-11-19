import type { JSX } from 'preact';
import { clockTime } from '../../lib/format';
import type { ChatItem, MessageItem } from '../../state/chat';
import { turnWorking, type Turn } from '../../state/transcriptLayout';
import { validAt } from './entries';
import { RewindButton } from './RewindButton';
import type { ChatTranscriptProps } from './types';

export function isStreamingPart(part: ChatItem): boolean {
  return part.kind === 'message' && part.streaming;
}

/**
 * The status chip after the author name. Ordinary completion needs no badge;
 * only live, failed and interrupted turns add a state to the reading flow.
 *
 * It uses the same "is this turn still doing something" test as the fold: a turn
 * whose prose has finished but whose tool call is still running is *working*,
 * not completed.
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

export function TurnHeader({
  at,
  working,
  parts,
  answer,
  onRewind,
}: {
  at: number;
  working: boolean;
  parts: readonly ChatItem[];
  answer?: MessageItem;
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element {
  return (
    <div class="msg-top">
      <span class="msg-author">karucode</span>
      {working && validAt(at) ? (
        <time class="msg-time" dateTime={new Date(at).toISOString()}>
          {clockTime(at)}
        </time>
      ) : null}
      {turnTag(parts)}
      {answer ? <RewindButton item={answer} isAssistant onRewind={onRewind} /> : null}
    </div>
  );
}
