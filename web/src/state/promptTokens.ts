/* Last reported prompt size, per session, across page loads.
 *
 * `usage` arrives once per model call and is deliberately *not* persisted by the
 * server: it is a live metric, not transcript. That is right for the cost HUD,
 * but it left the composer's context pill blank until the next call finished —
 * on a reload, "how full is this conversation" answered "unknown" until the user
 * happened to send something.
 *
 * The number kept here is the provider's own last `input_tokens` for that
 * session, so restoring it is restoring a fact, not inventing an estimate. It is
 * overwritten by the next call. A miss is a legitimate answer (a session that
 * never ran on this browser), which is why every caller treats it as optional.
 */

const STORAGE_KEY = 'karucode.lastPromptTokens';
/** Sessions remembered at once; older ones fall off. Above the rail's own list. */
const MAX_ENTRIES = 50;

type Store = Record<string, number>;

function read(): Store {
  try {
    if (typeof localStorage === 'undefined') return {};
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return {};
    const out: Store = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof value === 'number' && Number.isFinite(value) && value > 0) out[key] = value;
    }
    return out;
  } catch {
    // Corrupt or disabled storage is not an error: the pill just stays hidden.
    return {};
  }
}

function write(store: Store): void {
  try {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    // Quota or private mode; the in-memory state still works for this tab.
  }
}

/**
 * Record the prompt size a call just reported.
 *
 * Re-inserted so the entry moves to the end: the cap below then evicts the
 * sessions that have not run for longest.
 */
export function rememberPromptTokens(sessionId: string | null, tokens: number): void {
  if (!sessionId || !Number.isFinite(tokens) || tokens <= 0) return;
  const store = read();
  delete store[sessionId];
  store[sessionId] = Math.round(tokens);
  const keys = Object.keys(store);
  for (const stale of keys.slice(0, Math.max(0, keys.length - MAX_ENTRIES))) {
    delete store[stale];
  }
  write(store);
}

/** The last prompt size reported for a session, or `null` when never seen. */
export function recallPromptTokens(sessionId: string | null): number | null {
  if (!sessionId) return null;
  return read()[sessionId] ?? null;
}

/** Drop a session's entry (it was deleted, or its context was reset). */
export function forgetPromptTokens(sessionId: string | null): void {
  if (!sessionId) return;
  const store = read();
  if (!(sessionId in store)) return;
  delete store[sessionId];
  write(store);
}

/** Clear everything; tests only. */
export function resetPromptTokens(): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Nothing to clear.
  }
}
