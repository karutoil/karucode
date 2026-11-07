import { createArtifact } from '../api/endpoints';

/**
 * The browser half of chat artifacts: publishing a document and remembering the
 * URL it was published under.
 *
 * The server keeps artifacts content-addressed (`POST /api/artifacts` returns the
 * same id for the same bytes) and returns a URL carrying a read capability for
 * that artifact alone, but a card must not post on every render just
 * because the store is idempotent: it would be a request per keystroke of the
 * message being streamed. So the id is remembered per `(source, theme)` here, and
 * the in-flight promise too, which is what stops two cards showing the same mock
 * from racing each other with two identical posts.
 *
 * The cache is bounded and deliberately lossy. An entry that is gone costs one
 * round trip. A server restart can evict its side while this side still has a
 * scoped URL, so an explicit **Render again** or **Try again** invalidates that
 * URL and posts the source again instead of reloading a dead id.
 */

/** How many `(source, theme)` → scoped frame URLs to remember. */
const MAX_CACHED = 128;

const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();
/** Per-key invalidation clock; an older POST must not refill a cleared entry. */
const generations = new Map<string, number>();

export type ArtifactThemeName = 'light' | 'dark';

function key(source: string, theme: ArtifactThemeName): string {
  // A separator that cannot appear in the theme name, so no source can collide
  // with another theme's key.
  return `${theme}\u0000${source}`;
}

function remember(map: Map<string, string>, lookupKey: string, url: string): void {
  map.set(lookupKey, url);
  while (map.size > MAX_CACHED) {
    const oldest = map.keys().next();
    if (oldest.done) break;
    map.delete(oldest.value);
  }
}

/**
 * Forget a possibly server-evicted capability before an explicit retry. An
 * in-flight older request still finishes for its original caller, but cannot put
 * its now-suspect URL back into either cache.
 */
export function invalidateArtifact(source: string, theme: ArtifactThemeName): void {
  const lookupKey = key(source, theme);
  cache.delete(lookupKey);
  inflight.delete(lookupKey);
  generations.set(lookupKey, (generations.get(lookupKey) ?? 0) + 1);
}

/**
 * Publish one artifact (or return its cached capability URL) and resolve to the URL
 * an `<iframe>` should load.
 */
export async function publishArtifact(
  source: string,
  theme: ArtifactThemeName,
): Promise<string> {
  const lookupKey = key(source, theme);
  const generation = generations.get(lookupKey) ?? 0;
  const cached = cache.get(lookupKey);
  if (cached !== undefined) return cached;
  const pending = inflight.get(lookupKey);
  if (pending !== undefined) return pending;

  const task: Promise<string> = createArtifact({ html: source, theme })
    .then((response) => {
      // Use the server-issued URL verbatim. In particular, never reconstruct it
      // with the master bearer: this URL is exposed to the artifact's own JS.
      if ((generations.get(lookupKey) ?? 0) === generation) {
        remember(cache, lookupKey, response.url);
      }
      return response.url;
    })
    .finally(() => {
      if (inflight.get(lookupKey) === task) inflight.delete(lookupKey);
    });
  inflight.set(lookupKey, task);
  return task;
}

/** Drop everything remembered. Used by the tests, which must not leak state. */
export function resetArtifactCache(): void {
  cache.clear();
  inflight.clear();
  generations.clear();
}
