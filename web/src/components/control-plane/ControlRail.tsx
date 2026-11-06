import type { JSX, RefObject } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { ControlChatSummary, SessionSummary } from '../../api/types';
import { KebabIcon } from '../icons';
import {
  type Filter,
  agentBadgeTitle,
  chatTitle,
  initials,
  sessionStatus,
  shortAgo,
  tone,
} from './controlModel';
import '../../styles/control-rail.css';

const FILTERS: { id: Filter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'finished', label: 'Finished' },
];

export interface ControlRailProps {
  /** The rail region itself, focused when the plane opens (see the shell's mount effect). */
  railRef: RefObject<HTMLElement>;
  /** The control chats, newest first - the same list the hero counts. */
  chats: ControlChatSummary[];
  /** Every row the last `/api/sessions` read returned, control chats included. */
  sessions: SessionSummary[];
  loading: boolean;
  listError: string | null;
  /** The id of the open session, or `null` when the overview is showing. */
  selected: string | null;
  filter: Filter;
  onFilter: (filter: Filter) => void;
  query: string;
  onQuery: (query: string) => void;
  /** How many projects are registered; the footer says so. */
  projectCount: number;
  /** A session's project name, or "No project". */
  projectName: (id: string | null | undefined) => string;
  onRefresh: () => void;
  onNewChat: () => void;
  onNavigate: (session: { id: string; project_id: string | null }) => void;
  onRename: (session: SessionSummary) => void;
  onDelete: (session: SessionSummary) => void;
}

interface RowActions {
  onRename: (session: SessionSummary) => void;
  onDelete: (session: SessionSummary) => void;
}

/** The kebab on a row: rename and delete, without a trip into the project. */
function RowMenu({ session, title, onRename, onDelete }: {
  session: SessionSummary;
  title: string;
  onRename: (session: SessionSummary) => void;
  onDelete: (session: SessionSummary) => void;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (target instanceof Node && root.current && !root.current.contains(target)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);
  return <div class="cp-row-menu" ref={root}>
    <button
      type="button"
      ref={trigger}
      class="cp-icon cp-row-menu-trigger"
      aria-label={`Actions for ${title}`}
      aria-haspopup="menu"
      aria-expanded={open}
      onClick={() => setOpen((value) => !value)}
    >
      <KebabIcon size={15} />
    </button>
    {open ? <div class="session-menu cp-row-menu-list" role="menu" aria-label={`Actions for ${title}`}>
      <button
        type="button"
        role="menuitem"
        class="menu-item"
        onClick={() => { setOpen(false); onRename(session); }}
      >Rename</button>
      <button
        type="button"
        role="menuitem"
        class="menu-item menu-item-danger"
        onClick={() => { setOpen(false); onDelete(session); }}
      >Delete…</button>
    </div> : null}
  </div>;
}

/**
 * The rail: every conversation on the server, one browser column.
 *
 * The rail is two groups. **Control chats** are conversations that act on *other*
 * projects; they carry their own model and subagent counts and are listed first.
 * **Sessions** are the ordinary project conversations, with search, filters and
 * running-first ordering. The two never overlap: a session the server marks
 * `control` is never drawn as a project row, whatever its id says.
 */
export function ControlRail({
  railRef,
  chats,
  sessions,
  loading,
  listError,
  selected,
  filter,
  onFilter,
  query,
  onQuery,
  projectCount,
  projectName,
  onRefresh,
  onNewChat,
  onNavigate,
  onRename,
  onDelete,
}: ControlRailProps): JSX.Element {
  const sorting = (a: SessionSummary, b: SessionSummary): number =>
    Number(b.running) - Number(a.running) || b.updated_at - a.updated_at;
  const ordered = useMemo(() => [...sessions].sort(sorting), [sessions]);
  // The `control` flag is what makes a session a chat, so it is what keeps it out
  // of the project list — the id set is only a second guard for the case where
  // `/api/sessions` has not learned about the flag but the snapshot has.
  const controlIds = new Set(chats.map((chat) => chat.id));
  const projectSessions = ordered.filter(
    (session) => session.control !== true && !controlIds.has(session.id),
  );
  const needle = query.trim().toLowerCase();
  const filtered = projectSessions.filter((session) => {
    if (filter === 'running' && !session.running) return false;
    if (filter === 'finished' && (session.running || session.last_run_status !== 'completed')) return false;
    if (needle === '') return true;
    return `${session.title} ${projectName(session.project_id)}`.toLowerCase().includes(needle);
  });
  const counts: Record<Filter, number> = {
    all: projectSessions.length,
    running: projectSessions.filter((session) => session.running).length,
    finished: projectSessions.filter((session) => !session.running && session.last_run_status === 'completed').length,
  };
  const groups: { key: string; label: string; items: SessionSummary[] }[] = [
    { key: 'running', label: 'Running now', items: filtered.filter((session) => session.running) },
    { key: 'recent', label: filter === 'all' ? 'Recently active' : 'Sessions', items: filtered.filter((session) => !session.running) },
  ].filter((group) => group.items.length > 0);

  const row = (session: SessionSummary, rowActions: RowActions): JSX.Element => {
    const status = sessionStatus(session);
    const project = projectName(session.project_id);
    const title = chatTitle(session);
    return <li key={session.id} class="cp-row-item">
      <button
        type="button"
        class={`cp-row${selected === session.id ? ' selected' : ''}${session.running ? ' is-running' : ''}`}
        aria-pressed={selected === session.id}
        title={title}
        onClick={() => onNavigate(session)}
      >
        <span class="cp-glyph" data-tone={tone(session.project_id)} aria-hidden="true">{initials(project)}</span>
        <span class="cp-row-main">
          <span class="cp-row-title"><span class="cp-row-title-text">{title}</span></span>
          <span class="cp-row-meta">
            <span>{project}</span>
            <span class="cp-dot">·</span>
            <span>{shortAgo(session.updated_at)}</span>
            <span class="cp-dot">·</span>
            <span>{session.message_count.toLocaleString()} msgs</span>
          </span>
        </span>
        <span class={`cp-state ${status.tone}`} title={`Latest state: ${status.label}`}>
          <i aria-hidden="true" />{status.label}
        </span>
      </button>
      <RowMenu session={session} title={title} {...rowActions} />
    </li>;
  };

  const controlRow = (chat: ControlChatSummary, rowActions: RowActions): JSX.Element => {
    // A chat whose run is parked on an approval question is not "Running" in any
    // useful sense: it is waiting on a person. The word says so.
    const status = (chat.pending_approvals ?? 0) > 0
      ? { label: 'Needs you', tone: 'warn' as const }
      : sessionStatus(chat);
    const title = chatTitle(chat);
    return <li key={chat.id} class="cp-row-item">
      <button
        type="button"
        class={`cp-row${selected === chat.id ? ' selected' : ''}${chat.running ? ' is-running' : ''}`}
        aria-pressed={selected === chat.id}
        title={title}
        onClick={() => onNavigate(chat)}
      >
        <span class="cp-glyph control" aria-hidden="true">k.</span>
        <span class="cp-row-main">
          {/* Only the name lives on the title line. A chat's model is not part of
              its identity - the composer says which model will answer - so the
              title gets the whole line and truncates on its own. */}
          <span class="cp-row-title">
            <span class="cp-row-title-text">{title}</span>
          </span>
          <span class="cp-row-meta">
            {chat.agents.total > 0 ? <span
              class="cp-badge cp-badge-agents"
              title={agentBadgeTitle(chat.agents)}
            >{chat.agents.live}/{chat.agents.total} agents</span> : null}
            <span class="cp-row-time">{shortAgo(chat.updated_at)}</span>
          </span>
        </span>
        <span class={`cp-state ${status.tone}`} title={`Latest state: ${status.label}`}>
          <i aria-hidden="true" />{status.label}
        </span>
      </button>
      <RowMenu session={chat} title={title} {...rowActions} />
    </li>;
  };

  const rowActions: RowActions = { onRename, onDelete };

  return <aside ref={railRef} class="cp-browser" aria-label="Sessions across projects" tabIndex={-1}>
    <header class="cp-browser-head">
      <div class="cp-brand">
        <span class="cp-brand-mark" aria-hidden="true">k<i>.</i></span>
        <div><h1>Control plane</h1><p>Every project, one view</p></div>
      </div>
      <button type="button" class="cp-icon" title="Refresh sessions" aria-label="Refresh sessions" onClick={onRefresh}>⟳</button>
    </header>
    <div class="cp-search">
      <span aria-hidden="true">⌕</span>
      <input
        type="search"
        value={query}
        placeholder="Search sessions or projects…"
        aria-label="Search sessions or projects"
        onInput={(event) => onQuery(event.currentTarget.value)}
      />
    </div>
    <div class="cp-filters" role="group" aria-label="Filter sessions">
      {FILTERS.map((item) => <button
        type="button"
        key={item.id}
        aria-pressed={filter === item.id}
        onClick={() => onFilter(item.id)}
      >{item.label}<span>{counts[item.id]}</span></button>)}
    </div>
    <div class="cp-scroll">
      <section aria-label="Control chats">
        <div class="cp-group cp-group-action">
          <span>Control chats</span>
          <button type="button" class="cp-new-chat" onClick={onNewChat} aria-label="New control chat">＋ New</button>
        </div>
        {chats.length === 0
          ? <button type="button" class="cp-control-entry" onClick={onNewChat}>
              <span class="cp-spark" aria-hidden="true">✳</span>
              <span class="cp-control-copy"><strong>New control chat</strong><small>Act on any project from one place</small></span>
              <span class="cp-chev" aria-hidden="true">→</span>
            </button>
          : <ul class="cp-list">{chats.map((chat) => controlRow(chat, rowActions))}</ul>}
      </section>
      {loading && sessions.length === 0 ? <p class="cp-note">Loading sessions…</p> : null}
      {/* The project-session group keeps a stable name so a screen reader (and
          the browser verification script) can tell which group a row is in. */}
      <section aria-label="Sessions">
        <div class="cp-group"><span>Sessions</span><span class="cp-group-count">{projectSessions.length}</span></div>
        {!loading && projectSessions.length === 0 && !listError
          ? <p class="cp-note">No sessions yet. Start one from control chat.</p>
          : null}
        {groups.map((group) => <section key={group.key} aria-label={group.label}>
          {/* The sub-group count is only worth drawing when it says something
              the outer "Sessions" count does not: one group with every session
              in it repeated the same number twice in a row. */}
          <div class="cp-group cp-group-sub">
            <span>{group.label}</span>
            {groups.length > 1 ? <span class="cp-group-count">{group.items.length}</span> : null}
          </div>
          <ul class="cp-list">{group.items.map((session) => row(session, rowActions))}</ul>
        </section>)}
        {!loading && filtered.length === 0 && query.trim() !== '' ? <p class="cp-note">No sessions match “{query}”.</p> : null}
      </section>
    </div>
    <footer class="cp-browser-foot">
      <span class="cp-live"><i aria-hidden="true" />Live</span>
      <span class="cp-foot-projects">{projectCount} projects</span>
    </footer>
  </aside>;
}
