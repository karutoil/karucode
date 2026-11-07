import type { JSX, RefObject } from 'preact';
import { Fragment } from 'preact';
import { useMemo } from 'preact/hooks';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../../lib/menu';
import type { MenuController } from '../../lib/menu';
import { copyText } from '../../lib/clipboard';
import * as store from '../../state/store';
import type { ControlChatSummary, SessionSummary } from '../../api/types';
import { pluralise, relativeTime } from '../../lib/format';
import * as actions from '../../state/actions';
import {
  ArrowRightIcon,
  KebabIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  SettingsIcon,
  SparkIcon,
} from '../icons';
import {
  type Filter,
  type SessionStatus,
  agentBadgeTitle,
  chatTitle,
  initials,
  sessionStatus,
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

/**
 * The kebab on a row: rename and delete, without a trip into the project.
 *
 * The wrapper, trigger and menu are the workbench's own (`.session-menu-wrap`,
 * `.icon-button .session-menu-trigger`, `.session-menu`, `.menu-item`), and the
 * behaviour - open focuses the first item, arrows/Home/End move, Tab leaves,
 * Escape closes and returns focus to the trigger - is `lib/menu.tsx`'s, shared
 * with every other menu in the product. The row also answers a right-click with
 * the same two actions, anchored at the pointer.
 */
function RowMenu({ menu, session, title, onRename, onDelete }: {
  menu: MenuController;
  session: SessionSummary;
  title: string;
  onRename: (session: SessionSummary) => void;
  onDelete: (session: SessionSummary) => void;
}): JSX.Element {
  /** The row's title is selectable, so the native menu could copy a selection;
   *  ours copies the row's own spelling, which is the fact worth keeping. */
  const copyTitle = async (): Promise<void> => {
    const copied = await copyText(title);
    store.pushToast(copied ? 'Session title copied.' : 'Could not copy the title.',
      copied ? 'success' : 'error');
  };
  return <div class="session-menu-wrap">
    <button
      type="button"
      class="icon-button session-menu-trigger"
      aria-label={`Actions for ${title}`}
      {...menu.triggerProps}
    >
      <KebabIcon size={15} />
    </button>
    <Menu
        menu={menu}
        label={`Session actions for ${title}`}
        float={Boolean(menu.pointer)}
        class={menu.pointer ? undefined : 'session-menu'}
      >
      <MenuItem menu={menu} action={{ label: 'Rename', onSelect: () => onRename(session) }} />
      <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle() }} />
      <MenuItem menu={menu} action={{ label: 'Delete…', danger: true, onSelect: () => onDelete(session) }} />
    </Menu>
  </div>;
}

/**
 * The project tile on a row: the workbench's `.rail-avatar`, at the rail's size,
 * carrying the same per-project tone the overview's tiles carry. One project must
 * read the same colour in every list on this screen, so the hue comes from the
 * shared variant in `control-shared.css`, not from anything this file owns.
 */
function ProjectTile({ project, id }: { project: string; id: string | null }): JSX.Element {
  return <span class="rail-avatar" data-tone={tone(id)} aria-hidden="true">{initials(project)}</span>;
}

interface RailRowProps {
  session: SessionSummary;
  title: string;
  active: boolean;
  /** The shared word-and-tone pair (`controlModel.sessionStatus`). */
  status: SessionStatus;
  /** A run parked on an approval question: the row's rail turns rose. */
  needsYou: boolean;
  /** The avatar column: a project's initials, or a control chat's `k.`. */
  tile: JSX.Element;
  /** The facts between the status word and the age: project, messages. */
  facts: JSX.Element[];
  /** The agent-count pill, when the chat has delegated to anyone. */
  badge: JSX.Element | null;
  onOpen: () => void;
  onRename: (session: SessionSummary) => void;
  onDelete: (session: SessionSummary) => void;
}

/**
 * One row, in the workbench's session-row grammar.
 *
 * The list row is `.session-row` + `.session-open` + `.session-head` /
 * `.session-title` / `.session-meta` - the same classes, in the same order, as
 * `components/SessionsRail.tsx`, so a row here and a row in the workspace draw
 * the same 12px/400 secondary title, the same mono meta line and the same 2px
 * status rail at the left edge. Nothing in this component re-inks or re-sizes
 * those roles; the only plane-specific part is the avatar column
 * (`.cp-row-body`), because this list mixes projects and the workbench's does not.
 *
 * Every row shows a *word* for its state, including settled ones ("Finished",
 * "Cancelled", "No runs"), unlike the workbench rail which omits the quiet ones.
 * That is deliberate: a list of every project is read by scanning for "which of
 * these needs me", and three identical grey dots answer nothing.
 */
function RailRow({
  session,
  title,
  active,
  status,
  needsYou,
  tile,
  facts,
  badge,
  onOpen,
  onRename,
  onDelete,
}: RailRowProps): JSX.Element {
  // The same class vocabulary that drives `.session-row::before`: a run in
  // flight, a failure, and a row waiting on the reader each paint it differently.
  const rowState = [
    session.running ? 'running' : '',
    session.last_run_status === 'failed' ? 'failed' : '',
    needsYou ? 'needs' : '',
  ].filter(Boolean).join(' ');
  const menu = useMenu();

  return <li
    class={`session-row session-row-quiet${active ? ' is-active' : ''}${rowState ? ` ${rowState}` : ''}`}
    {...contextMenuProps(menu, () => true, () => menu.triggerRef.current)}
  >
    <button
      type="button"
      class="session-open"
      aria-current={active ? 'true' : undefined}
      title={title}
      onClick={onOpen}
    >
      <span class="cp-row-body">
        {tile}
        <span class="cp-row-text">
          <span class="session-head">
            <span class="session-title">{title}</span>
            {badge}
            <span class={`status-dot status-${status.tone}`} aria-hidden="true" />
          </span>
          <span class="session-meta">
            <span
              class={`session-state session-state-${status.tone}`}
              title={`Latest state: ${status.label}`}
            >{status.label}</span>
            {facts.map((fact, index) => <Fragment key={index}>
              <span class="meta-dot" aria-hidden="true" />
              {fact}
            </Fragment>)}
          </span>
        </span>
      </span>
    </button>
    <RowMenu menu={menu} session={session} title={title} onRename={onRename} onDelete={onDelete} />
  </li>;
}

/**
 * The rail: every conversation on the server, one browser column.
 *
 * The rail is two groups. **Control chats** are conversations that act on *other*
 * projects; they carry their own model and subagent counts and are listed first.
 * **Sessions** are the ordinary project conversations, with search, filters and
 * running-first ordering. The two never overlap: a session the server marks
 * `control` is never drawn as a project row, whatever its id says.
 *
 * The chrome is the workbench's, not a private copy (§5A of
 * `docs/internal/DESIGN-SYSTEM.md`): `.side-head` + `.side-search` +
 * `.session-chips`/`.filter-chip` + `.session-nav` + `.side-footer`/`.online-row`,
 * with `components/icons.tsx` for every glyph. What is genuinely the plane's own
 * is the two-group list, its sub-headings, and the project avatar column.
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
    return <RailRow
      key={session.id}
      session={session}
      title={title}
      active={selected === session.id}
      status={status}
      needsYou={false}
      tile={<ProjectTile project={project} id={session.project_id} />}
      badge={null}
      // The project name is the fact this list has and the workbench's row does
      // not; recency stays where the workbench reads it. The message count is
      // last on purpose - when the mono line runs out of room, `overflow: hidden`
      // cuts the tail, and losing "2 msgs" is a smaller loss than losing the age.
      // Both spellings are `lib/format.ts`'s: abbreviated on the line, spelled
      // out in its tooltip.
      // Age first: it is the fact a reader scans for, and the line truncates its
      // tail, so the fact most worth keeping goes where the clip cannot reach.
      // The project name carries its full spelling in a tooltip because it is the
      // one fact here that is long enough to be cut (§5.9).
      facts={[
        <span key="age">{relativeTime(session.updated_at)}</span>,
        <span key="project" title={project}>{project}</span>,
        <span key="messages" title={pluralise(session.message_count, 'message')}>
          {pluralise(session.message_count, 'msg')}
        </span>,
      ]}
      onOpen={() => onNavigate(session)}
      {...rowActions}
    />;
  };

  const controlRow = (chat: ControlChatSummary, rowActions: RowActions): JSX.Element => {
    // A chat whose run is parked on an approval question is not "Running" in any
    // useful sense: it is waiting on a person. The word says so.
    const needsYou = (chat.pending_approvals ?? 0) > 0;
    const status: SessionStatus = needsYou ? { label: 'Needs you', tone: 'warn' } : sessionStatus(chat);
    const title = chatTitle(chat);
    return <RailRow
      key={chat.id}
      session={chat}
      title={title}
      active={selected === chat.id}
      status={status}
      needsYou={needsYou}
      // The one glyph that is the plane's own: `k.` marks a conversation that is
      // not inside a project, painted like the workbench's `k.` mark.
      tile={<span class="rail-avatar cp-tile-control" aria-hidden="true">k.</span>}
      // Only the name lives on the title line. A chat's model is not part of its
      // identity - the composer says which model will answer - so the title gets
      // the whole line and truncates on its own. The agent count rides beside the
      // title (a `.badge`, the workbench's count pill) rather than in the meta
      // line, which is a single 12px row of mono text.
      badge={chat.agents.total > 0 ? <span
        class="badge"
        title={agentBadgeTitle(chat.agents)}
      >{chat.agents.live}/{chat.agents.total} agents</span> : null}
      facts={[<span key="age">{relativeTime(chat.updated_at)}</span>]}
      onOpen={() => onNavigate(chat)}
      {...rowActions}
    />;
  };

  const rowActions: RowActions = { onRename, onDelete };

  // The footer's "live" is about *this list*, not about the server connection:
  // the rail draws from a polled read, so a failed read is the one thing that
  // makes it stale, and the word says so instead of an unexplained amber dot.
  const live = listError === null;

  return <aside ref={railRef} class="cp-browser" aria-label="Sessions across projects" tabIndex={-1}>
    <header class="side-head cp-head">
      {/* The workbench's rail head is `.project-title` holding a text span and a
          `.path-hint`; the plane's head is the same role on a screen that lists
          every project, so it wears the same structure instead of a fourth
          title recipe. Not a heading: the workbench's is a labelled button, and
          the page's one h1 is the overview's hero. */}
      <div class="project-title">
        <span class="project-title-text">Control plane</span>
        <span class="path-hint">Every project, one view</span>
      </div>
      <button
        type="button"
        class="icon-button"
        title="Refresh sessions"
        aria-label="Refresh sessions"
        onClick={onRefresh}
      ><RefreshIcon size={15} /></button>
    </header>
    <label class="side-search">
      <SearchIcon size={15} />
      <input
        type="search"
        value={query}
        placeholder="Search sessions or projects…"
        aria-label="Search sessions or projects"
        autocomplete="off"
        spellcheck={false}
        data-escape-local
        onInput={(event) => onQuery(event.currentTarget.value)}
      />
    </label>
    <div class="session-chips cp-filters" role="group" aria-label="Filter sessions">
      {FILTERS.map((item) => <button
        type="button"
        key={item.id}
        class={`filter-chip${filter === item.id ? ' is-on' : ''}`}
        aria-pressed={filter === item.id}
        onClick={() => onFilter(item.id)}
      >{item.label}<span class="filter-chip-count">{counts[item.id]}</span></button>)}
    </div>
    <nav class="session-nav" aria-label="Session list">
      <section aria-label="Control chats">
        <div class="cp-group-head">
          <h2 class="session-group-label">Control chats</h2>
          <button
            type="button"
            class="button button-small"
            aria-label="New control chat"
            title="New control chat"
            onClick={onNewChat}
          ><PlusIcon size={13} /> New</button>
        </div>
        {chats.length === 0
          ? <button type="button" class="cp-control-entry" onClick={onNewChat}>
              <span class="cp-spark" aria-hidden="true"><SparkIcon size={14} /></span>
              <span class="cp-control-copy"><strong>New control chat</strong><small>Act on any project from one place</small></span>
              <span class="cp-chev" aria-hidden="true"><ArrowRightIcon size={14} /></span>
            </button>
          : <ul class="session-list">{chats.map((chat) => controlRow(chat, rowActions))}</ul>}
      </section>
      {loading && sessions.length === 0 ? <p class="cp-note">Loading sessions…</p> : null}
      {/* The project-session group keeps a stable name so a screen reader (and
          the browser verification script) can tell which group a row is in. */}
      <section aria-label="Sessions">
        <h2 class="session-group-label">Sessions<span aria-hidden="true">{projectSessions.length}</span></h2>
        {!loading && projectSessions.length === 0 && !listError
          ? <p class="cp-note">No sessions yet. Start one from control chat.</p>
          : null}
        {groups.map((group) => <section key={group.key} aria-label={group.label}>
          {/* The sub-group count is only worth drawing when it says something
              the outer "Sessions" count does not: one group with every session
              in it repeated the same number twice in a row. */}
          <h2 class="session-group-label cp-group-sub">
            {group.label}
            {groups.length > 1 ? <span aria-hidden="true">{group.items.length}</span> : null}
          </h2>
          <ul class="session-list">{group.items.map((session) => row(session, rowActions))}</ul>
        </section>)}
        {!loading && filtered.length === 0 && query.trim() !== '' ? <p class="cp-note">No sessions match “{query}”.</p> : null}
      </section>
    </nav>
    {/* The workbench's foot, with the plane's own facts.
     *
     * It is not a copy of the workspace foot, because the plane is not
     * project-scoped: there is no context meter (the meter is the open
     * conversation's) and no provider chip (the launch box and the composer name
     * the model where it is chosen). What it does carry is what a reader of *this*
     * list needs: whether the list is live or stale, how many projects are in
     * scope, and the way into Settings, which the plane had no route to at all.
     *
     * The count is pluralised: the old foot said "1 projects". */}
    <div class="side-footer">
      <div class="online-row">
        <span class={`online-ind status-${live ? 'ok' : 'warn'}`} aria-hidden="true" />
        <span>{live ? 'Live' : 'Stale'}</span>
        <span class="right">{pluralise(projectCount, 'project')}</span>
      </div>
      <div class="settings-row">
        {/* `focus()` before opening, as the workbench's own foot does: the sheet
         * returns focus here when it closes, and the button is about to be the
         * only thing under the pointer. */}
        <button
          type="button"
          class="settings-btn"
          onClick={(event) => { event.currentTarget.focus(); actions.openSettings(); }}
        >
          <SettingsIcon size={15} /> Settings
        </button>
      </div>
    </div>
  </aside>;
}
