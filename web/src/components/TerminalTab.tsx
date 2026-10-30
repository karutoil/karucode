import type { JSX } from 'preact';
import { lazy, Suspense } from 'preact/compat';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { Terminal } from '../api/types';
import * as terminalsState from '../state/terminals';
import { AlertIcon, CloseIcon, EditIcon, PlusIcon, RefreshIcon, TerminalIcon } from './icons';

/**
 * xterm is ~200 kB of emulator, and a user who never opens the terminal tab
 * should not download it. The surface is a separate chunk, fetched the first
 * time the tab is selected; the strip and the empty states below are tiny and
 * stay in the main bundle, so the tab is usable before the emulator arrives.
 */
const TerminalSurfaceLazy = lazy(() =>
  import('./TerminalSurface').then((module) => ({ default: module.TerminalSurface })),
);

/**
 * The terminal tab: a real shell in a real PTY, drawn by xterm.js.
 *
 * The tab strip is **server data** (`state/terminals.ts`), so the same tabs
 * appear in every browser and after a server restart. Only the *selected* tab is
 * a browser preference. Attaching is a websocket that replays the server's
 * scrollback, which is what makes a reload look like nothing happened.
 */
export function TerminalTab(): JSX.Element {
  const enabled = terminalsState.terminalsEnabled.value;
  const list = terminalsState.terminals.value;
  const active = terminalsState.activeTerminal.value;
  const loading = terminalsState.terminalsLoading.value;
  const error = terminalsState.terminalsError.value;
  const atBudget = terminalsState.atTerminalBudget.value;
  const budget = terminalsState.terminalBudget.value;

  // One fetch on mount. The list is server-owned, so there is no polling: a
  // second browser's create shows up on the next mount or the next mutation.
  useEffect(() => {
    void terminalsState.loadTerminals();
  }, []);

  const createFirst = useCallback(() => {
    void terminalsState.newTerminal();
  }, []);

  // Order matters: a failed fetch must not be reported as "disabled". Until the
  // list loads, `enabled` is false because nothing has said otherwise yet, so
  // the error state has to win.
  if (error && list.length === 0) {
    return (
      <div class="terminal-empty">
        <div class="terminal-empty-inner">
          <AlertIcon size={20} />
          <p class="terminal-empty-title">Could not load terminals</p>
          <p>{error}</p>
          <button
            type="button"
            class="button button-small"
            onClick={() => void terminalsState.loadTerminals()}
          >
            <RefreshIcon size={13} /> Try again
          </button>
        </div>
      </div>
    );
  }

  if (!enabled) {
    return (
      <div class="terminal-empty">
        <div class="terminal-empty-inner">
          <TerminalIcon size={22} />
          <p class="terminal-empty-title">The terminal is disabled</p>
          <p>
            This server does not offer a shell. Set <code>terminals.enabled = true</code> in the
            config file, or remove <code>KARUCODE_TERMINALS=0</code>, and restart it.
          </p>
        </div>
      </div>
    );
  }

  if (loading) {
    return (
      <div class="terminal-empty" aria-busy="true">
        <div class="terminal-empty-inner">
          <div class="skeleton skeleton-turn" />
        </div>
      </div>
    );
  }

  return (
    <div class="terminal-view">
      <TerminalStrip
        list={list}
        activeId={active?.id ?? null}
        atBudget={atBudget}
        budget={budget}
      />
      {active ? (
        <Suspense
          fallback={
            <div class="terminal-empty" aria-busy="true" aria-label="Loading the terminal">
              <div class="terminal-empty-inner">
                <div class="skeleton skeleton-turn" />
              </div>
            </div>
          }
        >
          <TerminalSurfaceLazy key={active.id} terminal={active} />
        </Suspense>
      ) : (
        <div class="terminal-empty">
          <div class="terminal-empty-inner">
            <TerminalIcon size={22} />
            <p class="terminal-empty-title">No terminal yet</p>
            <p>
              A terminal is a real shell on the server, as its user, in this project's directory.
              It keeps running when you reload the page, switch browser or clear your cache
              {' '}&mdash;{' '}
              the tabs themselves are stored on the server.
            </p>
            <button type="button" class="button button-small accent" onClick={createFirst}>
              <PlusIcon size={13} /> New terminal
            </button>
            {terminalsState.terminalShell.value ? (
              <p class="terminal-empty-hint">
                Shell: <code>{terminalsState.terminalShell.value}</code>
              </p>
            ) : null}
          </div>
        </div>
      )}
    </div>
  );
}

/** The sub-tab strip: one chip per terminal, plus **New**. */
function TerminalStrip({
  list,
  activeId,
  atBudget,
  budget,
}: {
  list: Terminal[];
  activeId: string | null;
  atBudget: boolean;
  budget: number;
}): JSX.Element {
  const statuses = terminalsState.terminalStatus.value;
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const create = async (): Promise<void> => {
    setCreating(true);
    try {
      await terminalsState.newTerminal();
    } finally {
      setCreating(false);
    }
  };

  return (
    <div class="terminal-strip">
      <div class="terminal-chips" role="tablist" aria-label="Terminals">
        {list.map((terminal) => {
          const selected = terminal.id === activeId;
          const status = statuses[terminal.id];
          if (renamingId === terminal.id) {
            return (
              <TerminalRenameInput
                key={terminal.id}
                terminal={terminal}
                onDone={() => setRenamingId(null)}
              />
            );
          }
          return (
            <div
              key={terminal.id}
              class={`terminal-chip${selected ? ' is-active' : ''}`}
              data-status={status ?? 'idle'}
            >
              <button
                type="button"
                role="tab"
                id={`terminal-tab-${terminal.id}`}
                aria-selected={selected}
                aria-controls={`terminal-panel-${terminal.id}`}
                tabIndex={selected ? 0 : -1}
                class="terminal-chip-select"
                title={`${terminal.title} — ${terminal.cwd}`}
                onClick={() => terminalsState.selectTerminal(terminal.id)}
                onDblClick={() => setRenamingId(terminal.id)}
              >
                <span class="terminal-dot" aria-hidden="true" />
                <span class="terminal-chip-label">{terminal.title}</span>
                {terminal.running ? null : (
                  <span class="terminal-chip-badge" title="The shell has exited">
                    exited
                  </span>
                )}
              </button>
              <button
                type="button"
                class="terminal-chip-action"
                aria-label={`Rename ${terminal.title}`}
                title="Rename"
                onClick={() => setRenamingId(terminal.id)}
              >
                <EditIcon size={11} />
              </button>
              <button
                type="button"
                class="terminal-chip-action"
                aria-label={`Close ${terminal.title}`}
                title="Close (ends the shell)"
                onClick={() => void terminalsState.closeTerminal(terminal.id)}
              >
                <CloseIcon size={11} />
              </button>
            </div>
          );
        })}
        <button
          type="button"
          class="terminal-new"
          onClick={() => void create()}
          disabled={atBudget || creating}
          title={
            atBudget
              ? `At the ${budget}-terminal limit (terminals.max_terminals)`
              : 'New terminal'
          }
        >
          <PlusIcon size={12} />
          <span>New</span>
        </button>
      </div>
      <p class="terminal-count">
        {list.length}/{budget}
      </p>
    </div>
  );
}

/** An inline rename field: Enter saves, Escape cancels, blank restores the default. */
function TerminalRenameInput({
  terminal,
  onDone,
}: {
  terminal: Terminal;
  onDone: () => void;
}): JSX.Element {
  const [value, setValue] = useState(terminal.title);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const commit = (): void => {
    const trimmed = value.trim();
    void terminalsState.renameTerminalTo(terminal.id, trimmed === '' ? null : trimmed);
    onDone();
  };

  return (
    <div class="terminal-chip is-renaming">
      <input
        ref={inputRef}
        class="terminal-rename-input"
        value={value}
        aria-label={`Rename ${terminal.title}`}
        maxLength={80}
        onInput={(event) => setValue((event.target as HTMLInputElement).value)}
        onBlur={commit}
        onKeyDown={(event) => {
          // The app-wide Escape handler stops a run; this field owns it first.
          event.stopPropagation();
          if (event.key === 'Enter') {
            event.preventDefault();
            commit();
          } else if (event.key === 'Escape') {
            event.preventDefault();
            onDone();
          }
        }}
      />
    </div>
  );
}
