import { apiRequestBlob } from '../api/client';
import { workspaceRawUrl } from '../api/endpoints';

/**
 * Authenticated workspace images, fetched once and shared.
 *
 * An `<img>` is a browser navigation: it cannot send the `Authorization` header,
 * so the bytes are read through the API and the DOM is handed a `blob:` URL.
 * Holding one per element would be enough for a single preview - and that is what
 * the file viewer did - but a chat transcript is windowed: a row that scrolls out
 * is unmounted and remounted on the way back, so a naive per-element fetch would
 * download and decode the same screenshot on every pass over the conversation.
 *
 * So one request per (project, path), kept alive while any element holds it. An
 * idle URL is only revoked when the map grows past `MAX_IDLE_ENTRIES`, which
 * bounds a session's worth of images without touching one that is on screen.
 */

interface HeldImage {
  url: string;
  /** Live holders (mounted elements that have been handed this URL). */
  refs: number;
}

const MAX_IDLE_ENTRIES = 48;

const images = new Map<string, HeldImage>();
const pending = new Map<string, Promise<string>>();

function cacheKey(path: string, project: string | null): string {
  return `${project ?? ''}\u0000${path}`;
}

/** Revoke the oldest idle entries once the cache is over its cap. */
function trim(): void {
  if (images.size <= MAX_IDLE_ENTRIES) return;
  for (const [key, entry] of images) {
    if (images.size <= MAX_IDLE_ENTRIES) break;
    // An entry with holders is on screen (or about to be): revoking its URL would
    // turn a rendered image into a broken one.
    if (entry.refs > 0) continue;
    URL.revokeObjectURL(entry.url);
    images.delete(key);
  }
}

/**
 * Take a reference to one workspace image, fetching it on first use.
 *
 * Every successful call must be paired with `releaseWorkspaceImage`; a caller
 * that lost interest before the bytes arrived releases when the promise settles.
 */
export function holdWorkspaceImage(path: string, project: string | null): Promise<string> {
  const key = cacheKey(path, project);
  const cached = images.get(key);
  if (cached) {
    cached.refs += 1;
    return Promise.resolve(cached.url);
  }
  let request = pending.get(key);
  if (!request) {
    request = apiRequestBlob(workspaceRawUrl(path, project))
      .then((blob) => {
        const url = URL.createObjectURL(blob);
        images.set(key, { url, refs: 0 });
        trim();
        return url;
      })
      .finally(() => {
        pending.delete(key);
      });
    pending.set(key, request);
  }
  return request.then((url) => {
    const entry = images.get(key);
    if (entry) entry.refs += 1;
    return url;
  });
}

/** Drop one reference; the bytes stay cached until the cap evicts them. */
export function releaseWorkspaceImage(path: string, project: string | null): void {
  const entry = images.get(cacheKey(path, project));
  if (!entry) return;
  entry.refs = Math.max(0, entry.refs - 1);
}

/**
 * Forget every cached image.
 *
 * Called by the test harness between cases so one document's object URLs never
 * leak into the next. The app itself has no path that needs it: the editor can
 * only save files the viewer renders as *text*, so nothing this UI writes can
 * change the bytes behind a cached picture. (An agent rewriting a screenshot
 * under a rendered message is the one stale case, and a live blob is a blob -
 * refusing to revoke one under a mounted `<img>` matters more than a repaint
 * nothing triggers.)
 */
export function dropWorkspaceImages(): void {
  for (const entry of images.values()) URL.revokeObjectURL(entry.url);
  images.clear();
  pending.clear();
}

/** Test seam: the number of cached entries. */
export function workspaceImageCacheSize(): number {
  return images.size;
}
