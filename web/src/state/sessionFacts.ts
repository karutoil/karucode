/* Per-session facts the sessions rail shows under each title.
 *
 * The rail's row carries a second line - the session's own "fingerprint" - and
 * `SessionSummary` cannot answer it: the list endpoint returns the title, age,
 * message count and run status, and nothing about what the session *did*. The
 * numbers behind the fingerprint (how many turns, which tools it leaned on) are
 * a per-session aggregate that `GET /api/sessions/:id/info` already computes from
 * persisted rows, so this module reads that rather than inventing a shape.
 *
 * Cost control matters here because a rail can hold seventy sessions and this is
 * the first screen a reader sees:
 *
 * * **Bounded** - at most `MAX_TRACKED` sessions are tracked and at most
 *   `CONCURRENCY` reads are in flight at once, so opening a long rail never
 *   fires seventy requests.
 * * **Coalesced** - one in-flight read per session; asking again while it is
 *   running reuses it instead of starting a second one.
 * * **Cached with a TTL** - a read is reused for `TTL_MS`. The whole record for a
 *   session is dropped when that session's run finishes (`invalidateFacts`), so
 *   the fingerprint follows a live run instead of sticking at its first value.
 * * **Optional** - every consumer treats a missing entry as "not known yet", and
 *   the row simply shows less. A figure the server did not report is never
 *   rendered as a zero.
 */

import { signal } from '@preact/signals';
import { getSessionInfo } from '../api/endpoints';
import type { SessionInfo } from '../api/types';

/** What the rail draws for one session. */
export interface SessionFacts {
  /** User turns: the model saw this many requests. */
  turns: number;
  /** Up to three tool names, most-used first, for the row's glyphs. */
  tools: string[];
  /** Whole-session tool calls, or `null` when the server reported none. */
  calls: number | null;
}

/** Beyond this many sessions the rail shows no fingerprint at all. */
export const MAX_TRACKED = 40;
/** Reads in flight at once. */
export const CONCURRENCY = 4;
/** How long a read is reused before a fresh one is allowed. */
export const TTL_MS = 5 * 60_000;

interface Entry {
  facts: SessionFacts;
  at: number;
}

/** Session id -> facts. Missing means "not read yet". */
export const sessionFacts = signal<Record<string, SessionFacts>>({});

const cache = new Map<string, Entry>();
const inflight = new Map<string, { promise: Promise<SessionFacts | null>; generation: number }>();
const generations = new Map<string, number>();

function generationFor(id: string): number {
  return generations.get(id) ?? 0;
}

/** Tools whose glyph says the most about what a session was: file reads, edits,
 * searches, shell work. Everything else is folded to its generic mark. */
const GLYPH_ORDER = [
  'run_command',
  'edit_file',
  'write_file',
  'read_file',
  'search_files',
  'list_dir',
  'git_diff',
  'git_status',
];

function factsFrom(info: SessionInfo): SessionFacts {
  const turns = info.stats.messages_main_by_role?.user ?? 0;
  const byName = [...(info.tools?.by_name ?? [])].sort((a, b) => b.count - a.count);
  // Prefer the tools that carry meaning, then fill from the most-used rest, so
  // three plugin MCP calls do not crowd out "this session ran commands".
  const ranked = [
    ...GLYPH_ORDER.map((name) => byName.find((tool) => tool.name === name)).filter(
      (tool): tool is { name: string; count: number } => tool !== undefined,
    ),
    ...byName.filter((tool) => !GLYPH_ORDER.includes(tool.name)),
  ];
  return {
    turns,
    tools: ranked.slice(0, 3).map((tool) => tool.name),
    calls: info.stats.tool_calls_total > 0 ? info.stats.tool_calls_total : null,
  };
}

function publish(id: string, facts: SessionFacts): void {
  sessionFacts.value = { ...sessionFacts.value, [id]: facts };
}

/**
 * Read one session's facts, coalescing concurrent asks and honouring the TTL.
 * Resolves to `null` when the read failed; the row then keeps its short form.
 */
export function loadFacts(id: string, options: { force?: boolean } = {}): Promise<SessionFacts | null> {
  const cached = cache.get(id);
  if (!options.force && cached && Date.now() - cached.at < TTL_MS) {
    // Republish so a consumer that mounted after the read still sees it.
    if (!sessionFacts.value[id]) publish(id, cached.facts);
    return Promise.resolve(cached.facts);
  }
  const running = inflight.get(id);
  if (running && running.generation === generationFor(id)) return running.promise;

  const generation = generationFor(id);
  const read = getSessionInfo(id)
    .then((info) => {
      const facts = factsFrom(info);
      // An invalidation may have happened while the request was on the wire.
      // Never let that older response repopulate the cache or repaint the rail.
      if (generation === generationFor(id)) {
        cache.set(id, { facts, at: Date.now() });
        publish(id, facts);
      }
      return facts;
    })
    .catch(() => null)
    .finally(() => {
      if (inflight.get(id)?.promise === read) inflight.delete(id);
    });
  inflight.set(id, { promise: read, generation });
  return read;
}

/**
 * Read facts for a bounded set of sessions, a few at a time.
 *
 * `ids` is expected newest-first: when it is longer than `MAX_TRACKED` the tail
 * is dropped, because those are the rows furthest down the rail.
 */
export async function loadFactsFor(ids: readonly string[]): Promise<void> {
  const wanted = ids.slice(0, MAX_TRACKED).filter((id) => {
    const entry = cache.get(id);
    return !entry || Date.now() - entry.at >= TTL_MS;
  });
  if (wanted.length === 0) return;
  let cursor = 0;
  const worker = async (): Promise<void> => {
    while (cursor < wanted.length) {
      const id = wanted[cursor];
      cursor += 1;
      if (id !== undefined) await loadFacts(id);
    }
  };
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, wanted.length) }, worker));
}

/** Forget one session (its run finished) or everything (a project switch). */
export function invalidateFacts(ids?: readonly string[]): void {
  if (ids === undefined) {
    cache.clear();
    const known = new Set([...inflight.keys(), ...Object.keys(sessionFacts.value)]);
    for (const id of known) generations.set(id, generationFor(id) + 1);
    sessionFacts.value = {};
    return;
  }
  const next = { ...sessionFacts.value };
  for (const id of ids) {
    cache.delete(id);
    generations.set(id, generationFor(id) + 1);
    delete next[id];
  }
  sessionFacts.value = next;
}
