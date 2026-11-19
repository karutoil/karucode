import type { JSX } from 'preact';
import { memo } from 'preact/compat';
import { clockTime } from '../../lib/format';
import type { MessageItem } from '../../state/chat';
import { validAt } from './entries';
import { RewindButton } from './RewindButton';
import { rowPropsEqual, type ChatTranscriptProps } from './types';

/** A user message: avatar, author, time and the question in a raised bubble. */
function UserRow({
  item,
  entryKey,
  fresh,
  onRewind,
}: {
  item: MessageItem;
  entryKey: string;
  /** True when this row just joined the transcript, rather than being revealed by scrolling. */
  fresh?: boolean;
  onRewind?: ChatTranscriptProps['onRewind'];
}): JSX.Element {
  return (
    <article
      class={`msg msg-user${fresh ? ' is-fresh' : ''}`}
      aria-label="Your message"
      data-entry-key={entryKey}
    >
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
export const MemoUserRow = memo(UserRow, rowPropsEqual);
