import type { JSX } from 'preact';
import { lazy, Suspense } from 'preact/compat';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { TargetedKeyboardEvent } from 'preact';
import type { Terminal } from '../api/types';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../lib/menu';
import type { TerminalSocketStatus } from '../state/terminalSocket';
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

type TerminalActivationOrigin = 'keyboard' | 'pointer';

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
  // The keyed surface must know why it mounted. Pointer activation moves into
  // the shell; APG automatic keyboard activation must leave focus on the tab so
  // the next Arrow key still belongs to the tablist.
  const [activation, setActivation] = useState<{
    id: string;
    origin: TerminalActivationOrigin;
  } | null>(null);

  const select = useCallback((id: string, origin: TerminalActivationOrigin): void => {
    setActivation({ id, origin });
    terminalsState.selectTerminal(id);
  }, []);

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
        onSelect={select}
      />
      {active ? (
        <Suspense
          fallback={
            // The selected tab's `aria-controls` must resolve even while the
            // lazily-loaded emulator chunk is still arriving, so the fallback
            // carries the same panel id and label the surface will take over.
            <div
              class="terminal-empty"
              id={`terminal-panel-${active.id}`}
              role="tabpanel"
              aria-labelledby={`terminal-tab-${active.id}`}
              aria-busy="true"
              aria-label="Loading the terminal"
              tabIndex={-1}
            >
              <div class="terminal-empty-inner">
                <div class="skeleton skeleton-turn" />
              </div>
            </div>
          }
        >
          <TerminalSurfaceLazy
            key={active.id}
            terminal={active}
            autoFocus={activation?.id !== active.id || activation.origin !== 'keyboard'}
          />
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
  onSelect,
}: {
  list: Terminal[];
  activeId: string | null;
  atBudget: boolean;
  budget: number;
  onSelect: (id: string, origin: TerminalActivationOrigin) => void;
}): JSX.Element {
  const statuses = terminalsState.terminalStatus.value;
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const chipsRef = useRef<HTMLDivElement>(null);

  const create = async (): Promise<void> => {
    setCreating(true);
    try {
      await terminalsState.newTerminal();
    } finally {
      setCreating(false);
    }
  };

  // Measured by hand rather than with `scrollIntoView`, which would also scroll
  // the workspace around the strip. A chip that already fits leaves `scrollLeft`
  // alone, so selecting a visible tab never nudges the strip; jsdom reports zero
  // rects, which makes this a no-op there.
  const revealTab = (id: string | null): void => {
    const strip = chipsRef.current;
    if (!strip || id === null) return;
    const tab = document.getElementById(`terminal-tab-${id}`);
    if (!tab || !strip.contains(tab)) return;
    const stripBox = strip.getBoundingClientRect();
    const tabBox = tab.getBoundingClientRect();
    if (tabBox.left < stripBox.left) strip.scrollLeft -= stripBox.left - tabBox.left;
    else if (tabBox.right > stripBox.right) strip.scrollLeft += tabBox.right - stripBox.right;
  };

  // A terminal can become selected without a pointer - an arrow key, a keyboard
  // focus, a tab created elsewhere - and a selection off-screen is a selection
  // the user cannot see. The strip scrolls (terminal.css) rather than wrapping,
  // so this is what keeps the active chip reachable.
  useEffect(() => {
    revealTab(activeId);
  }, [activeId, list.length, renamingId]);

  const onKeyDown = (event: TargetedKeyboardEvent<HTMLDivElement>): void => {
    // Only the tabs own the arrow keys: the rename field and the chip actions
    // live inside this tablist too, and Home/End in a text field must move the
    // caret, not the selection.
    const target = event.target as HTMLElement | null;
    if (!target || target.getAttribute('role') !== 'tab') return;
    const index = list.findIndex((terminal) => terminal.id === activeId);
    if (index === -1 || list.length === 0) return;
    let next = index;
    if (event.key === 'ArrowRight') next = (index + 1) % list.length;
    else if (event.key === 'ArrowLeft') next = (index - 1 + list.length) % list.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = list.length - 1;
    else return;
    event.preventDefault();
    const targetTerminal = list[next];
    if (!targetTerminal) return;
    onSelect(targetTerminal.id, 'keyboard');
    // Automatic activation moves focus with the selection, so the next arrow key
    // is handled by the newly selected tab.
    document.getElementById(`terminal-tab-${targetTerminal.id}`)?.focus();
  };

  return (
    <div class="terminal-strip">
      <div
        ref={chipsRef}
        class="terminal-chips"
        role="tablist"
        aria-label="Terminals"
        aria-orientation="horizontal"
        onKeyDown={onKeyDown}
      >
        {list.map((terminal) => (
          <TerminalChip
            key={terminal.id}
            terminal={terminal}
            selected={terminal.id === activeId}
            status={statuses[terminal.id]}
            renaming={renamingId === terminal.id}
            onSelect={onSelect}
            onStartRename={() => setRenamingId(terminal.id)}
            onDoneRename={() => setRenamingId(null)}
            revealTab={revealTab}
          />
        ))}
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

/**
 * One chip in the strip: the tab itself plus its two actions.
 *
 * It is its own component because a menu is per chip — it opens at *this* chip's
 * pointer and returns focus to *this* tab, so one controller cannot serve the
 * strip. The chip has no kebab and never grows one (a tab strip is already
 * dense), so the menu is pointer-anchored only: `float`, portalled onto the menu
 * layer above the strip's horizontal scroller, which would otherwise clip it.
 *
 * **Rename** and **Close** stay on the chip as buttons; the menu is a second way
 * to reach them, not the only one. It also declines the right-click while this
 * chip is mid-rename, so the field keeps the browser's own menu — its Cut, Copy
 * and Paste are the affordance for a text selection, and ours has nothing to add.
 */
function TerminalChip({
  terminal,
  selected,
  status,
  renaming,
  onSelect,
  onStartRename,
  onDoneRename,
  revealTab,
}: {
  terminal: Terminal;
  selected: boolean;
  status: TerminalSocketStatus | undefined;
  renaming: boolean;
  onSelect: (id: string, origin: TerminalActivationOrigin) => void;
  onStartRename: () => void;
  onDoneRename: () => void;
  revealTab: (id: string) => void;
}): JSX.Element {
  const menu = useMenu();
  // Focus returns to the tab button: the chip's `div` is not focusable, and the
  // tab is the element a keyboard user would have activated.
  const selectRef = useRef<HTMLButtonElement>(null);

  return (
    <div
      class={`terminal-chip${selected ? ' is-active' : ''}${renaming ? ' is-renaming' : ''}`}
      data-status={status ?? 'idle'}
      {...contextMenuProps(menu, () => !renaming, () => selectRef.current)}
    >
      <button
        ref={selectRef}
        type="button"
        role="tab"
        id={`terminal-tab-${terminal.id}`}
        aria-selected={selected}
        // Only the selected terminal has a mounted panel; pointing the
        // other tabs at an id that does not exist makes the tablist
        // describe controls the document never renders.
        aria-controls={selected ? `terminal-panel-${terminal.id}` : undefined}
        tabIndex={selected ? 0 : -1}
        class="terminal-chip-select"
        title={`${terminal.title} — ${terminal.cwd}`}
        onClick={() => onSelect(terminal.id, 'pointer')}
        onFocus={() => revealTab(terminal.id)}
        onDblClick={onStartRename}
      >
        <span class="terminal-dot" aria-hidden="true" />
        <span class="terminal-chip-label">{terminal.title}</span>
        {terminal.running ? null : (
          <span class="terminal-chip-badge" title="The shell has exited">
            exited
          </span>
        )}
      </button>
      {renaming ? (
        <TerminalRenameInput
          terminal={terminal}
          onDone={onDoneRename}
        />
      ) : (
        <>
          <button
            type="button"
            class="terminal-chip-action"
            aria-label={`Rename ${terminal.title}`}
            title="Rename"
            onClick={onStartRename}
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
        </>
      )}
      <Menu menu={menu} float label={`Terminal actions for ${terminal.title}`}>
        <MenuItem menu={menu} action={{ label: 'Rename', title: 'Rename', onSelect: onStartRename }} />
        <MenuItem
          menu={menu}
          action={{
            label: 'Close',
            title: 'Close (ends the shell)',
            danger: true,
            onSelect: () => void terminalsState.closeTerminal(terminal.id),
          }}
        />
      </Menu>
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
  );
}
