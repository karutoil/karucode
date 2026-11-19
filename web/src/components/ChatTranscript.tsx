import type { JSX } from 'preact';
import { useMemo } from 'preact/hooks';
import { dayLabel } from '../features/transcript/entries';
import { MemoAssistantRow } from '../features/transcript/MessageRow';
import type { ChatTranscriptProps, ViewOptions } from '../features/transcript/types';
import { MemoUserRow } from '../features/transcript/UserMessage';
import { useVirtualWindow } from '../features/transcript/useVirtualWindow';
import { activeTheme } from '../theme';

export { storedMessageId } from '../features/transcript/entries';
export { ToolRewind } from '../features/transcript/RewindButton';
export type { ChatTranscriptProps } from '../features/transcript/types';

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
  viewportRef,
  contentRef,
  sessionKey = null,
  onMeasured,
}: ChatTranscriptProps): JSX.Element {
  const theme = activeTheme.value;
  // Frozen by identity: a new object here every render would make every row's
  // props compare unequal and defeat every boundary below.
  const options = useMemo<ViewOptions>(
    () => ({
      toolStyle: theme.toolStyle,
      grouping: theme.grouping,
      working: theme.working,
      thinking: theme.thinking,
    }),
    [theme.toolStyle, theme.grouping, theme.working, theme.thinking],
  );

  const { transcriptRef, virtual, pending, topPadding, bottomPadding, visible, freshVisible } =
    useVirtualWindow({
      items,
      viewportRef,
      contentRef,
      sessionKey,
      onMeasured,
    });

  return (
    <div
      class={virtual || pending ? 'transcript is-windowed' : 'transcript'}
      ref={transcriptRef}
      style={
        topPadding > 0 || bottomPadding > 0
          ? { paddingTop: `${topPadding}px`, paddingBottom: `${bottomPadding}px` }
          : undefined
      }
    >
      {visible.map((entry) => {
        if (entry.kind === 'day') {
          return (
            <p class="date-marker" key={entry.key} data-entry-key={entry.key}>
              {dayLabel(entry.at)}
            </p>
          );
        }
        if (entry.kind === 'user') {
          return (
            <MemoUserRow
              item={entry.item}
              entryKey={entry.key}
              fresh={freshVisible.has(entry.key)}
              key={entry.key}
              onRewind={onRewind}
            />
          );
        }
        return (
          <MemoAssistantRow
            key={entry.key}
            entry={entry}
            fresh={freshVisible.has(entry.key)}
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
