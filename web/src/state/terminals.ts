import { computed, signal } from '@preact/signals';
import {
  createTerminal,
  deleteTerminal,
  listTerminals,
  renameTerminal,
  restartTerminal,
} from '../api/endpoints';
import { toErrorMessage } from '../api/client';
import type { Terminal } from '../api/types';
import * as store from './store';
import type { TerminalSocketStatus } from './terminalSocket';

/**
 * The terminal tab strip.
 *
 * The list is **server-owned**: it comes from `GET /api/terminals`, which reads
 * SQLite, so the same tabs appear in every browser and after a server restart.
 * Nothing here is kept in `localStorage` except which tab was last selected -
 * that is a view preference, not data, and putting the list itself in storage is
 * exactly the "lost when the cache is cleared" failure this feature exists to
 * avoid.
 */

const ACTIVE_TERMINAL_KEY = 'karucode.activeTerminal';

/** `terminals.enabled` off means the workspace tab is not offered at all. */
export const terminalsEnabled = signal(false);
export const terminals = signal<Terminal[]>([]);
export const terminalsLoading = signal(false);
export const terminalsError = signal<string | null>(null);
/** `terminals.max_terminals`, for disabling **New** at the cap. */
export const terminalBudget = signal(16);
/** The resolved shell program, shown as a hint in the empty state. */
export const terminalShell = signal<string>('');

/**
 * The visible terminal, or `null` when none exists yet.
 *
 * Kept as an id (not a row) so it survives a refetch: a rename or a restart
 * replaces the row but not the selection. Persisted because "which tab was I
 * on" is a per-browser preference; the tab itself is not.
 */
export const activeTerminalId = signal<string | null>(readActiveTerminalId());

/** Live connection state per terminal id, for the status dot in the strip. */
export const terminalStatus = signal<Record<string, TerminalSocketStatus>>({});

/**
 * How the closed terminal's emulator is released.
 *
 * xterm lives in a lazily-loaded chunk (`components/TerminalSurface`), so this
 * module cannot import it without dragging ~200 kB of emulator into the main
 * bundle for every user who never opens the terminal tab. The surface registers
 * its own `disposeTerminal` here on load instead, and closing a tab calls it.
 * Until the chunk has loaded there is no emulator to release.
 */
type TerminalDisposer = (id: string) => void;
let emulatorDisposer: TerminalDisposer | null = null;

export function setTerminalEmulatorDisposer(disposer: TerminalDisposer | null): void {
  emulatorDisposer = disposer;
}

function releaseEmulator(id: string): void {
  try {
    emulatorDisposer?.(id);
  } catch {
    // Releasing an emulator must never fail a close.
  }
}

export const activeTerminal = computed<Terminal | null>(() => {
  const list = terminals.value;
  const id = activeTerminalId.value;
  return list.find((terminal) => terminal.id === id) ?? list[0] ?? null;
});

export const atTerminalBudget = computed(
  () => terminals.value.length >= terminalBudget.value,
);

function readActiveTerminalId(): string | null {
  try {
    return typeof localStorage === 'undefined'
      ? null
      : localStorage.getItem(ACTIVE_TERMINAL_KEY);
  } catch {
    return null;
  }
}

export function persistActiveTerminalId(id: string | null): void {
  try {
    if (typeof localStorage === 'undefined') return;
    if (id) localStorage.setItem(ACTIVE_TERMINAL_KEY, id);
    else localStorage.removeItem(ACTIVE_TERMINAL_KEY);
  } catch {
    // Storage may be disabled; the in-memory signal still works for this tab.
  }
}

export function selectTerminal(id: string | null): void {
  activeTerminalId.value = id;
  persistActiveTerminalId(id);
}

export function setTerminalStatus(id: string, status: TerminalSocketStatus): void {
  const current = terminalStatus.value;
  if (current[id] === status) return;
  terminalStatus.value = { ...current, [id]: status };
}

/**
 * Fetch the list. Called on mount and after any mutation, so the strip always
 * reflects what the server actually has rather than an optimistic guess that can
 * drift (a create that failed, a terminal another browser closed).
 */
export async function loadTerminals(): Promise<void> {
  // The tab is only offered when the server says the feature is on, and that
  // flag comes with the list itself: one request answers both questions.
  terminalsLoading.value = terminals.value.length === 0;
  terminalsError.value = null;
  try {
    const result = await listTerminals();
    terminalsEnabled.value = result.enabled;
    terminalBudget.value = result.max_terminals ?? 16;
    terminalShell.value = result.shell ?? '';
    terminals.value = result.terminals ?? [];
    // Keep the selection valid: a terminal closed in another browser must not
    // leave the panel pointing at a row that no longer exists.
    const ids = new Set(terminals.value.map((terminal) => terminal.id));
    if (activeTerminalId.value && !ids.has(activeTerminalId.value)) {
      selectTerminal(terminals.value[0]?.id ?? null);
    }
  } catch (error) {
    terminalsError.value = toErrorMessage(error);
  } finally {
    terminalsLoading.value = false;
  }
}

/**
 * Create a terminal and select it.
 *
 * The directory is the current project's, which is what a user opening the
 * terminal tab expects; a terminal is not confined there, so `cd` still works.
 */
export async function newTerminal(options: { cols?: number; rows?: number } = {}): Promise<Terminal | null> {
  terminalsError.value = null;
  if (atTerminalBudget.value) {
    store.pushToast(
      `You already have ${terminalBudget.value} terminals (the ${'`terminals.max_terminals`'} limit). Close one first.`,
      'error',
    );
    return null;
  }
  try {
    const { terminal } = await createTerminal({
      project: store.currentProjectId.value,
      cols: options.cols,
      rows: options.rows,
    });
    terminals.value = [...terminals.value, terminal];
    selectTerminal(terminal.id);
    return terminal;
  } catch (error) {
    terminalsError.value = toErrorMessage(error);
    store.pushToast(toErrorMessage(error), 'error');
    return null;
  }
}

/** Close a terminal: the shell is killed, the tab disappears and its emulator is released. */
export async function closeTerminal(id: string): Promise<void> {
  const previous = terminals.value;
  const index = previous.findIndex((terminal) => terminal.id === id);
  // Optimistic removal so the tab disappears on click; a failure restores it.
  terminals.value = previous.filter((terminal) => terminal.id !== id);
  if (activeTerminalId.value === id) {
    const next = terminals.value[Math.min(index, terminals.value.length - 1)];
    selectTerminal(next?.id ?? null);
  }
  // The emulator is module-level state keyed by id (`components/TerminalSurface`)
  // and outlives the unmount, so the tab has to release it explicitly or every
  // terminal ever viewed keeps an emulator, canvas and listeners for the life of
  // the page.
  releaseEmulator(id);
  try {
    await deleteTerminal(id);
  } catch (error) {
    terminals.value = previous;
    // The tab is coming back, so the emulator must survive too: recreate it on
    // the next mount rather than restoring a disposed one.
    store.pushToast(toErrorMessage(error), 'error');
  }
}

export async function renameTerminalTo(id: string, title: string | null): Promise<void> {
  try {
    const { terminal } = await renameTerminal(id, title);
    terminals.value = terminals.value.map((existing) =>
      existing.id === id ? terminal : existing,
    );
  } catch (error) {
    store.pushToast(toErrorMessage(error), 'error');
  }
}

/** Kill the shell and start a new one, keeping the tab's id, name and place. */
export async function restartTerminalShell(id: string): Promise<void> {
  try {
    const { terminal } = await restartTerminal(id);
    terminals.value = terminals.value.map((existing) =>
      existing.id === id ? terminal : existing,
    );
  } catch (error) {
    store.pushToast(toErrorMessage(error), 'error');
  }
}

/** Test seam: reset every signal. */
export function resetTerminalState(): void {
  terminalsEnabled.value = false;
  terminals.value = [];
  terminalsLoading.value = false;
  terminalsError.value = null;
  terminalBudget.value = 16;
  terminalShell.value = '';
  activeTerminalId.value = null;
  terminalStatus.value = {};
}