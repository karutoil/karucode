import { useEffect, useId, useMemo, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { useFocusTrap } from '../lib/dom';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../lib/menu';
import { copyText } from '../lib/clipboard';
import { modKey } from '../lib/platform';
import type { SessionSummary } from '../api/types';
import { formatCount, relativeTime } from '../lib/format';
import { contextUsage, sessionStatusView } from '../state/selectors';
import * as store from '../state/store';
import * as actions from '../state/actions';
import {
  invalidateFacts,
  loadFactsFor,
  sessionFacts,
  type SessionFacts,
} from '../state/sessionFacts';
import { Dialog } from './Dialog';
import { controlPlaneOpen } from '../state/controlPlane';
import * as railPeek from '../state/railPeek';
import {
  AddProjectDialog,
  DeleteProjectDialog,
  ProjectGlyph,
  RenameProjectDialog,
  unreadLabel,
} from './ProjectRail';
import {
  ChevronDownIcon,
  CloseIcon,
  FolderIcon,
  KebabIcon,
  PanelIcon,
  PlusIcon,
  SearchIcon,
  SettingsIcon,
} from './icons';
import '../styles/workbench-navigation.css';

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
  const menu = useMenu();
  const inputRef = useRef<HTMLInputElement>(null);
  const skipBlur = useRef(false);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  useEffect(() => {
    if (!editing) setDraft(session.title);
  }, [session.title, editing]);

  /** The title is selectable text, so the native menu could copy it; ours must
   *  not be a regression, and the row's own spelling is the one worth keeping. */
  const copyTitle = async (): Promise<void> => {
    const title = session.title || 'Untitled session';
    const copied = await copyText(title);
    store.pushToast(copied ? 'Session title copied.' : 'Could not copy the title.',
      copied ? 'success' : 'error');
  };

  const startRename = (): void => {
    skipBlur.current = false;
    menu.close();
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

  // The word and the colour both come from `sessionStatusView` (`state/selectors.ts`),
  // the same helper the control plane reads: a failed run must not be spelled one
  // way here and another way there.
  const { label: statusLabel, tone: statusTone } = sessionStatusView(session);
  // The mock's row classes drive the status rail: a run in flight, a failure,
  // and a session waiting on the reader (unread news) each paint it differently.
  const rowState = [
    session.running ? 'running' : '',
    session.last_run_status === 'failed' ? 'failed' : '',
    unreadCount > 0 ? 'needs' : '',
  ]
    .filter(Boolean)
    .join(' ');
  const turns = facts ? facts.turns : null;
  // Tool names and turn/call counts stay discoverable without making every row
  // repeat the same fingerprint. The visible line is reserved for state, recency
  // and the server's message count.
  const details = [
    `${session.message_count} messages`,
    turns !== null && turns > 0 ? `${turns} ${turns === 1 ? 'turn' : 'turns'}` : null,
    facts && facts.calls !== null ? `${facts.calls} ${facts.calls === 1 ? 'tool call' : 'tool calls'}` : null,
    facts && facts.tools.length > 0 ? `Tools: ${facts.tools.join(', ')}` : null,
  ].filter((detail): detail is string => detail !== null).join(' · ');

  return (
    <li
      class={`session-row session-row-quiet${active ? ' is-active' : ''}${rowState ? ` ${rowState}` : ''}`}
      data-session-id={session.id}
      {...contextMenuProps(menu, () => !editing, () => menu.triggerRef.current)}
    >
      {editing ? (
        <form
          class="session-rename"
          onSubmit={(event) => {
            event.preventDefault();
            commitRename();
          }}
        >
          {/* `data-escape-local`: Escape cancels the rename here; the sheet's
            * focus trap defers to this control instead of closing the sheet. */}
          <input
            ref={inputRef}
            class="session-rename-input"
            value={draft}
            aria-label={`Rename session ${session.title}`}
            maxLength={200}
            data-escape-local
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
          title={details}
        >
          <span class="session-head">
            <span class="session-title">{session.title || 'Untitled session'}</span>
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
            {statusLabel ? (
              <>
                <span class={`session-state session-state-${statusTone}`}>{statusLabel}</span>
                <span class="meta-dot" aria-hidden="true" />
              </>
            ) : null}
            <span>{relativeTime(session.updated_at, now)}</span>
            <span class="meta-dot" aria-hidden="true" />
            <span title={`${session.message_count} messages`}>{formatCount(session.message_count)}</span>
            {session.last_run_status === 'failed' ? <span class="sr-only">Last run failed.</span> : null}
            {session.last_run_status === 'interrupted' ? (
              <span class="sr-only">Last run interrupted.</span>
            ) : null}
          </span>
        </button>
      )}

      {!editing ? (
        <div class="session-menu-wrap">
          <button
            type="button"
            class="icon-button session-menu-trigger"
            aria-label={`Actions for ${session.title || 'untitled session'}`}
            {...menu.triggerProps}
          >
            <KebabIcon size={15} />
          </button>
          <Menu
            menu={menu}
            label={`Session actions for ${session.title}`}
            float={Boolean(menu.pointer)}
            class={menu.pointer ? undefined : 'session-menu'}
          >
            <MenuItem menu={menu} action={{ label: 'Rename', onSelect: startRename }} />
            <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle() }} />
            <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle() }} />
            <MenuItem
              menu={menu}
              action={{
                label: 'Delete…',
                danger: true,
                onSelect: () => onRequestDelete(session, menu.triggerRef.current),
              }}
            />
          </Menu>
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
 * One project switcher for both navigation layouts.
 *
 * The ordinary wide Workbench uses a compact active-project disclosure in this
 * column; the collapsed desktop mode still uses ProjectRail's real tiles and
 * RailPeek anchors. A phone keeps the existing full horizontal project list in
 * the sessions sheet. Both modes share the store, dialogs and actual commands.
 */
interface ProjectSwitcherProps {
  /** Wide view: a compact disclosure in the one navigation column. */
  desktop?: boolean;
}

/** Hover intent for the desktop project dropdown: crossing the trigger on the way
 *  to the composer must not flash a list, but the pause before opening should be
 *  short enough to read as "it wants to open". */
export const PROJECTS_HOVER_OPEN_MS = 120;
/** The pointer crossing the gap between the trigger and the panel takes a beat;
 *  the panel waits this long before it treats the exit as leaving. */
export const PROJECTS_HOVER_CLOSE_MS = 200;

function ProjectSwitcher({ desktop = false }: ProjectSwitcherProps): JSX.Element {
  const projects = store.projects.value;
  const activeId = store.currentProjectId.value;
  const activeProject = store.currentProject.value;
  const unreadByProject = store.unreadByProject.value;
  const [dialog, setDialog] = useState<'add' | 'rename' | 'delete' | null>(null);
  const menu = useMenu();
  const [projectsOpen, setProjectsOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const disclosureRef = useRef<HTMLButtonElement>(null);
  const disclosureId = useId();
  /** Whether resting on the trigger opens the list, or only a press does. */
  const hoverOpens = store.projectMenuOpen.value === 'hover';
  /** Hover timers for the desktop dropdown, plus whether the pointer is what
   *  opened it — a hover must never move focus. */
  const hoverOpenTimer = useRef<number | null>(null);
  const hoverCloseTimer = useRef<number | null>(null);
  const openedByHover = useRef(false);

  const clearHoverTimers = (): void => {
    if (hoverOpenTimer.current !== null) {
      window.clearTimeout(hoverOpenTimer.current);
      hoverOpenTimer.current = null;
    }
    if (hoverCloseTimer.current !== null) {
      window.clearTimeout(hoverCloseTimer.current);
      hoverCloseTimer.current = null;
    }
  };

  const closeProjects = (restoreFocus = true): void => {
    clearHoverTimers();
    openedByHover.current = false;
    setProjectsOpen(false);
    if (restoreFocus) {
      window.setTimeout(() => {
        if (disclosureRef.current?.isConnected) disclosureRef.current.focus();
      }, 0);
    }
  };

  /** The click and keyboard path: Enter/Space on the trigger lands here too. */
  const toggleProjects = (): void => {
    if (projectsOpen) {
      closeProjects(false);
      return;
    }
    clearHoverTimers();
    openedByHover.current = false;
    setProjectsOpen(true);
  };

  /** Resting on the trigger opens the dropdown (the way a menu bar does).
   *
   *  Only in `hover` mode: the setting exists because this is the one control in
   *  the rail that opens under a pointer which was only crossing it on the way to
   *  the composer. In `click` mode the trigger is an ordinary disclosure and the
   *  pointer may pass over it freely. */
  const hoverOpenProjects = (): void => {
    if (!desktop || !hoverOpens) return;
    clearHoverTimers();
    if (projectsOpen) return;
    hoverOpenTimer.current = window.setTimeout(() => {
      hoverOpenTimer.current = null;
      openedByHover.current = true;
      setProjectsOpen(true);
    }, PROJECTS_HOVER_OPEN_MS);
  };

  /** The pointer is back inside (the panel): cancel a pending close. */
  const holdProjects = (): void => {
    if (hoverCloseTimer.current !== null) {
      window.clearTimeout(hoverCloseTimer.current);
      hoverCloseTimer.current = null;
    }
  };

  /** Leaving the trigger or the panel closes it, after the grace period.
   *
   *  A `click` menu stays open until it is dismissed: closing it because the
   *  pointer moved would be the hover behaviour the setting just turned off. */
  const hoverCloseProjects = (): void => {
    if (!desktop || !hoverOpens) return;
    if (hoverOpenTimer.current !== null) {
      // Left before it opened: never open at all.
      window.clearTimeout(hoverOpenTimer.current);
      hoverOpenTimer.current = null;
    }
    if (!projectsOpen) return;
    holdProjects();
    hoverCloseTimer.current = window.setTimeout(() => {
      hoverCloseTimer.current = null;
      openedByHover.current = false;
      setProjectsOpen(false);
    }, PROJECTS_HOVER_CLOSE_MS);
  };

  useEffect(() => clearHoverTimers, []);

  useEffect(() => {
    if (!desktop || !projectsOpen) return;
    const onPointerDown = (event: MouseEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) closeProjects(false);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      // The nested options menu owns its Escape first. Only once that is closed
      // does the next Escape dismiss the projects disclosure.
      if (event.key === 'Escape' && !menu.open) {
        event.preventDefault();
        event.stopPropagation();
        closeProjects();
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [desktop, projectsOpen, menu.open]);

  useEffect(() => {
    if (!desktop || !projectsOpen) return;
    // A hover opens the list for the eyes only. Moving focus under a resting
    // pointer would pull the caret out of whatever the reader was typing in.
    if (openedByHover.current) {
      openedByHover.current = false;
      return;
    }
    const active = rootRef.current?.querySelector<HTMLButtonElement>(
      '.project-switcher-item[aria-current="true"]',
    );
    const first = rootRef.current?.querySelector<HTMLButtonElement>('.project-switcher-item');
    (active ?? first)?.focus();
  }, [desktop, projectsOpen]);

  // A project that disappears (deleted elsewhere) must not leave the menu open
  // pointing at nothing.
  useEffect(() => {
    if (!activeProject) menu.close();
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

  const activeUnread = activeProject ? unreadByProject[activeProject.id] ?? 0 : 0;

  /** The project rows, shared by the desktop dropdown (which floats over the rail)
   *  and the phone sheet (where the list is part of the sheet). */
  const projectList = (
    <>
      <ul class="project-switcher-list">
        {projects.map((item) => {
          const active = item.id === activeId;
          const unread = unreadByProject[item.id] ?? 0;
          const unreadNote = unread > 0 ? `, ${unreadLabel(unread)}` : '';
          return (
            <li key={item.id}>
              <button
                type="button"
                class={`project-switcher-item${active ? ' is-active' : ''}`}
                aria-label={`Switch to ${item.name} (${item.path})${unreadNote}`}
                aria-current={active ? 'true' : undefined}
                title={`${item.path}${
                  item.session_count > 0
                    ? `\n${item.session_count} ${item.session_count === 1 ? 'session' : 'sessions'}`
                    : ''
                }${unread > 0 ? `\n${unreadLabel(unread)}` : ''}`}
                onClick={() => {
                  controlPlaneOpen.value = false;
                  if (desktop) closeProjects();
                  if (!active) void actions.switchProject(item.id);
                }}
              >
                <ProjectGlyph project={item} />
                <span class="project-switcher-name">{item.name}</span>
                {unread > 0 ? (
                  <span class="workbench-project-unread" aria-hidden="true">
                    {unread > 99 ? '99+' : unread}
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>
      {desktop && projects.length === 0 ? (
        <p class="workbench-projects-empty">No projects yet. Use + to add one.</p>
      ) : null}
    </>
  );

  return (
    <div
      ref={rootRef}
      class={`project-switcher${desktop ? ' project-switcher-desktop' : ''}`}
    >
      <div class={`project-switcher-head${desktop ? ' project-heading' : ''}`}>
        {desktop ? (
          <button
            ref={disclosureRef}
            type="button"
            class="workbench-project-disclosure project-title"
            aria-label={`Projects: ${activeProject?.name ?? 'none selected'}${
              activeUnread > 0 ? `, ${unreadLabel(activeUnread)}` : ''
            }`}
            aria-expanded={projectsOpen}
            aria-controls={disclosureId}
            title={activeProject?.path ?? 'Choose a project'}
            onClick={toggleProjects}
            onMouseEnter={hoverOpenProjects}
            onMouseLeave={hoverCloseProjects}
          >
            {activeProject ? <ProjectGlyph project={activeProject} /> : null}
            <span class="workbench-project-copy">
              <span class="project-title-text">{activeProject?.name ?? 'Choose a project'}</span>
              <span class="path-hint">{activeProject?.path ?? 'No project selected'}</span>
            </span>
            {activeUnread > 0 ? (
              <span class="workbench-project-unread" aria-hidden="true">
                {activeUnread > 99 ? '99+' : activeUnread}
              </span>
            ) : null}
            <ChevronDownIcon size={13} />
          </button>
        ) : (
          <p class="eyebrow">Projects</p>
        )}
        <div class={desktop ? 'side-head-actions' : 'project-switcher-actions'}>
          <button
            type="button"
            class="side-mini-btn"
            aria-label="Add project"
            title="Add project"
            onClick={() => setDialog('add')}
          >
            <PlusIcon size={15} />
          </button>
          <div class="project-switcher-menu-wrap">
            <button
              type="button"
              class="side-mini-btn"
              aria-label="Project options"
              title="Project options"
              disabled={!activeProject}
              {...menu.triggerProps}
            >
              <KebabIcon size={15} />
            </button>
            {activeProject ? (
              <Menu menu={menu} label={`Project options for ${activeProject.name}`} class="rail-menu project-switcher-menu">
                <div class="rail-menu-meta">
                  <span class="rail-menu-name">{activeProject.name}</span>
                  <span class="rail-menu-path">{activeProject.path}</span>
                </div>
                <MenuItem
                  menu={menu}
                  action={{
                    label: 'Copy project path',
                    disabled: !activeProject.path,
                    onSelect: () => void copyPath(),
                  }}
                />
                <MenuItem
                  menu={menu}
                  action={{ label: 'Refresh sessions', onSelect: () => void actions.refreshSessions() }}
                />
                <MenuItem
                  menu={menu}
                  action={{ label: 'Project settings…', onSelect: actions.openSettings }}
                />
                <MenuItem menu={menu} action={{ label: 'Rename…', onSelect: () => setDialog('rename') }} />
                <MenuItem
                  menu={menu}
                  action={{ label: 'Delete project…', danger: true, onSelect: () => setDialog('delete') }}
                />
              </Menu>
            ) : null}
          </div>
          {!desktop || projectsOpen ? (
            <button
              type="button"
              class="side-mini-btn"
              aria-label={desktop ? 'Close project switcher' : 'Close sessions panel'}
              data-autofocus={desktop ? undefined : true}
              onClick={desktop ? () => closeProjects() : actions.closeSheets}
            >
              <CloseIcon size={15} />
            </button>
          ) : null}
        </div>

        {/* The dropdown lives inside the trigger's row so that row is its
         * containing block: it hangs under the trigger, out of flow, and the rail
         * below keeps its geometry instead of being pushed down. */}
        {desktop && projectsOpen ? (
          <div
            id={disclosureId}
            class="workbench-projects-disclosure"
            aria-label="Available projects"
            data-escape-local
            onMouseEnter={holdProjects}
            onMouseLeave={hoverCloseProjects}
            // Tabbing out must take the overlay with it: the rail's next control
            // (Control plane, New session) sits behind this panel, and leaving
            // focus in a list that covers it would hide where the reader landed.
            onKeyDown={(event) => {
              if (event.key === 'Tab') closeProjects(false);
            }}
          >
            {projectList}
          </div>
        ) : null}
      </div>

      {/* A phone keeps the list in the sheet: there is no trigger to hover, and the
       * sheet is the only place projects exist there. */}
      {!desktop ? <div>{projectList}</div> : null}

      {desktop ? (
        <button
          type="button"
          class="workbench-control-plane"
          aria-label="Open control plane"
          aria-pressed={controlPlaneOpen.value}
          onClick={() => {
            closeProjects(false);
            controlPlaneOpen.value = true;
          }}
        >
          <span class="workbench-control-mark" aria-hidden="true">k.</span>
          <span>Control plane</span>
        </button>
      ) : null}

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

/**
 * The collapsed rail's fly-out: one project's sessions, over the chat.
 *
 * `Ctrl/Cmd+B` gives the sessions rail zero width, which is right for a wide
 * conversation and a dead end when all you want is to open a session. Hovering a
 * project tile while the rail is collapsed shows this instead: the same rows, the
 * same rename and delete menus, positioned against the project rail and out of flow
 * so the conversation never resizes to make room. Hovering a *different* project
 * shows that project — the rows come from `state/railPeek.ts`, which reads the list
 * once and caches it, because the store only holds the open project's sessions.
 *
 * It closes when the pointer leaves (after a short grace period, so clipping the
 * edge on the way past does not flash it) and when a row is opened. It is a view,
 * not a mode: nothing is remembered once it is gone.
 */
export function RailPeek({ projectId }: { projectId: string }): JSX.Element {
  const projects = store.projects.value;
  const project = projects.find((candidate) => candidate.id === projectId) ?? null;
  const isCurrent = store.currentProjectId.value === projectId;
  const sessions = isCurrent
    ? store.sessions.value
    : railPeek.peekSessions.value[projectId] ?? [];
  const loading = isCurrent
    ? store.sessionsLoading.value
    : railPeek.peekLoading.value[projectId] ?? false;
  const error = isCurrent ? null : railPeek.peekError.value[projectId] ?? null;
  const initial = (project?.name ?? 'k').trim().slice(0, 1).toLowerCase() || 'k';

  // The same fingerprints the full rail shows under each title, from the same
  // bounded loader: a peek that dropped them would make the rows twice as wide and
  // half as informative as the list it replaces.
  const sessionKey = sessions.map((session) => session.id).join(',');
  useEffect(() => {
    if (sessionKey === '') return;
    void loadFactsFor(sessionKey.split(','));
  }, [sessionKey, projectId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') railPeek.closePeek();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const openSession = (id: string): void => {
    railPeek.closePeek();
    if (isCurrent) actions.switchSession(id);
    else void actions.openSessionInProject(projectId, id);
  };

  const startDraft = (): void => {
    railPeek.closePeek();
    // `actions.startDraftSession()` drafts in `store.currentProjectId`, which is
    // *not* the project this preview belongs to while the fly-out covers another
    // one. Switch first so the empty state's "New session" starts a conversation
    // in the project the reader is looking at, not silently in the open one.
    void (async () => {
      if (!isCurrent) {
        await actions.switchProject(projectId);
        // A newer navigation may have completed while this switch was pending.
        // Never let the stale continuation draft in that newer project.
        if (store.currentProjectId.value !== projectId) return;
      }
      actions.startDraftSession();
    })();
  };

  // Renames and deletes are by session id, so they work for any project; the cached
  // list has to be re-read because it is not the store's current-project list.
  const refreshPeek = (): void => {
    if (isCurrent) return;
    railPeek.invalidatePeekSessions(projectId);
    void railPeek.loadPeekSessions(projectId, true);
  };

  return (
    <div
      class="rail rail-peek"
      aria-label={`Sessions in ${project?.name ?? 'this project'}`}
      data-project-id={projectId}
      onMouseEnter={railPeek.holdPeek}
      onMouseLeave={() => railPeek.schedulePeekClose()}
    >
      <div class="rail-inner">
        <div class="side-head">
          <div class="project-heading">
            <span class="project-title-glyph" aria-hidden="true">
              {initial}
            </span>
            <h2 class="project-title" title={project?.path ?? 'Unknown project'}>
              <span class="project-title-text">{project?.name ?? 'Unknown project'}</span>
              <span class="path-hint">
                {isCurrent ? project?.path ?? '' : `${sessions.length} ${sessions.length === 1 ? 'session' : 'sessions'}`}
              </span>
            </h2>
            <div class="side-head-actions">
              <button
                type="button"
                class="side-mini-btn"
                aria-label="Pin the session list open"
                aria-keyshortcuts="Control+B Meta+B"
                title={`Pin the session list open — ${modKey()} B`}
                onClick={() => {
                  railPeek.closePeek();
                  // The rail is collapsed by definition while peeking, so this
                  // toggle is what shows it for real.
                  actions.toggleRail();
                }}
              >
                <PanelIcon size={15} />
              </button>
              <button
                type="button"
                class="side-mini-btn"
                aria-label="Close the session preview"
                title="Close (Esc)"
                onClick={railPeek.closePeek}
              >
                <CloseIcon size={15} />
              </button>
            </div>
          </div>
        </div>

        {isCurrent ? (
          <button
            type="button"
            class="new-session"
            onClick={startDraft}
            aria-label={`New session (${modKey()} + N)`}
          >
            <span class="new-left">
              <PlusIcon size={15} /> New session
            </span>
            <span class="shortcut">{modKey()} N</span>
          </button>
        ) : (
          <button
            type="button"
            class="new-session"
            aria-label={`Open ${project?.name ?? 'this project'}`}
            onClick={() => {
              railPeek.closePeek();
              void actions.switchProject(projectId);
            }}
          >
            <span class="new-left">
              <FolderIcon size={15} /> Open this project
            </span>
          </button>
        )}

        <nav class="session-nav" aria-label={`Sessions in ${project?.name ?? 'this project'}`}>
          {error ? (
            <div class="rail-empty" role="status">
              <p class="rail-empty-title">Could not read those sessions</p>
              <p class="rail-empty-body">{error}</p>
              <button
                type="button"
                class="button"
                onClick={() => void railPeek.loadPeekSessions(projectId, true)}
              >
                Try again
              </button>
            </div>
          ) : (
            <SessionList
              sessions={sessions}
              currentId={isCurrent ? store.currentSessionId.value : null}
              now={store.clock.value}
              loading={loading}
              workspace={project?.path}
              onCreate={startDraft}
              onSelect={openSession}
              onRename={async (id, title) => {
                const ok = await actions.renameSessionById(id, title);
                if (ok) refreshPeek();
                return ok;
              }}
              onDelete={async (id) => {
                const ok = await actions.deleteSessionById(id);
                if (ok) refreshPeek();
                return ok;
              }}
            />
          )}
        </nav>
      </div>
    </div>
  );
}

export function SessionsRail(): JSX.Element {
  const sessions = store.sessions.value;
  const currentId = store.currentSessionId.value;
  const isSheetOpen = store.mobileSheet.value === 'sessions';
  const isNarrow = store.narrowViewport.value;
  const project = store.currentProject.value;
  const railRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>('all');
  useFocusTrap(railRef, isNarrow && isSheetOpen, actions.closeSheets);
  const unread = store.unread.value;
  // A newer build exists. The rail says so next to the version because that is
  // where the version is; opening it lands on Settings → Updates. Once that build
  // is on disk (and this process is still running the old one) the same chip
  // becomes a restart reminder, because an install offered again would re-download
  // what is already there.
  const updateState = store.updateStatus.value;
  const latestBuild = updateState?.update_available ? (updateState.latest?.build ?? null) : null;
  const installedBuild = latestBuild !== null && updateState?.last_applied?.to === String(latestBuild);

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

  const contextUsageValue = contextUsage(store.chat.value, store.activeContextWindow.value);
  const contextPercent = contextUsageValue
    ? Math.round(contextUsageValue.fraction * 100)
    : null;
  const promptTokens = contextUsageValue?.tokens ?? null;
  const contextWindow = contextUsageValue?.window ?? null;

  const providerLabel = store.providerConfigured.value
    ? store.activeModelLabel.value ?? 'Provider configured'
    : 'No provider';

  // The collapsed rail's fly-out replaces the rail rather than sitting beside it, so
  // the page never holds two session lists (and two "Sessions" landmarks) at once.
  const peekId = railPeek.peekProjectId.value;
  const peekActive = peekId !== null && !isNarrow && store.railCollapsed.value && !controlPlaneOpen.value;
  if (peekActive && peekId !== null) return <RailPeek projectId={peekId} />;

  return (
    <div
      ref={railRef}
      class={`rail${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Sessions"
      // A plain div has no implicit role, so the rail's landmark (complementary,
      // what an <aside> would have been) and the phone sheet's dialog are both
      // explicit. preact 11's per-element ARIA unions do not allow dialog on an
      // aside, so the element is a div.
      role={isSheetOpen ? 'dialog' : 'complementary'}
      aria-modal={isSheetOpen ? 'true' : undefined}
    >
      <div class="rail-inner">
        {/* One switcher owns project navigation and management in both layouts.
         * Wide screens get a compact disclosure; the phone sheet keeps the full
         * horizontal list and its existing close/control-plane actions. */}
        {isNarrow ? (
          <>
            <ProjectSwitcher />
            <button
              type="button"
              class="new-session"
              onClick={() => {
                actions.closeSheets();
                controlPlaneOpen.value = true;
              }}
            >
              Control plane
            </button>
          </>
        ) : null}
        {isNarrow ? null : (
          <div class="side-head">
            <ProjectSwitcher desktop />
          </div>
        )}

        <button
          type="button"
          class="new-session"
          onClick={() => actions.startDraftSession()}
          aria-label={`New session (${modKey()} + N)`}
        >
          <span class="new-left">
            <PlusIcon size={15} /> New session
          </span>
          <span class="shortcut">{modKey()} N</span>
        </button>

        <label class="side-search">
          <SearchIcon size={15} />
          {/* `data-escape-local`: Escape first clears the query (and a second
            * press closes the sheet on a phone). Without the marker the sheet's
            * capture-phase focus trap would close everything on the first press. */}
          <input
            ref={searchRef}
            type="search"
            value={query}
            placeholder="Find a session…"
            aria-label="Find a session"
            autocomplete="off"
            spellcheck={false}
            data-escape-local
            onInput={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                if (query !== '') {
                  event.preventDefault();
                  setQuery('');
                } else if (isNarrow) {
                  // Nothing left to clear: the next press is the sheet's.
                  event.preventDefault();
                  actions.closeSheets();
                }
              } else if (event.key === 'Enter') {
                const first = sessions.find((session) =>
                  passesFilter(session, sessionFilter, unread[session.id] ?? 0) &&
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
            onCreate={() => actions.startDraftSession()}
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
            <span class="right">
              {store.health.value ? `v${store.health.value.version}` : 'local'}
              {latestBuild !== null ? (
                <button
                  type="button"
                  class="update-dot"
                  title={
                    installedBuild
                      ? `Build ${latestBuild} is installed — restart karucode to run it`
                      : `Build ${latestBuild} is available — open Settings → Updates`
                  }
                  aria-label={
                    installedBuild
                      ? `Restart to run build ${latestBuild}`
                      : `Update available: build ${latestBuild}`
                  }
                  onClick={() => actions.openSettings('updates')}
                >
                  {installedBuild ? '↻' : '↑'}
                  {latestBuild}
                </button>
              ) : null}
            </span>
          </div>
          <div class="settings-row">
            <button
              type="button"
              class="settings-btn"
              onClick={(event) => {
                event.currentTarget.focus();
                actions.openSettings();
              }}
            >
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

    </div>
  );
}