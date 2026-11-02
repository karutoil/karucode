import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { useFocusTrap } from '../lib/dom';
import { copyText } from '../lib/clipboard';
import { modKey } from '../lib/platform';
import type { SessionSummary } from '../api/types';
import { formatCount, relativeTime } from '../lib/format';
import { contextUsage } from '../state/selectors';
import * as store from '../state/store';
import * as actions from '../state/actions';
import {
  invalidateFacts,
  loadFactsFor,
  sessionFacts,
  type SessionFacts,
} from '../state/sessionFacts';
import { Dialog } from './Dialog';
import {
  AddProjectDialog,
  DeleteProjectDialog,
  RenameProjectDialog,
  SessionGlyph,
  projectInitials,
} from './ProjectRail';
import { CloseIcon, KebabIcon, PlusIcon, SearchIcon, SettingsIcon } from './icons';

const DAY = 86_400_000;

/** The rail's one-tap filters, in the mock's order. */
export type SessionFilter = 'all' | 'needs' | 'running' | 'changes';

export const SESSION_FILTERS: ReadonlyArray<{
  id: SessionFilter;
  label: string;
  title: string;
}> = [
  { id: 'all', label: 'All', title: 'Every session in this project' },
  { id: 'needs', label: 'Needs you', title: 'Sessions with unread news or a failed run' },
  { id: 'running', label: 'Running', title: 'Sessions with a run in flight' },
  { id: 'changes', label: 'Changes', title: 'Sessions that touched files' },
];

/** Does one session pass the rail's filter? */
export function passesFilter(
  session: SessionSummary,
  filter: SessionFilter,
  unread: number,
): boolean {
  if (filter === 'all') return true;
  if (filter === 'needs') {
    return unread > 0 || session.last_run_status === 'failed' || session.last_run_status === 'interrupted';
  }
  if (filter === 'running') return session.running;
  // "Changes" is about what the session *did*, and only a session the server has
  // described can answer it. Unknown is not "no": it stays visible under All.
  return false;
}

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
  /** The facts read for this session, or `null` while they are unknown. */
  facts: SessionFacts | null;
  /** Unread notifications for this session. */
  unreadCount: number;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onRequestDelete: (session: SessionSummary, trigger: HTMLElement | null) => void;
}

function SessionRow({
  session,
  active,
  now,
  facts,
  unreadCount,
  onSelect,
  onRename,
  onRequestDelete,
}: SessionRowProps): JSX.Element {
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
  // The mock's row classes drive the status rail: a run in flight, a failure,
  // and a session waiting on the reader (unread news) each paint it differently.
  const rowState = [
    session.running ? 'running' : '',
    session.last_run_status === 'failed' ? 'failed' : '',
    unreadCount > 0 ? 'needs' : '',
  ]
    .filter(Boolean)
    .join(' ');
  const glyphs = facts?.tools ?? [];
  const turns = facts ? facts.turns : null;

  return (
    <li
      class={`session-row${active ? ' is-active' : ''}${rowState ? ` ${rowState}` : ''}`}
      data-session-id={session.id}
    >
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
          <span class="session-head">
            <span class="session-title">{session.title || 'Untitled session'}</span>
            {glyphs.length > 0 ? (
              <span class="session-glyphs" aria-hidden="true">
                {glyphs.map((kind) => (
                  <SessionGlyph key={kind} kind={kind} />
                ))}
              </span>
            ) : null}
            {unreadCount > 0 ? (
              <span
                class="session-unread"
                aria-label={`${unreadCount} unread notification${unreadCount === 1 ? '' : 's'}`}
                title={`${unreadCount} unread notification${unreadCount === 1 ? '' : 's'}`}
              >
                {unreadCount > 9 ? '9+' : unreadCount}
              </span>
            ) : null}
            {statusTone ? <span class={`status-dot status-${statusTone}`} aria-hidden="true" /> : null}
          </span>
          <span class="session-meta">
            {session.running ? <span class="sr-only">Running. </span> : null}
            <span>{relativeTime(session.updated_at, now)}</span>
            <span class="meta-dot" aria-hidden="true" />
            <span title={`${session.message_count} messages`}>{formatCount(session.message_count)}</span>
            {/* The mock's second line is the session's fingerprint. Ours is the
             * one aggregate the server reports per session: how many turns it
             * took, and how many tool calls it made to get there. A session the
             * server has not described yet shows neither rather than a zero. */}
            {turns !== null && turns > 0 ? (
              <span class="session-turns">{turns === 1 ? '1 turn' : `${turns} turns`}</span>
            ) : facts && facts.calls !== null ? (
              <span class="session-turns">
                {facts.calls === 1 ? '1 call' : `${facts.calls} calls`}
              </span>
            ) : null}
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
  /** The chip filter in force; `all` shows everything. */
  filter?: SessionFilter;
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
  filter = 'all',
  onCreate,
  onSelect,
  onRename,
  onDelete,
}: SessionListProps): JSX.Element {
  const [pendingDelete, setPendingDelete] = useState<{
    session: SessionSummary;
    trigger: HTMLElement | null;
  } | null>(null);
  // The per-session fingerprints under each title. Read lazily and bounded (see
  // `state/sessionFacts.ts`): the rail still renders from the list endpoint, so
  // a slow or refused read only costs the second line.
  const facts = sessionFacts.value;

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
  const unreadCounts = store.unread.value;
  const visible = sessions.filter((session) => {
    if (!passesFilter(session, filter, unreadCounts[session.id] ?? 0)) return false;
    return needle === '' || (session.title || '').toLowerCase().includes(needle);
  });

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
            {query.trim() === '' ? (
              <>No session in this project matches the {filter} filter right now.</>
            ) : (
              <>
                Nothing in this project matches <strong>{query.trim()}</strong>. Searches only look at
                session titles.
              </>
            )}
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
                  facts={facts[session.id] ?? null}
                  // The dot is the surface that works when a desktop notification
                  // could not be shown (no permission, no secure context, a mobile
                  // browser), and it clears when the session is opened.
                  unreadCount={store.unread.value[session.id] ?? 0}
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

/**
 * The project switcher, rendered inside the sessions sheet on a phone.
 *
 * On desktop the far-left rail owns this: one square per project, plus add and
 * the project menu. On a phone that rail cost 56 of 440px of every screen to
 * show two glyphs, so it is hidden and this horizontal strip takes over — the
 * same projects, the same dialogs, the same actions, in a row instead of a
 * column. It is not rendered at all above 1023px (CSS hides it there too, so the
 * DOM and the layout agree).
 */
function ProjectSwitcher(): JSX.Element {
  const projects = store.projects.value;
  const activeId = store.currentProjectId.value;
  const activeProject = store.currentProject.value;
  const [dialog, setDialog] = useState<'add' | 'rename' | 'delete' | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuWrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: MouseEvent): void => {
      if (!menuWrapRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener('mousedown', onPointerDown);
    return () => document.removeEventListener('mousedown', onPointerDown);
  }, [menuOpen]);

  // A project that disappears (deleted elsewhere) must not leave the menu open
  // pointing at nothing.
  useEffect(() => {
    if (!activeProject) setMenuOpen(false);
  }, [activeProject]);

  const copyPath = async (): Promise<void> => {
    const path = activeProject?.path ?? '';
    if (path === '') return;
    const copied = await copyText(path);
    store.pushToast(
      copied ? 'Project path copied.' : 'Could not copy the path.',
      copied ? 'success' : 'error',
    );
  };

  return (
    <div class="project-switcher">
      <div class="project-switcher-head">
        <p class="eyebrow">Projects</p>
        <div class="project-switcher-actions">
          <button
            type="button"
            class="side-mini-btn"
            aria-label="Add project"
            title="Add project"
            onClick={() => setDialog('add')}
          >
            <PlusIcon size={15} />
          </button>
          <div class="project-switcher-menu-wrap" ref={menuWrapRef}>
            <button
              type="button"
              class="side-mini-btn"
              aria-label="Project options"
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              title="Project options"
              disabled={!activeProject}
              onClick={() => setMenuOpen((open) => !open)}
            >
              <KebabIcon size={15} />
            </button>
            {menuOpen && activeProject ? (
              <div
                class="rail-menu project-switcher-menu"
                role="menu"
                aria-label={`Project options for ${activeProject.name}`}
              >
                <div class="rail-menu-meta">
                  <span class="rail-menu-name">{activeProject.name}</span>
                  <span class="rail-menu-path">{activeProject.path}</span>
                </div>
                <button
                  type="button"
                  role="menuitem"
                  class="menu-item"
                  onClick={() => {
                    setMenuOpen(false);
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
                    setMenuOpen(false);
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
                    setMenuOpen(false);
                    actions.openSettings();
                  }}
                >
                  Project settings…
                </button>
                <button
                  type="button"
                  role="menuitem"
                  class="menu-item"
                  onClick={() => {
                    setMenuOpen(false);
                    setDialog('rename');
                  }}
                >
                  Rename…
                </button>
                <button
                  type="button"
                  role="menuitem"
                  class="menu-item menu-item-danger"
                  onClick={() => {
                    setMenuOpen(false);
                    setDialog('delete');
                  }}
                >
                  Delete project…
                </button>
              </div>
            ) : null}
          </div>
          <button
            type="button"
            class="side-mini-btn"
            aria-label="Close sessions panel"
            data-autofocus
            onClick={actions.closeSheets}
          >
            <CloseIcon size={15} />
          </button>
        </div>
      </div>

      <ul class="project-switcher-list">
        {projects.map((item) => {
          const active = item.id === activeId;
          return (
            <li key={item.id}>
              <button
                type="button"
                class={`project-switcher-item${active ? ' is-active' : ''}`}
                aria-label={`Switch to ${item.name} (${item.path})`}
                aria-current={active ? 'true' : undefined}
                title={item.path}
                onClick={() => {
                  if (!active) void actions.switchProject(item.id);
                }}
              >
                <span class="project-switcher-initials" aria-hidden="true">
                  {projectInitials(item.name, item.path)}
                </span>
                <span class="project-switcher-name">{item.name}</span>
              </button>
            </li>
          );
        })}
      </ul>

      <AddProjectDialog open={dialog === 'add'} onClose={() => setDialog(null)} />
      <RenameProjectDialog
        project={activeProject}
        open={dialog === 'rename'}
        onClose={() => setDialog(null)}
      />
      <DeleteProjectDialog
        project={activeProject}
        open={dialog === 'delete'}
        onClose={() => setDialog(null)}
      />
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
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>('all');
  const [menu, setMenu] = useState<ProjectMenuState | null>(null);
  useFocusTrap(railRef, isNarrow && isSheetOpen, actions.closeSheets);
  const unread = store.unread.value;

  const filterCounts = useMemo<Record<SessionFilter, number>>(() => {
    const counts: Record<SessionFilter, number> = { all: sessions.length, needs: 0, running: 0, changes: 0 };
    for (const session of sessions) {
      if (passesFilter(session, 'needs', unread[session.id] ?? 0)) counts.needs += 1;
      if (session.running) counts.running += 1;
    }
    return counts;
  }, [sessions, unread]);

  // A filter whose count drops to zero (the last run finished) would strand the
  // reader on an empty list with no obvious way back.
  useEffect(() => {
    if (sessionFilter !== 'all' && filterCounts[sessionFilter] === 0) setSessionFilter('all');
  }, [sessionFilter, filterCounts]);

  // The per-session fingerprints under each title. Bounded and coalesced by the
  // loader, and keyed by the newest-first list, so a long rail reads the newest
  // rows first and never more than the cap.
  const sessionKey = sessions.map((session) => session.id).join(',');
  const projectId = project?.id ?? null;
  const previousRunning = useRef<Record<string, boolean>>({});
  // A project switch must not let a late response from the previous project
  // paint the new rail. The loader also guards the response, but clearing here
  // removes old fingerprints immediately.
  useEffect(() => {
    invalidateFacts();
  }, [projectId]);
  useEffect(() => {
    const finished = sessions
      .filter((session) => previousRunning.current[session.id] === true && !session.running)
      .map((session) => session.id);
    if (finished.length > 0) invalidateFacts(finished);
    previousRunning.current = Object.fromEntries(sessions.map((session) => [session.id, session.running]));
  }, [sessionKey, sessions]);
  // The loader owns TTL, concurrency and request coalescing. This effect only
  // changes when the list identity changes, so status updates do not refetch.
  useEffect(() => {
    if (sessionKey === '') return;
    void loadFactsFor(sessionKey.split(','));
  }, [sessionKey, projectId]);

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

  const contextUsageValue = contextUsage(store.chat.value, store.activeContextWindow.value);
  const contextPercent = contextUsageValue
    ? Math.round(contextUsageValue.fraction * 100)
    : null;
  const promptTokens = contextUsageValue?.tokens ?? null;
  const contextWindow = contextUsageValue?.window ?? null;

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
        {isNarrow ? <ProjectSwitcher /> : null}
        {/* The switcher above owns the project story on a phone — name, every
         * project, the add button, the project menu — so this head is a duplicate
         * there. It is not rendered at all rather than hidden in CSS, so the
         * sheet has one `Project options` button rather than two. */}
        {isNarrow ? null : (
          <div class="side-head">
            <div class="project-heading">
              <span class="project-title-glyph" aria-hidden="true">
                {(project?.name ?? 'k').trim().slice(0, 1).toLowerCase() || 'k'}
              </span>
              <h2 class="project-title" title={project?.path ?? 'No project'}>
                <span class="project-title-text">{project?.name ?? 'No project'}</span>
                <span class="path-hint">{project?.path ?? 'No project selected'}</span>
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
              </div>
            </div>
          </div>
        )}

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

        {/* The mock's `.chips`: one-tap filters over the list, each with its own
         * count, and a chip that would filter to nothing is not offered. */}
        <div class="session-chips" role="group" aria-label="Filter sessions">
          {SESSION_FILTERS.map((filter) => {
            const count = filterCounts[filter.id] ?? 0;
            if (filter.id !== 'all' && count === 0) return null;
            return (
              <button
                key={filter.id}
                type="button"
                class={`filter-chip${sessionFilter === filter.id ? ' is-on' : ''}${
                  filter.id === 'needs' ? ' is-alert' : ''
                }`}
                aria-pressed={sessionFilter === filter.id}
                title={filter.title}
                onClick={() => setSessionFilter(filter.id)}
              >
                {filter.label}
                <span class="filter-chip-count">{count}</span>
              </button>
            );
          })}
        </div>

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
            filter={sessionFilter}
            onCreate={() => void actions.createSession()}
            onSelect={actions.switchSession}
            onRename={actions.renameSessionById}
            onDelete={actions.deleteSessionById}
          />
        </nav>

        <div class="side-footer">
          {/* The mock's "Workspace context" meter. Ours is the same number the
           * composer's context pill shows - the newest call's prompt over the
           * model's window - so the two can never disagree. It renders nothing
           * when either side is unknown, because a bar drawn from a guess is
           * worse than no bar. */}
          {contextPercent !== null ? (
            <div class="side-meter" title={`${promptTokens?.toLocaleString() ?? '?'} of ${contextWindow?.toLocaleString() ?? '?'} prompt tokens used`}>
              <div class="side-meter-top">
                <span>Context</span>
                <span>{contextPercent}%</span>
              </div>
              <div class="side-meter-bar">
                <i
                  style={`width:${Math.max(2, Math.min(100, contextPercent))}%`}
                  data-tone={contextPercent >= 90 ? 'hot' : contextPercent >= 70 ? 'warn' : 'ok'}
                />
              </div>
            </div>
          ) : null}
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