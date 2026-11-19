import * as api from '../../api/endpoints';
import type { QueuedKind } from '../../api/types';
import { readBytes } from '../../lib/attachments';
import { MENTION_INDEX_DEPTH, type TreeLoader } from '../../lib/mentions';
import { isApplePlatform, modKey } from '../../lib/platform';
import * as store from '../../state/store';
import type { UploadContext } from './types';

export const CHAR_COUNTER_THRESHOLD = 4000;
/** Suggestion index lifetime; the workspace changes under us all the time. */
export const INDEX_TTL_MS = 60_000;
export const MENTION_MENU_ID = 'composer-mention-menu';
export const SLASH_MENU_ID = 'composer-slash-menu';

/**
 * How many parked composer forms this tab keeps.
 *
 * A record holds the draft *and* the attachments, whose inlined file bodies are
 * whole source files, so the map is a window over the sessions a reader
 * alternates between - not a history of every session they have ever opened.
 * 24 covers a rail's worth of switching back and forth.
 */
export const FORM_SCOPE_LIMIT = 24;

/** One-line label for each queue kind, used by the toggle and the rows. */
export const QUEUE_LABEL: Record<QueuedKind, string> = {
  steer: 'Steer',
  follow_up: 'Follow-up',
};

export const QUEUE_TITLE: Record<QueuedKind, string> = {
  steer:
    'Steer: fold this into the turn that is already running. The model sees it at its next step, without losing the work in flight.',
  follow_up:
    'Follow-up: run this as a new turn once the current one finishes. The agent finishes what it is doing first.',
};

/**
 * Bound the parked-scope map, shedding the coldest scope first.
 *
 * `Map` iterates in insertion order and the composer re-inserts a scope every
 * time it draws it, so the first key is the one nothing has touched the
 * longest. `protect` is the scope on screen: it is never evicted, however cold
 * its key looks, because throwing away the box the reader is typing in is worse
 * than keeping one entry too many.
 */
export function trimFormScopes<K, V>(map: Map<K, V>, limit: number, protect: K): void {
  // Shed coldest-first. `map.size <= limit` is the target, so a map of 25 with a
  // limit of 24 sheds one. When the protected scope is the only key left above the
  // limit the walk runs to the end, which is the documented trade: one entry too
  // many beats throwing away the box the reader is typing in. Deleting the entry
  // the iterator is on is well defined - the next `next()` continues from the
  // following key - so the walk does not stop early at the protected one.
  for (const key of map.keys()) {
    if (map.size <= limit) return;
    if (key === protect) continue;
    map.delete(key);
  }
}

export const defaultLoadTree: TreeLoader = (depth = MENTION_INDEX_DEPTH) =>
  api.getWorkspaceTree('.', depth, store.currentProjectId.value);

export const defaultUploadFile = async (
  file: File,
  context: UploadContext,
): Promise<{ path: string }> => {
  if (!context.session) throw new Error('Open a session before attaching files.');
  // The wire stays `application/octet-stream` per the contract; the response's
  // `path` is what the message references (the name may be deduplicated).
  const result = await api.uploadAttachment(
    { name: file.name, data: await readBytes(file) },
    { session: context.session, project: context.project },
  );
  return { path: result.path };
};

export function hasFiles(event: DragEvent): boolean {
  const types = event.dataTransfer?.types;
  if (!types) return false;
  return Array.from(types).includes('Files');
}

export function composerPlaceholder(options: {
  canType: boolean;
  disabledReason: string | null;
  running: boolean;
  narrowViewport: boolean;
  queueMode: QueuedKind;
}): string {
  const { canType, disabledReason, running, narrowViewport, queueMode } = options;
  if (!canType) return disabledReason ?? 'The composer is unavailable.';
  if (running) {
    if (narrowViewport) {
      return queueMode === 'steer' ? 'Steer the run…' : 'Queue a follow-up…';
    }
    return queueMode === 'steer'
      ? 'The agent is working — send a message to steer it at its next step…'
      : 'The agent is working — type the next thing to do once it finishes…';
  }
  return narrowViewport
    ? 'Ask karucode anything…'
    : 'Ask karucode anything, or describe a change...';
}

export function sendChordLabels(enterSends: boolean): {
  sendChord: string;
  sendChordLong: string;
} {
  /** The sending chord, spelled the way the foot hint and the buttons spell it.
   *  The modifier is the platform's own (`⌘` on a Mac, where there is no Ctrl
   *  key): every other hint in the app already reads `modKey()`, and a chord
   *  nobody has on their keyboard is worse than no hint at all. */
  const sendChord = enterSends ? 'Enter' : `${modKey()} Enter`;
  /** The same chord in words, for accessible names and tooltips.
   *  `⌘` is a private-use glyph: several screen readers announce it as nothing
   *  useful, which made the send button's name "Send message (Enter)" on a Mac.
   *  So the spoken form spells the modifier out on the platform's own terms. */
  const sendChordLong = enterSends
    ? 'Enter'
    : isApplePlatform()
      ? 'Command + Enter'
      : 'Control + Enter';
  return { sendChord, sendChordLong };
}
