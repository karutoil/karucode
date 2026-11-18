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

/**
 * How many characters of remembered source may be retained, across both the
 * cache and the invalidation clock.
 *
 * A cache key *is* the artifact source, so each entry costs its own bytes: 128
 * distinct 1 MiB mocks was ~128 MB of strings alive for the whole life of the
 * tab, for URLs that are re-derivable with one round trip. Eight megabytes is
 * about one screenful of large mocks; past it the oldest entries go.
 */
const MAX_CACHED_CHARS = 8 * 1024 * 1024;

/** One cached capability: the frame URL and what it costs to keep. */
const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();
/** Per-key invalidation clock; an older POST must not refill a cleared entry. */
const generations = new Map<string, number>();
/**
 * POSTs that have not settled yet, per key.
 *
 * A generation record may only be dropped while nothing could still be holding
 * an older one: an invalidated POST's `.then` compares the generation it
 * captured against this map, so forgetting the record would read as generation
 * zero and let exactly the stale URL the record exists to reject back in.
 */
const running = new Map<string, number>();

export type ArtifactThemeName = 'light' | 'dark';

function key(source: string, theme: ArtifactThemeName): string {
  // A separator that cannot appear in the theme name, so no source can collide
  // with another theme's key.
  return `${theme}\u0000${source}`;
}

/** Characters a map of source-keyed entries retains. */
function retainedChars(map: Map<string, unknown>, withValues = false): number {
  let total = 0;
  for (const [lookupKey, value] of map) {
    total += lookupKey.length;
    if (withValues) total += (value as string).length;
  }
  return total;
}

/** Drop one cached entry, and the generation that guarded it when nothing can still use it. */
function forget(lookupKey: string): void {
  const entry = cache.get(lookupKey);
  if (entry !== undefined) {
    cache.delete(lookupKey);
    if (!running.has(lookupKey)) generations.delete(lookupKey);
  }
}

/**
 * Trim the invalidation clock to the same budget as the cache, oldest first.
 *
 * It is pruned as a side effect of an entry being evicted (see [`forget`]), and
 * here for sources that were invalidated but never cached - the case that used
 * to grow the map for the life of the tab.
 */
function trimGenerations(): void {
  for (const lookupKey of generations.keys()) {
    if (generations.size <= MAX_CACHED && retainedChars(generations) <= MAX_CACHED_CHARS) break;
    if (running.has(lookupKey)) continue;
    generations.delete(lookupKey);
  }
}

function remember(lookupKey: string, url: string): void {
  cache.set(lookupKey, url);
  // Oldest first, the order the map was filled in, which is the idiom this cache
  // has always used. The newest entry stays even when it alone is over budget:
  // it is the document on screen, and a miss there is a POST nobody asked for.
  while (cache.size > 1) {
    if (cache.size <= MAX_CACHED && retainedChars(cache, true) <= MAX_CACHED_CHARS) break;
    const oldest = cache.keys().next();
    if (oldest.done) break;
    forget(oldest.value);
  }
  trimGenerations();
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
  trimGenerations();
}

/** What the caches are holding, in characters of source and entries. Used by the tests. */
export function artifactCacheStats(): { bytes: number; entries: number; generations: number } {
  return {
    bytes: retainedChars(cache, true) + retainedChars(generations),
    entries: cache.size,
    generations: generations.size,
  };
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
        remember(lookupKey, response.url);
      }
      return response.url;
    })
    .finally(() => {
      if (inflight.get(lookupKey) === task) inflight.delete(lookupKey);
      const still = (running.get(lookupKey) ?? 1) - 1;
      if (still <= 0) running.delete(lookupKey);
      else running.set(lookupKey, still);
    });
  inflight.set(lookupKey, task);
  running.set(lookupKey, (running.get(lookupKey) ?? 0) + 1);
  return task;
}

/** Drop everything remembered. Used by the tests, which must not leak state. */
export function resetArtifactCache(): void {
  cache.clear();
  inflight.clear();
  generations.clear();
  running.clear();
}
