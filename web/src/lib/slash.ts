/**
 * Slash commands in the message box.
 *
 * The catalog comes from `GET /api/commands`; everything here is pure so the
 * filter, the menu and the send path can never disagree about what the user is
 * typing.
 *
 * The parse rule mirrors the server's (`crates/kc-server/src/slash/mod.rs`)
 * exactly, because the server is what decides whether a message is a command: a
 * command must be the **first** token (`/home/me/notes.md` is a path), a name is
 * ASCII letters, digits, `-` and `_`, and what follows it must be the end of the
 * message or whitespace (`/compact!` is prose).
 */

import type { CommandSpec } from '../api/types';

/** How many rows the `/` menu shows at once. */
export const SLASH_MENU_LIMIT = 8;

/** The name a message invokes, lowercased, or `null` when it is not a command. */
export function commandName(content: string): string | null {
  if (!content.startsWith('/')) return null;
  const match = /^\/([A-Za-z][A-Za-z0-9_-]*)/.exec(content);
  if (!match) return null;
  const rest = content.slice(match[0].length);
  if (!(rest === '' || /^\s/.test(rest))) return null;
  return match[1]!.toLowerCase();
}

/** Everything after the command name, trimmed. */
export function commandArgs(content: string): string {
  const match = /^\/[A-Za-z][A-Za-z0-9_-]*/.exec(content);
  return match ? content.slice(match[0].length).trim() : '';
}

/**
 * The name being typed, while the menu should be open.
 *
 * Only the whole draft counts: the menu appears when the box starts with `/` and
 * closes the moment a space is typed, because from then on the user is writing
 * arguments and every key belongs to them. The caret has to be at the end of the
 * draft for the same reason - a menu popping open over text the user went back to
 * edit is an interruption.
 */
export function slashQuery(draft: string, caret: number): string | null {
  if (caret < draft.length) return null;
  const match = /^\/([A-Za-z0-9_-]*)$/.exec(draft);
  if (!match) return null;
  return (match[1] ?? '').toLowerCase();
}

/** Score one command against a query; 0 means "no match". */
export function scoreCommand(command: CommandSpec, query: string): number {
  const needle = query.trim().toLowerCase();
  if (needle === '') return 1;
  const name = command.name.toLowerCase();
  const title = command.title.toLowerCase();
  const summary = command.summary.toLowerCase();
  if (name === needle) return 100;
  if (name.startsWith(needle)) return 90 - Math.min(name.length - needle.length, 40);
  if (name.includes(needle)) return 70;
  if (title.startsWith(needle)) return 55;
  if (title.includes(needle)) return 45;
  if (summary.includes(needle)) return 30;
  return 0;
}

/**
 * Rank the catalog for what has been typed, best first.
 *
 * An empty query keeps the server's order: built-ins in menu order, then the
 * skill library. That is the order `/` alone should show, because it is the order
 * somebody learning the commands reads them in.
 */
export function rankCommands(
  catalog: readonly CommandSpec[],
  query: string,
  limit: number = SLASH_MENU_LIMIT,
): CommandSpec[] {
  if (query.trim() === '') return catalog.slice(0, Math.max(0, limit));
  const scored: { command: CommandSpec; score: number; order: number }[] = [];
  catalog.forEach((command, order) => {
    const score = scoreCommand(command, query);
    if (score > 0) scored.push({ command, score, order });
  });
  scored.sort(
    (a, b) => b.score - a.score || a.order - b.order || a.command.name.localeCompare(b.command.name),
  );
  return scored.slice(0, Math.max(0, limit)).map((entry) => entry.command);
}

/** The catalog row for a message, when the message is exactly that command. */
export function knownCommand(
  catalog: readonly CommandSpec[],
  content: string,
): CommandSpec | null {
  const name = commandName(content);
  if (name === null) return null;
  return catalog.find((command) => command.name.toLowerCase() === name) ?? null;
}

/** How the composer writes a picked command into the box. */
export function insertCommand(command: CommandSpec): string {
  return command.insert !== '' ? command.insert : `/${command.name}`;
}

/**
 * Whether a name is one a skill could be invoked by, matching the server.
 *
 * Used only to reject a name the menu should not have offered; the server has the
 * last word on every command either way.
 */
export function isCommandToken(name: string): boolean {
  return /^[a-z][a-z0-9_-]*$/.test(name);
}