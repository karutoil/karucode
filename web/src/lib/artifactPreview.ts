import { getAuthToken } from '../api/client';
import { artifactFrameUrl, createArtifact } from '../api/endpoints';

/**
 * The browser half of chat artifacts: publishing a document and remembering the
 * URL it was published under.
 *
 * The server keeps artifacts content-addressed (`POST /api/artifacts` returns the
 * same id for the same bytes), but a card must not post on every render just
 * because the store is idempotent: it would be a request per keystroke of the
 * message being streamed. So the id is remembered per `(source, theme)` here, and
 * the in-flight promise too, which is what stops two cards showing the same mock
 * from racing each other with two identical posts.
 *
 * The cache is bounded and deliberately lossy. An entry that is gone costs one
 * round trip, and a server restart empties its side while this side still has
 * ids - so a frame that 404s is not an error the reader has to act on: the card
 * posts again (see `ArtifactCard`'s reload path).
 */

/** How many `(source, theme)` → id pairs to remember. */
const MAX_CACHED = 128;

const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();

export type ArtifactThemeName = 'light' | 'dark';

function key(source: string, theme: ArtifactThemeName): string {
  // A separator that cannot appear in the theme name, so no source can collide
  // with another theme's key.
  return `${theme}\u0000${source}`;
}

function remember(map: Map<string, string>, lookupKey: string, id: string): void {
  map.set(lookupKey, id);
  while (map.size > MAX_CACHED) {
    const oldest = map.keys().next();
    if (oldest.done) break;
    map.delete(oldest.value);
  }
}

/**
 * Publish one artifact (or return the id it already has) and resolve to the URL
 * an `<iframe>` should load.
 */
export async function publishArtifact(
  source: string,
  theme: ArtifactThemeName,
): Promise<string> {
  const lookupKey = key(source, theme);
  const cached = cache.get(lookupKey);
  if (cached !== undefined) return cached;
  const pending = inflight.get(lookupKey);
  if (pending !== undefined) return pending;

  const task = createArtifact({ html: source, theme })
    .then((response) => {
      remember(cache, lookupKey, response.id);
      return response.id;
    })
    .finally(() => {
      inflight.delete(lookupKey);
    });
  inflight.set(lookupKey, task);
  return task;
}

/** The framed URL for a published artifact, with the bearer token when one is set. */
export function artifactUrl(id: string): string {
  return artifactFrameUrl(id, getAuthToken());
}

/** Drop everything remembered. Used by the tests, which must not leak state. */
export function resetArtifactCache(): void {
  cache.clear();
  inflight.clear();
}
