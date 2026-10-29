import { useEffect, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { useFocusTrap } from '../lib/dom';
import { copyText } from '../lib/clipboard';
import { modKey } from '../lib/platform';
import type { SessionSummary } from '../api/types';
import { formatCount, relativeTime } from '../lib/format';
import * as store from '../state/store';
import * as actions from '../state/actions';
import { Dialog } from './Dialog';
import { CloseIcon, KebabIcon, PlusIcon, SearchIcon, SettingsIcon } from './icons';

const DAY = 86_400_000;

/** Bucket a session's last activity into the rail's date groups. */
export function sessionGroup(updatedAt: number, now: number): string {
  const startOfDay = (value: number): number => {
    const date = new Date(value);
    date.setHours(0, 0, 0, 0);
    return date.getTime();
  };
  const today = startOfDay(now);
  const day = startOfDay(updatedAt);
  if (day >= today) return 'Today';
  if (day >= today - DAY) return 'Yesterday';
  if (day >= today - 7 * DAY) return 'Previous 7 days';
  return 'Older';
}

interface SessionRowProps {
  session: SessionSummary;
  active: boolean;
  now: number;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onRequestDelete: (session: SessionSummary, trigger: HTMLElement | null) => void;
}

function SessionRow({ session, active, now, onSelect, onRename, onRequestDelete }: SessionRowProps): JSX.Element {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(session.title);
  const [menuOpen, setMenuOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const skipBlur = useRef(false);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  useEffect(() => {
    if (!menuOpen) return;
    const first = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]');
    first?.focus();
    const onDocumentClick = (event: MouseEvent): void => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        setMenuOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener('mousedown', onDocumentClick);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onDocumentClick);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [menuOpen]);

  useEffect(() => {
    if (!editing) setDraft(session.title);
  }, [session.title, editing]);

  const startRename = (): void => {
    skipBlur.current = false;
    setMenuOpen(false);
    setDraft(session.title);
    setEditing(true);
  };

  const cancelRename = (): void => {
    skipBlur.current = true;
    setEditing(false);
    setDraft(session.title);
  };

  const commitRename = (): void => {
    if (skipBlur.current) {
      skipBlur.current = false;
      return;
    }
    const next = draft.trim();
    setEditing(false);
    if (next !== '' && next !== session.title) void onRename(session.id, next);
    else setDraft(session.title);
  };

  const statusTone = session.running
    ? 'running'
    : session.last_run_status === 'failed'
      ? 'danger'
      : session.last_run_status === 'interrupted'
        ? 'warn'
        : null;

  return (
    <li class={`session-row${active ? ' is-active' : ''}`} data-session-id={session.id}>
      {editing ? (
        <form
          class="session-rename"
          onSubmit={(event) => {
            event.preventDefault();
            commitRename();
          }}
        >
          <input
            ref={inputRef}
            class="session-rename-input"
            value={draft}
            aria-label={`Rename session ${session.title}`}
            maxLength={200}
            onInput={(event) => setDraft(event.currentTarget.value)}
            onBlur={commitRename}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                cancelRename();
              }
            }}
          />
        </form>
      ) : (
        <button
          type="button"
          class="session-open"
          onClick={() => onSelect(session.id)}
          onDblClick={startRename}
          aria-current={active ? 'true' : undefined}
        >
          <span class="session-title">{session.title || 'Untitled session'}</span>
          <span class="session-meta">
            {statusTone ? <span class={`status-dot status-${statusTone}`} aria-hidden="true" /> : null}
            {session.running ? <span class="sr-only">Running. </span> : null}
            <span>{relativeTime(session.updated_at, now)}</span>
            <span class="meta-dot" aria-hidden="true" />
            <span title={`${session.message_count}`}>{formatCount(session.message_count)}</span>
            {session.last_run_status === 'failed' ? <span class="sr-only">Last run failed.</span> : null}
            {session.last_run_status === 'interrupted' ? (
              <span class="sr-only">Last run interrupted.</span>
            ) : null}
          </span>
        </button>
      )}

      {!editing ? (
        <div class="session-menu-wrap" ref={menuRef}>
          <button
            type="button"
            ref={triggerRef}
            class="icon-button session-menu-trigger"
            aria-label={`Actions for ${session.title || 'untitled session'}`}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((open) => !open)}
          >
            <KebabIcon size={15} />
          </button>
          {menuOpen ? (
            <div
              class="session-menu"
              role="menu"
              aria-label={`Session actions for ${session.title}`}
              onKeyDown={(event) => {
                const items = Array.from(
                  menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
                );
                if (items.length === 0) return;
                const index = items.indexOf(document.activeElement as HTMLButtonElement);
                let next = index;
                if (event.key === 'ArrowDown') next = (index + 1) % items.length;
                else if (event.key === 'ArrowUp') next = (index - 1 + items.length) % items.length;
                else if (event.key === 'Home') next = 0;
                else if (event.key === 'End') next = items.length - 1;
                else if (event.key === 'Tab') {
                  setMenuOpen(false);
                  return;
                } else return;
                event.preventDefault();
                items[next]?.focus();
              }}
            >
              <button type="button" role="menuitem" class="menu-item" onClick={startRename}>
                Rename
              </button>
              <button
                type="button"
                role="menuitem"
                class="menu-item menu-item-danger"
                onClick={() => {
                  triggerRef.current?.focus();
                  setMenuOpen(false);
                  onRequestDelete(session, triggerRef.current);
                }}
              >
                Delete…
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

export interface SessionListProps {
  sessions: SessionSummary[];
  currentId: string | null;
  now: number;
  loading: boolean;
  workspace?: string;
  /** Case-insensitive title filter typed into the rail's search field. */
  query?: string;
  onCreate: () => void;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onDelete: (id: string) => void | Promise<unknown>;
}

/** Presentational session list; exported for tests. */
export function SessionList({
  sessions,
  currentId,
  now,
  loading,
  workspace,
  query = '',
  onCreate,
  onSelect,
  onRename,
  onDelete,
}: SessionListProps): JSX.Element {
  const [pendingDelete, setPendingDelete] = useState<{
    session: SessionSummary;
    trigger: HTMLElement | null;
  } | null>(null);

  const closeDelete = (): void => {
    const trigger = pendingDelete?.trigger;
    setPendingDelete(null);
    if (trigger) {
      window.setTimeout(() => {
        if (trigger.isConnected) trigger.focus();
      }, 0);
    }
  };

  if (loading && sessions.length === 0) {
    return (
      <div class="session-list-loading" aria-busy="true" aria-label="Loading sessions">
        {[0, 1, 2, 3].map((index) => (
          <div class="skeleton skeleton-session" key={index} />
        ))}
      </div>
    );
  }

  const needle = query.trim().toLowerCase();
  const visible = needle === '' ? sessions : sessions.filter((session) => (session.title || '').toLowerCase().includes(needle));

  // Preserve the server order (newest first) inside each date bucket.
  const groups: { label: string; items: SessionSummary[] }[] = [];
  for (const session of visible) {
    const label = sessionGroup(session.updated_at, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.items.push(session);
    else groups.push({ label, items: [session] });
  }

  return (
    <div class="session-list-wrap">
      {sessions.length === 0 ? (
        <div class="rail-empty">
          <p class="rail-empty-title">No sessions yet</p>
          <p class="rail-empty-body">
            A session keeps one conversation, its runs and the files it touches
            {workspace ? (
              <>
                {' '}
                in <code class="inline-path">{workspace}</code>
              </>
            ) : null}
            .
          </p>
          <button type="button" class="button button-primary" onClick={onCreate}>
            <PlusIcon size={14} /> New session
          </button>
          <p class="rail-hint">
            Create with <kbd>{modKey()}</kbd> + <kbd>N</kbd>, or pick one from the command palette{' '}
            <kbd>{modKey()}</kbd> + <kbd>K</kbd>.
          </p>
        </div>
      ) : visible.length === 0 ? (
        <div class="rail-empty" role="status">
          <p class="rail-empty-title">No matching sessions</p>
          <p class="rail-empty-body">
            Nothing in this project matches <strong>{query.trim()}</strong>. Searches only look at session
            titles.
          </p>
          <button type="button" class="button" onClick={onCreate}>
            <PlusIcon size={14} /> New session
          </button>
        </div>
      ) : (
        groups.map((group) => (
          <section class="session-group" key={group.label} aria-label={`${group.label} sessions`}>
            <h2 class="session-group-label">
              {group.label}
              <span aria-hidden="true">{group.items.length}</span>
            </h2>
            <ul class="session-list">
              {group.items.map((session) => (
                <SessionRow
                  key={session.id}
                  session={session}
                  active={session.id === currentId}
                  now={now}
                  onSelect={onSelect}
                  onRename={onRename}
                  onRequestDelete={(session, trigger) => setPendingDelete({ session, trigger })}
                />
              ))}
            </ul>
          </section>
        ))
      )}

      <Dialog open={pendingDelete !== null} title="Delete session?" onClose={closeDelete} class="dialog-confirm">
        {pendingDelete ? (
          <div class="confirm-body">
            <p>
              Delete <strong>{pendingDelete.session.title || 'this session'}</strong>? Its messages, runs and
              approvals are removed. This cannot be undone.
            </p>
            <div class="dialog-actions">
              <button type="button" class="button" data-autofocus onClick={closeDelete}>
                Cancel
              </button>
              <button
                type="button"
                class="button button-danger"
                onClick={() => {
                  const target = pendingDelete.session;
                  closeDelete();
                  void onDelete(target.id);
                }}
              >
                Delete session
              </button>
            </div>
          </div>
        ) : null}
      </Dialog>
    </div>
  );
}

function connectionTone(): string {
  const value = store.connection.value;
  if (value.api === 'offline') return 'danger';
  if (value.stream === 'unauthorized') return 'danger';
  if (value.stream === 'reconnecting' || value.stream === 'connecting') return 'warn';
  if (value.stream === 'open') return 'ok';
  if (value.api === 'online') return 'ok';
  return 'muted';
}

interface ProjectMenuState {
  top: number;
  left: number;
}

export function SessionsRail(): JSX.Element {
  const sessions = store.sessions.value;
  const currentId = store.currentSessionId.value;
  const isSheetOpen = store.mobileSheet.value === 'sessions';
  const isNarrow = store.narrowViewport.value;
  const project = store.currentProject.value;
  const railRef = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionsRef = useRef<HTMLButtonElement>(null);
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState<ProjectMenuState | null>(null);
  useFocusTrap(railRef, isNarrow && isSheetOpen, actions.closeSheets);

  // `/` focuses the session search, like the mock's keycap promises.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest('input, textarea, select, [contenteditable="true"]')
      ) {
        return;
      }
      if (store.settingsOpen.value || store.paletteOpen.value || store.themeStudioOpen.value) return;
      event.preventDefault();
      if (isNarrow && store.mobileSheet.value !== 'sessions') actions.toggleRail();
      else if (!isNarrow && store.railCollapsed.value) actions.toggleRail();
      window.setTimeout(() => searchRef.current?.focus(), 0);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isNarrow]);

  const closeMenu = (restoreFocus = true): void => {
    setMenu(null);
    if (restoreFocus) {
      window.setTimeout(() => optionsRef.current?.focus(), 0);
    }
  };

  useEffect(() => {
    if (!menu) return;
    const first = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]');
    first?.focus();
    const onPointerDown = (event: MouseEvent): void => {
      const target = event.target as Node | null;
      if (menuRef.current?.contains(target)) return;
      setMenu(null);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        closeMenu();
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    const onResize = (): void => setMenu(null);
    window.addEventListener('resize', onResize);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', onResize);
    };
  }, [menu]);

  const openProjectMenu = (): void => {
    const rect = optionsRef.current?.getBoundingClientRect();
    setMenu({
      top: rect && rect.height > 0 ? rect.bottom + 6 : 60,
      left: rect && rect.width > 0 ? rect.left : 16,
    });
  };

  const copyPath = async (): Promise<void> => {
    const path = project?.path ?? '';
    if (path === '') return;
    const copied = await copyText(path);
    store.pushToast(copied ? 'Project path copied.' : 'Could not copy the path.', copied ? 'success' : 'error');
  };

  const providerLabel = store.providerConfigured.value
    ? store.activeModelLabel.value ?? 'Provider configured'
    : 'No provider';

  return (
    <aside
      ref={railRef}
      class={`rail${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Sessions"
      role={isSheetOpen ? 'dialog' : undefined}
      aria-modal={isSheetOpen ? 'true' : undefined}
    >
      <div class="rail-inner">
        <div class="side-head">
          <p class="eyebrow">Your workspace</p>
          <div class="project-heading">
            <h2 class="project-title" title={project?.name ?? 'No project'}>
              <span class="project-title-dot" aria-hidden="true" />
              <span class="project-title-text">{project?.name ?? 'No project'}</span>
            </h2>
            <div class="side-head-actions">
              <button
                ref={optionsRef}
                type="button"
                class="side-mini-btn"
                aria-label="Project options"
                aria-haspopup="menu"
                aria-expanded={menu !== null}
                title="Project options"
                onClick={() => (menu ? closeMenu(false) : openProjectMenu())}
              >
                <KebabIcon size={15} />
              </button>
              {isNarrow ? (
                <button
                  type="button"
                  class="side-mini-btn rail-close"
                  aria-label="Close sessions panel"
                  data-autofocus
                  onClick={actions.closeSheets}
                >
                  <CloseIcon size={15} />
                </button>
              ) : null}
            </div>
          </div>
          <p class="path-hint" title={project?.path ?? ''}>
            {project?.path ?? 'No project selected'}
          </p>
        </div>

        <button
          type="button"
          class="new-session"
          onClick={() => void actions.createSession()}
          aria-label={`New session (${modKey()} + N)`}
        >
          <span class="new-left">
            <PlusIcon size={15} /> New session
          </span>
          <span class="shortcut">{modKey()} N</span>
        </button>

        <label class="side-search">
          <SearchIcon size={15} />
          <input
            ref={searchRef}
            type="search"
            value={query}
            placeholder="Find a session…"
            aria-label="Find a session"
            autocomplete="off"
            spellcheck={false}
            onInput={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                if (query !== '') {
                  event.preventDefault();
                  setQuery('');
                }
              } else if (event.key === 'Enter') {
                const first = sessions.find((session) =>
                  (session.title || '').toLowerCase().includes(query.trim().toLowerCase()),
                );
                if (first) {
                  event.preventDefault();
                  actions.switchSession(first.id);
                }
              }
            }}
          />
          <span class="keycap" aria-hidden="true">
            /
          </span>
        </label>

        <nav class="session-nav" aria-label="Session list">
          <SessionList
            sessions={sessions}
            currentId={currentId}
            now={store.clock.value}
            loading={store.sessionsLoading.value}
            workspace={
              project?.path ?? store.health.value?.workspace ?? store.config.value?.workspace
            }
            query={query}
            onCreate={() => void actions.createSession()}
            onSelect={actions.switchSession}
            onRename={actions.renameSessionById}
            onDelete={actions.deleteSessionById}
          />
        </nav>

        <div class="side-footer">
          <div class="online-row">
            <span class={`online-ind status-${connectionTone()}`} aria-hidden="true" />
            <span>{store.connectionLabel.value}</span>
            <span class="right">{store.health.value ? `v${store.health.value.version}` : 'local'}</span>
          </div>
          <div class="settings-row">
            <button type="button" class="settings-btn" onClick={actions.openSettings}>
              <SettingsIcon size={15} /> Settings
            </button>
            <span class={store.providerConfigured.value ? 'provider' : 'provider is-warn'} title={providerLabel}>
              {providerLabel}
            </span>
          </div>
          {store.connection.value.lastError ? (
            <p class="rail-error" title={store.connection.value.lastError}>
              {store.connection.value.lastError}
            </p>
          ) : null}
        </div>
      </div>

      {menu ? (
        <div
          ref={menuRef}
          class="rail-menu"
          role="menu"
          aria-label={`Project options for ${project?.name ?? 'this project'}`}
          style={{ top: `${menu.top}px`, left: `${menu.left}px` }}
          onKeyDown={(event) => {
            const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
            if (items.length === 0) return;
            const index = items.indexOf(document.activeElement as HTMLButtonElement);
            let next = index;
            if (event.key === 'ArrowDown') next = (index + 1) % items.length;
            else if (event.key === 'ArrowUp') next = (index - 1 + items.length) % items.length;
            else if (event.key === 'Home') next = 0;
            else if (event.key === 'End') next = items.length - 1;
            else if (event.key === 'Tab') {
              setMenu(null);
              return;
            } else return;
            event.preventDefault();
            items[next]?.focus();
          }}
        >
          <div class="rail-menu-meta">
            <span class="rail-menu-name">{project?.name ?? 'No project'}</span>
            <span class="rail-menu-path">{project?.path ?? ''}</span>
          </div>
          <button
            type="button"
            role="menuitem"
            class="menu-item"
            onClick={() => {
              closeMenu(false);
              void copyPath();
            }}
          >
            Copy project path
          </button>
          <button
            type="button"
            role="menuitem"
            class="menu-item"
            onClick={() => {
              closeMenu(false);
              void actions.refreshSessions();
            }}
          >
            Refresh sessions
          </button>
          <button
            type="button"
            role="menuitem"
            class="menu-item"
            onClick={() => {
              closeMenu(false);
              actions.openSettings();
            }}
          >
            Project settings…
          </button>
        </div>
      ) : null}
    </aside>
  );
}