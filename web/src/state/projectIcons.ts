import * as api from '../api/endpoints';
import * as store from './store';

/**
 * Per-project icons for the rail.
 *
 * The bytes come from `GET /api/projects/:id/favicon`, which answers `404` for
 * the common case of a project with no favicon: that project is remembered as
 * "no icon" so the rail draws its initials, and the fetch is not retried on every
 * render.
 *
 * Object URLs have to be revoked by hand, so this module owns them: one entry per
 * project id, replaced (and the old URL revoked) when the icon is refetched,
 * dropped when the project goes away. Fetches are also bounded to one in flight
 * per project, because `refreshProjects` can be called repeatedly while a list
 * response is arriving.
 */

interface IconEntry {
  /** The object URL, or `null` when the project ships no icon (or it failed). */
  url: string | null;
  /** True once the answer is known, so the rail does not fetch it again. */
  settled: boolean;
}

const entries = new Map<string, IconEntry>();
const inFlight = new Map<string, Promise<string | null>>();
/** A fetch that fails for a transport reason is retried, but not on every render. */
const RETRY_AFTER_MS = 60_000;
const failedAt = new Map<string, number>();

function objectUrlsAvailable(): boolean {
  return typeof URL !== 'undefined' && typeof URL.createObjectURL === 'function';
}

/** Publish the cache into the store so the rail re-renders. */
function publish(): void {
  const next: Record<string, string> = {};
  for (const [id, entry] of entries) {
    if (entry.url) next[id] = entry.url;
  }
  const current = store.projectIcons.value;
  // Only replace the signal when something actually changed, so an unrelated
  // refresh does not re-render every project square.
  const same =
    Object.keys(next).length === Object.keys(current).length &&
    Object.entries(next).every(([id, url]) => current[id] === url);
  if (!same) store.projectIcons.value = next;
}

function forget(id: string): void {
  const entry = entries.get(id);
  if (entry?.url) {
    try {
      URL.revokeObjectURL(entry.url);
    } catch {
      // A URL that cannot be revoked is garbage-collected with the page.
    }
  }
  entries.delete(id);
  inFlight.delete(id);
  failedAt.delete(id);
}

/**
 * Load the icon for one project. Resolves to the object URL, or `null` when the
 * project has none. A settled project answers from the cache, so this is safe to
 * call from a render effect.
 */
export async function loadProjectIcon(id: string, force = false): Promise<string | null> {
  if (force) forget(id);
  const settled = entries.get(id);
  if (settled?.settled) return settled.url;
  const pending = inFlight.get(id);
  if (pending) return pending;

  const lastFailure = failedAt.get(id);
  if (lastFailure !== undefined && Date.now() - lastFailure < RETRY_AFTER_MS) return null;

  if (!objectUrlsAvailable()) {
    entries.set(id, { url: null, settled: false });
    return null;
  }

  const promise = (async (): Promise<string | null> => {
    let blob: Blob | null;
    try {
      blob = await api.getProjectIcon(id);
    } catch {
      // A network blip or a 500 is not "this project has no icon": allow a retry
      // later instead of remembering the failure forever.
      failedAt.set(id, Date.now());
      return null;
    }
    if (!blob || blob.size === 0) {
      entries.set(id, { url: null, settled: true });
      publish();
      return null;
    }
    let url: string;
    try {
      url = URL.createObjectURL(blob);
    } catch {
      failedAt.set(id, Date.now());
      return null;
    }
    entries.set(id, { url, settled: true });
    publish();
    return url;
  })().finally(() => inFlight.delete(id));

  inFlight.set(id, promise);
  return promise;
}

/**
 * Load every project's icon, dropping the ones whose project is gone.
 *
 * Called whenever the project list changes. Projects already settled are skipped
 * without a request, so this is cheap on a plain re-render.
 */
export function syncProjectIcons(projects: readonly { id: string }[]): void {
  const known = new Set(projects.map((project) => project.id));
  let dropped = false;
  for (const id of [...entries.keys()]) {
    if (!known.has(id)) {
      forget(id);
      dropped = true;
    }
  }
  if (dropped) publish();
  for (const project of projects) {
    void loadProjectIcon(project.id);
  }
}

/** Test seam: forget every icon and revoke the URLs. */
export function resetProjectIcons(): void {
  for (const id of [...entries.keys()]) forget(id);
  entries.clear();
  inFlight.clear();
  failedAt.clear();
  if (Object.keys(store.projectIcons.value).length > 0) store.projectIcons.value = {};
}
