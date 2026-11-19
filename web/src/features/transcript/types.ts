import type { RefObject } from 'preact';
import type { ChatItem, MessageItem } from '../../state/chat';
import type * as store from '../../state/store';
import type { ThemeGrouping, ThemeThinking, ThemeToolStyle, ThemeWorking } from '../../theme';

export interface ChatTranscriptProps {
  items: ChatItem[];
  resolvingApprovals: Record<string, boolean>;
  onApprovalDecision: (approvalId: string, decision: 'allow_once' | 'allow_session' | 'deny') => void;
  /** Open the rewind dialog for one stored message. */
  onRewind?: (target: store.RewindTarget) => void;
  /**
   * The scroll container the transcript lives in. When present and laid out,
   * only the rows around the viewport are mounted (see `lib/virtualWindow.ts`);
   * without it — tests, embedded renderers — every entry is rendered as before.
   */
  viewportRef?: RefObject<HTMLElement | null>;
  /**
   * Filled with the transcript element itself, whose top edge is the start of
   * the loaded rows. The history loader anchors on it to reveal a page that
   * arrived while the reader was above every row (`lib/historyLoader.ts`).
   */
  contentRef?: RefObject<HTMLElement | null>;
  /** Called when a measurement changed the rows' geometry, so the follow can re-decide. */
  onMeasured?: () => void;
  /**
   * Which conversation the rows belong to. A change resets the window to the
   * tail before the sticky-bottom pin moves the viewport, so a session opened
   * with a long history never paints the previous session's scroll position.
   */
  sessionKey?: string | null;
}

/** The transcript view options, read once per render from the theme. */
export interface ViewOptions {
  toolStyle: ThemeToolStyle;
  grouping: ThemeGrouping;
  /** Whether a finished turn's working steps start expanded. */
  working: ThemeWorking;
  thinking: ThemeThinking;
}

/** The rows the transcript renders, in order: date markers, user rows, turns. */
export type Entry =
  | { kind: 'day'; key: string; at: number }
  | { kind: 'user'; key: string; item: MessageItem }
  | { kind: 'turn'; key: string; at: number; parts: ChatItem[] };

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
export function rowPropsEqual(prev: object, next: object): boolean {
  const before = prev as Record<string, unknown>;
  const after = next as Record<string, unknown>;
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  for (const key of keys) {
    if (key === 'onApprovalDecision' || key === 'onRewind' || key === 'onDecide') continue;
    if (!Object.is(before[key], after[key])) return false;
  }
  return true;
}
