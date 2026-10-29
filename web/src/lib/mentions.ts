/**
 * `@`-mentions for the composer.
 *
 * The index is the real workspace listing (`GET /api/workspace/tree?depth=6`),
 * so a mention always names a path the agent can open. Everything here is pure
 * apart from `indexFromTree`, which only reshapes the API response.
 */

import type { WorkspaceTree } from '../api/types';

/** One path the user can reference with `@`. */
export interface MentionCandidate {
  /** Workspace-relative path with forward slashes. */
  path: string;
  kind: 'dir' | 'file';
  size: number;
}

/** Directories worth hiding: noise no one wants to mention. */
export const IGNORED_SEGMENTS: readonly string[] = [
  'node_modules',
  '.git',
  'target',
  'dist',
  '.next',
  '.turbo',
  '.venv',
  'venv',
  '__pycache__',
  '.pytest_cache',
  '.mypy_cache',
  'coverage',
  '.idea',
];

/** Depth used for the mention index; the server clamps to 1..6. */
export const MENTION_INDEX_DEPTH = 6;
/** Upper bound on indexed paths (the server caps a tree at 2000 entries). */
export const MENTION_INDEX_LIMIT = 2000;
/** How many suggestions the menu shows at once. */
export const MENTION_LIMIT = 8;

function isIgnored(path: string): boolean {
  if (path === '' || path === '.') return false;
  const segments = path.toLowerCase().split('/');
  return segments.some((segment) => IGNORED_SEGMENTS.includes(segment));
}

/** Flatten a tree response into mention candidates, dropping noise directories. */
export function indexFromTree(tree: WorkspaceTree): MentionCandidate[] {
  const out: MentionCandidate[] = [];
  for (const entry of tree.entries ?? []) {
    if (out.length >= MENTION_INDEX_LIMIT) break;
    if (!entry || typeof entry.path !== 'string' || entry.path === '') continue;
    const kind: 'dir' | 'file' = entry.kind === 'dir' ? 'dir' : 'file';
    if (isIgnored(entry.path)) continue;
    out.push({ path: entry.path, kind, size: Number.isFinite(entry.size) ? entry.size : 0 });
  }
  return out;
}

function normalise(value: string): string {
  return value.replace(/\\/g, '/').toLowerCase();
}

function basename(path: string): string {
  const index = path.lastIndexOf('/');
  return index === -1 ? path : path.slice(index + 1);
}

/** True when `needle` appears in `haystack` in order (fuzzy file search). */
export function isSubsequence(needle: string, haystack: string): boolean {
  if (needle === '') return true;
  let cursor = 0;
  for (const char of haystack) {
    if (char === needle[cursor]) cursor += 1;
    if (cursor === needle.length) return true;
  }
  return cursor === needle.length;
}

/** Score one candidate against a query; 0 means "no match". */
export function scoreMention(candidate: MentionCandidate, query: string): number {
  const needle = normalise(query.trim());
  const path = normalise(candidate.path);
  const name = basename(path);
  const depth = path.split('/').length;

  // Shallower paths are cheaper to type and easier to read.
  const depthPenalty = Math.min(depth * 2, 12);
  const kindBias = candidate.kind === 'file' ? 1 : 0;
  if (needle === '') return 40 + kindBias - depthPenalty;

  let score = 0;
  if (name === needle) score = 100;
  else if (name.startsWith(needle)) score = 90 - Math.min(name.length - needle.length, 20);
  else if (path.startsWith(needle)) score = 80 - Math.min(path.length - needle.length, 20);
  else if (name.includes(needle)) score = 70;
  else if (path.includes(needle)) score = 60;
  else if (isSubsequence(needle, name)) score = 40;
  else if (isSubsequence(needle, path)) score = 30;
  else return 0;
  return score + kindBias - depthPenalty;
}

/** Rank the index for a query, best first, capped at `limit`. */
export function rankMentions(
  index: readonly MentionCandidate[],
  query: string,
  limit: number = MENTION_LIMIT,
): MentionCandidate[] {
  const scored: { candidate: MentionCandidate; score: number }[] = [];
  for (const candidate of index) {
    const score = scoreMention(candidate, query);
    if (score > 0) scored.push({ candidate, score });
  }
  scored.sort(
    (a, b) => b.score - a.score || a.candidate.path.localeCompare(b.candidate.path),
  );
  return scored.slice(0, Math.max(0, limit)).map((entry) => entry.candidate);
}

/**
 * The `@token` the caret is currently inside, if any.
 *
 * Accepts both `@src/main.ts` and `@"my notes/plan.md"`; the `@` must start a
 * word, so an email address never opens the menu.
 */
export interface ActiveMention {
  /** Index of the `@`. */
  start: number;
  /** Index just past the token (past the closing quote when quoted). */
  end: number;
  /** Text typed after the `@` (or inside the quotes). */
  query: string;
  quoted: boolean;
}

export function activeMention(text: string, caret: number): ActiveMention | null {
  const position = Math.max(0, Math.min(caret, text.length));
  const at = text.lastIndexOf('@', position - 1);
  if (at === -1) return null;
  if (at > 0) {
    const before = text[at - 1] as string;
    if (!/\s/.test(before) && before !== '(' && before !== '[' && before !== '/') return null;
  }

  const quoted = text[at + 1] === '"';
  if (quoted) {
    const from = at + 2;
    if (position < from) return null;
    const close = text.indexOf('"', from);
    const newline = text.indexOf('\n', from);
    const closed = close !== -1;
    const end = closed ? close : newline === -1 ? text.length : newline;
    // The caret sits just past the closing quote when the quote was typed last.
    if (position > end && !(closed && position === end + 1)) return null;
    return {
      start: at,
      end: closed ? end + 1 : end,
      query: text.slice(from, end),
      quoted: true,
    };
  }

  const from = at + 1;
  if (position < from) return null;
  let end = from;
  while (end < text.length && !/\s/.test(text[end] as string)) end += 1;
  if (position > end) return null;
  const query = text.slice(from, Math.min(position, end));
  if (query.includes('\n')) return null;
  return { start: at, end, query, quoted: false };
}

/** A path needs quoting in a mention when it contains whitespace or a quote. */
export function needsQuoting(path: string): boolean {
  return /[\s"]/.test(path);
}

/** How a path should be written in the message body (`@x` or `@"x"`). */
export function formatMention(path: string): string {
  return needsQuoting(path) ? `@"${path}"` : `@${path}`;
}

export interface InsertResult {
  text: string;
  caret: number;
}

/** Replace the active token with the chosen path and put the caret after it. */
export function insertMention(text: string, mention: ActiveMention, path: string): InsertResult {
  const token = `${formatMention(path)} `;
  const next = `${text.slice(0, mention.start)}${token}${text.slice(mention.end)}`;
  return { text: next, caret: mention.start + token.length };
}

/** Loads the workspace tree behind the mention index. */
export type TreeLoader = (depth?: number) => Promise<WorkspaceTree>;
