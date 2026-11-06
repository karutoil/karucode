import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { apiRequest } from '../api/client';
import { listAgents, listSessions } from '../api/endpoints';
import type { AgentSummary, Config, ControlChatSummary, SessionSummary } from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { copyText } from '../lib/clipboard';
import {
  CONTROL_REFRESH_MS,
  agentsForControlChat,
  controlChats,
  controlOverview,
  controlOverviewError,
  controlPlaneOpen,
  loadControlOverview,
} from '../state/controlPlane';
import { ChatView } from './ChatView';
import { Dialog } from './Dialog';
import { ModelSwitcher, ReasoningSelector } from './ModelSwitcher';
import { KebabIcon } from './icons';
import '../styles/session-control.css';

type Filter = 'all' | 'running' | 'finished';

const FILTERS: { id: Filter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'finished', label: 'Finished' },
];

/**
 * The reasoning efforts the launch box offers.
 *
 * Mirrors `REASONING_OPTIONS` in `ModelSwitcher.tsx` — the composer's selector is
 * the source of truth for what the server accepts, and this table exists only
 * because a `<select>` cannot reuse a popover's option list. `default` sends no
 * `reasoning_effort` at all, which is why it is stored as `null` when pinned.
 */
const LAUNCH_REASONING: { value: string; label: string }[] = [
  { value: 'default', label: 'Default' },
  { value: 'none', label: 'Off' },
  { value: 'minimal', label: 'Minimal' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'max', label: 'Max' },
];

/** "just now", "9m", "3h", "2d" — compact enough for a dense row. */
export function shortAgo(ms: number, now: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 45) return 'just now';
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))}m ago`;
  if (seconds < 86_400) return `${Math.round(seconds / 3600)}h ago`;
  return `${Math.round(seconds / 86_400)}d ago`;
}

/** Two initials for a project glyph; a project with no name falls back to one dot. */
export function initials(name: string): string {
  const words = name.split(/[^A-Za-z0-9]+/).filter(Boolean);
  if (words.length === 0) return '?';
  return words.slice(0, 2).map((word) => word[0]).join('').toUpperCase();
}

interface Status {
  label: string;
  tone: 'live' | 'good' | 'bad' | 'warn' | 'idle';
}

export function sessionStatus(session: Pick<SessionSummary, 'running' | 'last_run_status'>): Status {
  if (session.running) return { label: 'Running', tone: 'live' };
  switch (session.last_run_status) {
    case 'completed': return { label: 'Finished', tone: 'good' };
    case 'failed': return { label: 'Failed', tone: 'bad' };
    case 'interrupted': return { label: 'Interrupted', tone: 'warn' };
    case 'limit_reached': return { label: 'Limit reached', tone: 'warn' };
    case 'cancelled': return { label: 'Cancelled', tone: 'idle' };
    default: return { label: 'No runs', tone: 'idle' };
  }
}

/** Stable 0–3 tone per project, so the list is scannable by project without colour meaning anything else. */
export function tone(id: string | null | undefined): number {
  let hash = 0;
  for (const char of id ?? '') hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return hash % 4;
}

/** What a chat's agent badge means, spelled out for the tooltip. */
export function agentBadgeTitle(agents: { live: number; total: number }): string {
  const running = agents.live === 1 ? 'subagent' : 'subagents';
  return `${agents.live} ${running} running · ${agents.total} total`;
}

/** "Untitled chat" rather than an empty row: the server creates chats unnamed on purpose. */
function chatTitle(chat: Pick<SessionSummary, 'title'>): string {
  return chat.title.trim() === '' ? 'Untitled chat' : chat.title;
}

interface RowActions {
  onRename: (session: SessionSummary) => void;
  onDelete: (session: SessionSummary) => void;
}

/**
 * The control plane: every project in one browser, and one place to act on it.
 *
 * The rail is two groups. **Control chats** are conversations that act on *other*
 * projects; they carry their own model and subagent counts and are listed first.
 * **Sessions** are the ordinary project conversations, with search, filters and
 * running-first ordering. The two never overlap: a session the server marks
 * `control` is never drawn as a project row, whatever its id says.
 *
 * The overview is one snapshot (`GET /api/control/overview` via
 * `state/controlPlane.ts`), never five derived lists, so the counters and the
 * lists beneath them cannot disagree with each other. A failed read marks the
 * snapshot stale and keeps it on screen, because "nothing is running anywhere"
 * is a different and wrong answer.
 */
export function ControlPlane(): JSX.Element {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [listError, setListError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState('');
  const [launchProject, setLaunchProject] = useState('');
  const [launchModelKey, setLaunchModelKey] = useState('');
  const [launchEffort, setLaunchEffort] = useState(() => store.modelSelection.value.effort ?? 'default');
  const [renaming, setRenaming] = useState<SessionSummary | null>(null);
  const [renameDraft, setRenameDraft] = useState('');
  const [deleting, setDeleting] = useState<SessionSummary | null>(null);
  /**
   * Every subagent of the open session, whatever its status.
   *
   * The snapshot's `live_agents` only carries the running ones - that is the
   * right list for "who is working right now", and the wrong one for "what did
   * this chat delegate". A finished child is still the answer to that question, so
   * the open conversation reads its own session's agents from
   * `GET /api/agents?session=`, which lists every row the store has.
   */
  const [detailAgents, setDetailAgents] = useState<AgentSummary[] | null>(null);
  const generation = useRef(0);
  const navigation = useRef<Promise<void>>(Promise.resolve());
  const mounted = useRef(true);
  const refreshTick = useRef(0);

  const overview = controlOverview.value;
  const staleReason = controlOverviewError.value;
  const chats = controlChats();

  /**
   * Read the rail and the snapshot together.
   *
   * One timer rather than `startControlPlaneRefresh()` plus a second one: the
   * rail also needs `/api/sessions` (the snapshot only carries the running,
   * attention and recent slices), so both reads share the snapshot's period
   * instead of polling the same server twice on two schedules.
   */
  const refresh = async (): Promise<void> => {
    const token = ++refreshTick.current;
    const [sessionResult] = await Promise.allSettled([listSessions(), loadControlOverview()]);
    if (!mounted.current || token !== refreshTick.current) return;
    if (sessionResult.status === 'fulfilled') {
      setSessions(sessionResult.value);
      setListError(null);
    } else {
      setListError(
        sessionResult.reason instanceof Error ? sessionResult.reason.message : 'Could not load sessions.',
      );
    }
    setLoading(false);
  };

  useEffect(() => {
    if (launchProject === '' || !store.projects.value.some((project) => project.id === launchProject)) {
      setLaunchProject(store.currentProjectId.value ?? store.projects.value[0]?.id ?? '');
    }
  }, [store.projects.value, launchProject]);

  // The launch box starts on the model a run would use right now, and follows
  // the config when it arrives (`/api/config` is read after this mounts).
  useEffect(() => {
    if (launchModelKey !== '') return;
    const selection = store.modelSelection.value;
    const choice = modelChoices(store.config.value).find(
      (item) => item.providerId === selection.providerId && item.modelId === selection.modelId,
    );
    if (choice) setLaunchModelKey(choice.key);
  }, [store.config.value, launchModelKey]);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), CONTROL_REFRESH_MS);
    return () => {
      mounted.current = false;
      generation.current++;
      window.clearInterval(timer);
    };
  }, []);

  /**
   * The open session's own subagents, on their own read.
   *
   * Keyed on the selection: opening another chat must not show the previous
   * chat's children, and a failed read must leave the rows already on screen
   * alone rather than blanking them (the snapshot's live agents stay the
   * fallback). A control chat has no project, so the read is by session id - the
   * only scope that finds them.
   */
  useEffect(() => {
    if (!selected) {
      setDetailAgents(null);
      return;
    }
    let cancelled = false;
    const read = async (): Promise<void> => {
      try {
        const agents = await listAgents({ session: selected });
        if (!cancelled) setDetailAgents(agents);
      } catch {
        // Keep what is on screen; the snapshot still answers "who is live".
      }
    };
    void read();
    const timer = window.setInterval(() => void read(), CONTROL_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selected]);

  const projectName = (id: string | null | undefined): string =>
    id ? store.projects.value.find((project) => project.id === id)?.name ?? id : 'No project';

  /**
   * Open one session in the ordinary workspace: switch its project first, because
   * the chat, composer and tools all read the active project.
   */
  const navigate = (session: { id: string; project_id: string | null }, close?: boolean): Promise<void> => {
    const token = ++generation.current;
    setSelected(session.id); setOpening(true); setError(null);
    navigation.current = navigation.current.catch(() => {}).then(async () => {
      if (!mounted.current || token !== generation.current) return;
      if (session.project_id) {
        if (!store.projects.value.some((project) => project.id === session.project_id)) {
          throw new Error('This session’s project is unavailable.');
        }
        if (store.currentProjectId.value !== session.project_id) await actions.switchProject(session.project_id);
        if (!mounted.current || token !== generation.current) return;
        if (store.currentProjectId.value !== session.project_id) throw new Error('Could not switch project.');
      }
      if (store.currentSessionId.value !== session.id && !await actions.openSession(session.id)) {
        throw new Error('Could not open session.');
      }
      if (!mounted.current || token !== generation.current) return;
      setOpening(false);
      if (close) controlPlaneOpen.value = false;
    }).catch((e: unknown) => {
      if (mounted.current && token === generation.current) {
        setOpening(false); setError(e instanceof Error ? e.message : 'Could not open session.');
      }
    });
    return navigation.current;
  };

  /** Open a control chat: the newest one, or a brand-new one. */
  const openControlSession = async (createNew: boolean): Promise<string | null> => {
    const token = ++generation.current;
    setOpening(true); setError(null);
    try {
      const newest: ControlChatSummary | null = chats[0] ?? null;
      if (createNew || !newest) {
        const created = await apiRequest<{ session_id: string; project_id: string | null }>(
          '/api/control/sessions', { method: 'POST', body: {} },
        );
        if (!mounted.current || token !== generation.current) return null;
        // The snapshot is the only list the rail draws, so re-read it before
        // opening the chat: otherwise the rail and the header disagree for a
        // whole poll interval about whether the chat exists.
        await refresh();
        await navigate({ id: created.session_id, project_id: created.project_id ?? null });
        return created.session_id;
      }
      await navigate({ id: newest.id, project_id: newest.project_id ?? null });
      return newest.id;
    } catch (e) {
      if (mounted.current && token === generation.current) {
        setOpening(false); setError(e instanceof Error ? e.message : 'Could not open control chat.');
      }
      return null;
    }
  };

  const launch = async (event: JSX.TargetedSubmitEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    const text = draft.trim();
    const target = store.projects.value.find((project) => project.id === launchProject);
    const instruction = target
      ? `Start work in project "${target.name}" (project_id: ${target.id}).\n\n${text}`
      : text;
    if (text === '') { await openControlSession(false); return; }
    const submitted = draft;
    const id = await openControlSession(false);
    // Only fill the composer once the control session is really the open one.
    // On failure, leave the launch prompt exactly where it can be retried. On
    // success, clear only that submission: edits made while it was pending are
    // new work and must not be replaced by an older continuation.
    if (id && store.currentSessionId.value === id) {
      // Pin the chosen model before the prompt lands, so the first run of the
      // chat uses the model the launch box showed. A refused pin must not
      // swallow the prompt: the draft is handed over either way.
      const choice = modelChoices(store.config.value).find((item) => item.key === launchModelKey);
      if (choice) {
        await actions.setSessionModelSelection(
          choice.providerId,
          choice.modelId,
          launchEffort === 'default' ? null : launchEffort,
        );
      }
      actions.fillComposer(instruction);
      setDraft((current) => (current === submitted ? '' : current));
    }
  };

  /** Rename through the ordinary route, then re-read the snapshot the rail draws. */
  const rename = async (session: SessionSummary, title: string): Promise<void> => {
    const trimmed = title.trim();
    if (trimmed === '') return;
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(session.id)}`, {
        method: 'PATCH',
        body: { title: trimmed },
      });
      setRenaming(null);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not rename this session.');
    }
  };

  /** Delete one session; the open one returns the pane to the overview. */
  const remove = async (session: SessionSummary): Promise<void> => {
    setDeleting(null);
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(session.id)}`, { method: 'DELETE' });
      if (selected === session.id) { setSelected(null); setError(null); }
      // The workspace store may still be pointing at the row that just vanished;
      // leaving it there would make the ordinary workspace open a dead session
      // after **Project workspace**.
      if (store.currentSessionId.value === session.id) store.currentSessionId.value = null;
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not delete this session.');
    }
  };

  const chosen = sessions.find((item) => item.id === selected)
    ?? chats.find((item) => item.id === selected)
    ?? null;
  const isControl = chats.some((item) => item.id === selected);
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
        onClick={() => navigate(session)}
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
        onClick={() => navigate(chat)}
      >
        <span class="cp-glyph control" aria-hidden="true">k.</span>
        <span class="cp-row-main">
          {/* The model rides on the title line: a chat's model is part of what it
              is, and the meta line below is where the live numbers go. */}
          <span class="cp-row-title">
            <span class="cp-row-title-text">{title}</span>
            {chat.model_label
              ? <span class="cp-badge cp-badge-model" title={`Model: ${chat.model_label}`}>{chat.model_label}</span>
              : null}
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

  const rowActions: RowActions = {
    onRename: (session) => { setRenameDraft(chatTitle(session)); setRenaming(session); },
    onDelete: (session) => setDeleting(session),
  };

  const openInProject = (): void => {
    if (!selected) return;
    const target = chosen ?? store.activeSession.value;
    if (!target) return;
    navigate({ id: target.id, project_id: target.project_id }, true);
  };

  const back = (): void => { setSelected(null); setError(null); };

  const runningList = overview?.running ?? [];
  const recentList = overview?.recent ?? [];
  const attentionList = overview?.attention ?? [];
  const waitingList = overview?.waiting ?? [];
  const liveAgents = overview?.live_agents ?? [];
  const projectRows = overview?.projects ?? [];
  const sessionCounts = overview?.sessions;

  return <div class={`control-plane cp${selected ? ' detail-open' : ''}`}>
    <aside class="cp-browser" aria-label="Sessions across projects">
      <header class="cp-browser-head">
        <div class="cp-brand">
          <span class="cp-brand-mark" aria-hidden="true">k<i>.</i></span>
          <div><h1>Control plane</h1><p>Every project, one view</p></div>
        </div>
        <button type="button" class="cp-icon" title="Refresh sessions" aria-label="Refresh sessions" onClick={() => void refresh()}>⟳</button>
      </header>
      <div class="cp-search">
        <span aria-hidden="true">⌕</span>
        <input
          type="search"
          value={query}
          placeholder="Search sessions or projects…"
          aria-label="Search sessions or projects"
          onInput={(event) => setQuery(event.currentTarget.value)}
        />
      </div>
      <div class="cp-filters" role="group" aria-label="Filter sessions">
        {FILTERS.map((item) => <button
          type="button"
          key={item.id}
          aria-pressed={filter === item.id}
          onClick={() => setFilter(item.id)}
        >{item.label}<span>{counts[item.id]}</span></button>)}
      </div>
      <div class="cp-scroll">
        <section aria-label="Control chats">
          <div class="cp-group cp-group-action">
            <span>Control chats</span>
            <button type="button" class="cp-new-chat" onClick={() => void openControlSession(true)} aria-label="New control chat">＋ New</button>
          </div>
          {chats.length === 0
            ? <button type="button" class="cp-control-entry" onClick={() => void openControlSession(true)}>
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
            <div class="cp-group cp-group-sub"><span>{group.label}</span><span class="cp-group-count">{group.items.length}</span></div>
            <ul class="cp-list">{group.items.map((session) => row(session, rowActions))}</ul>
          </section>)}
          {!loading && filtered.length === 0 && query.trim() !== '' ? <p class="cp-note">No sessions match “{query}”.</p> : null}
        </section>
      </div>
      <footer class="cp-browser-foot">
        <span class="cp-live"><i aria-hidden="true" />Live</span>
        <span class="cp-foot-projects">{store.projects.value.length} projects</span>
      </footer>
    </aside>

    <main class="cp-pane">
      {listError ? <div class="cp-error" role="alert">Could not refresh sessions: {listError}. The list may be stale.</div> : null}
      {error ? <div class="cp-error" role="alert">{error} <button type="button" onClick={() => setError(null)}>Dismiss</button></div> : null}
      {/* A failed read never clears the snapshot: it is marked stale instead. */}
      {staleReason ? <p class="cp-stale" role="status">stale · {staleReason}</p> : null}
      {selected ? <section class="cp-session" aria-label="Open session">
        <header class="cp-session-head">
          <button type="button" class="cp-back" onClick={back} aria-label="Back to overview" title="Back to overview">←</button>
          <span class={`cp-glyph${isControl ? ' control' : ''}`} aria-hidden="true">{isControl ? 'k.' : initials(projectName(chosen?.project_id ?? null))}</span>
          <div class="cp-session-id">
            <div class="cp-eyebrow">{isControl ? 'Control plane · across projects' : projectName(chosen?.project_id ?? null)}</div>
            <h2>{chosen ? chatTitle(chosen) : (isControl ? 'Untitled chat' : 'Session')}</h2>
          </div>
          <div class="cp-session-actions">
            {chosen ? <span class={`cp-state ${sessionStatus(chosen).tone}`} title={`Latest state: ${sessionStatus(chosen).label}`}><i aria-hidden="true" />{sessionStatus(chosen).label}</span> : null}
            {chosen ? <span class="cp-session-count">{chosen.message_count.toLocaleString()} messages</span> : null}
            {/* Both selectors write the *open session's* pin, and `store.activeSession`
                resolves a control chat too, so they work here unchanged. */}
            <ModelSwitcher />
            <ReasoningSelector />
            {isControl ? null : <button type="button" class="cp-secondary" title="Open in project" aria-label="Open in project" onClick={openInProject}>Open in project ↗</button>}
          </div>
        </header>
        {opening && !chosen ? <div class="cp-loading"><span class="cp-spinner" aria-hidden="true" />Opening session…</div> : null}
        {store.currentSessionId.value === selected
          ? <div class="cp-session-body">
              <div class="cp-chat"><ChatView
                mode="control-plane"
                emptyContent={isControl ? <ControlChatIntro onStart={(project) => actions.fillComposer(controlStarter(project))} /> : undefined}
                mentionProjects={isControl ? store.projects.value.map((project) => ({ id: project.id, name: project.name })) : undefined}
              /></div>
              <aside class="cp-inspector" aria-label="Chat details">
                <SessionFacts
                  session={chosen}
                  control={isControl}
                  onOpenInProject={openInProject}
                />
                <Subagents
                  sessionId={selected}
                  control={isControl}
                  agents={detailAgents}
                  claimedTotal={chats.find((chat) => chat.id === selected)?.agents.total ?? null}
                />
              </aside>
            </div>
          : !opening ? <div class="cp-loading">This session is not open yet. <button type="button" class="cp-secondary" onClick={() => { if (chosen) navigate(chosen); }}>Try again</button></div> : null}
      </section> : <div class="cp-overview">
        <header class="cp-hero">
          <div>
            <div class="cp-eyebrow">All projects</div>
            <h1>What are we building<span>.</span></h1>
            <p>{(sessionCounts?.running ?? 0) > 0
              ? `${sessionCounts?.running} session${sessionCounts?.running === 1 ? '' : 's'} working right now across ${projectRows.length} projects.`
              : 'Nothing is running. Pick a session below, or ask control chat to start work.'}
              {chats.length > 0
                ? ` ${chats.length} control chat${chats.length === 1 ? '' : 's'} stand beside the projects.`
                : null}</p>
          </div>
          <div class="cp-stats" aria-label="Workspace summary">
            <div><strong>{sessionCounts?.running ?? 0}</strong><span>running</span></div>
            <div><strong>{chats.length}</strong><span>chats</span></div>
            <div><strong>{sessionCounts?.total ?? 0}</strong><span>sessions</span></div>
            <div><strong>{overview?.agents.live ?? 0}</strong><span>agents</span></div>
            <div><strong>{projectRows.length}</strong><span>projects</span></div>
          </div>
        </header>

        {(overview?.approvals?.pending ?? 0) > 0
          ? <p class="cp-pending" role="status">
              <strong>{overview?.approvals?.pending}</strong> approval{(overview?.approvals?.pending ?? 0) === 1 ? '' : 's'} waiting on you — open the session to answer.
            </p>
          : null}

        <form class="cp-launch" onSubmit={(event) => void launch(event)}>
          <label class="cp-launch-label" for="cp-launch-input">Start something with control chat</label>
          <div class="cp-launch-row">
            <span class="cp-spark" aria-hidden="true">✳</span>
            <select
              class="cp-launch-project"
              aria-label="Project for control chat"
              value={launchProject}
              onChange={(event) => setLaunchProject(event.currentTarget.value)}
            >
              {store.projects.value.length === 0 ? <option value="">No projects</option> : null}
              {store.projects.value.map((project) => <option value={project.id} key={project.id}>{project.name}</option>)}
            </select>
            <input
              id="cp-launch-input"
              value={draft}
              placeholder="Describe work for any project — you’ll confirm the project before it starts…"
              onInput={(event) => setDraft(event.currentTarget.value)}
            />
            <button type="submit" class="cp-primary" disabled={opening}>{draft.trim() === '' ? 'Open chat' : 'Continue'} →</button>
          </div>
          <div class="cp-launch-model">
            <LaunchModelPicker
              value={launchModelKey}
              effort={launchEffort}
              onModel={setLaunchModelKey}
              onEffort={setLaunchEffort}
            />
          </div>
          <p class="cp-launch-hint">The chat is pinned to this model before your prompt lands. Control chat names the project explicitly and never starts work without your review, according to the approval policy.</p>
        </form>

        <div class="cp-dash">
          <div class="cp-dash-main">
            <section class="cp-section" aria-label="Running now">
              <div class="cp-section-head"><h3>Running now</h3><span>{runningList.length}</span></div>
              {runningList.length === 0
                ? <p class="cp-note">No active runs. Recently active sessions are beside this.</p>
                : <div class="cp-cards">{runningList.slice(0, 8).map((session) => <button
                    type="button"
                    key={session.id}
                    class="cp-card"
                    onClick={() => navigate(session)}
                  >
                    <span class="cp-card-top">
                      <span class="cp-glyph" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                      <span class="cp-state live"><i aria-hidden="true" />Running</span>
                    </span>
                    <strong>{session.title}</strong>
                    <span class="cp-card-meta">{projectName(session.project_id)} · {shortAgo(session.updated_at)} · {session.message_count.toLocaleString()} msgs</span>
                    <span class="cp-card-action">Open conversation →</span>
                  </button>)}</div>}
            </section>

            <section class="cp-section" aria-label="Recent sessions">
              <div class="cp-section-head"><h3>Recent sessions</h3><span>{recentList.length}</span></div>
              {recentList.length === 0
                ? <p class="cp-note">No other sessions yet.</p>
                : <div class="cp-table" role="table" aria-label="Recent sessions">
                    <div class="cp-table-head" role="row">
                      <span role="columnheader">Session</span>
                      <span role="columnheader">Project</span>
                      <span role="columnheader">Updated</span>
                      <span role="columnheader">Msgs</span>
                      <span role="columnheader">State</span>
                    </div>
                    {recentList.slice(0, 12).map((session) => {
                      const status = sessionStatus(session);
                      return <button
                        type="button"
                        role="row"
                        key={session.id}
                        class="cp-table-row"
                        title={session.title}
                        onClick={() => navigate(session)}
                      >
                        <span class="cp-table-session" role="cell">
                          <span class="cp-glyph small" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                          <strong>{session.title}</strong>
                        </span>
                        <span class="cp-table-cell" role="cell">{projectName(session.project_id)}</span>
                        <span class="cp-table-cell" role="cell">{shortAgo(session.updated_at)}</span>
                        <span class="cp-table-cell num" role="cell">{session.message_count.toLocaleString()}</span>
                        <span class={`cp-state ${status.tone} cp-table-state-cell`} role="cell"><i aria-hidden="true" />{status.label}</span>
                      </button>;
                    })}
                  </div>}
            </section>
          </div>

          <div class="cp-dash-side">
            {waitingList.length > 0 ? <section class="cp-section" aria-label="Waiting on you">
              <div class="cp-section-head"><h3>Waiting on you</h3><span>{waitingList.length}</span></div>
              <ul class="cp-recent">{waitingList.slice(0, 6).map((session) => <li key={session.id}>
                <button type="button" title={session.title} onClick={() => navigate(session)}>
                  <span class="cp-glyph small" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                  <span class="cp-recent-main"><strong>{session.title}</strong><small>{projectName(session.project_id)} · {shortMoment(session.updated_at)}</small></span>
                  <span class="cp-state warn"><i aria-hidden="true" />Needs you</span>
                </button>
              </li>)}</ul>
            </section> : null}

            {attentionList.length > 0 ? <section class="cp-section" aria-label="Needs attention">
              <div class="cp-section-head"><h3>Needs attention</h3><span>{attentionList.length}</span></div>
              <ul class="cp-recent">{attentionList.slice(0, 6).map((session) => {
                const status = sessionStatus(session);
                return <li key={session.id}>
                  <button type="button" title={session.title} onClick={() => navigate(session)}>
                    <span class="cp-glyph small" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                    <span class="cp-recent-main"><strong>{session.title}</strong><small>{projectName(session.project_id)} · {shortMoment(session.updated_at)}</small></span>
                    <span class={`cp-state ${status.tone}`}><i aria-hidden="true" />{status.label}</span>
                  </button>
                </li>;
              })}</ul>
            </section> : null}

            {liveAgents.length > 0 ? <section class="cp-section" aria-label="Live agents">
              <div class="cp-section-head"><h3>Agents working</h3><span>{liveAgents.length}</span></div>
              <ul class="cp-agents">{liveAgents.slice(0, 6).map((agent) => <li key={agent.id}>
                <span class={`cp-agent-dot ${agent.status === 'paused' ? 'paused' : ''}`} aria-hidden="true" />
                <span class="cp-agent-name">{agent.name}</span>
                <span class="cp-agent-task" title={agent.task}>{agent.note || agent.task}</span>
                <button type="button" class="cp-link" onClick={() => navigate({ id: agent.session_id, project_id: agent.project_id ?? null })}>{agent.session_title} →</button>
              </li>)}</ul>
            </section> : null}

            <section class="cp-section" aria-label="Projects">
              <div class="cp-section-head"><h3>Projects</h3><span>{projectRows.length}</span></div>
              <div class="cp-projects">{projectRows.map((item) => <button
                type="button"
                key={item.id}
                class="cp-project"
                title={`Filter sessions to ${item.name}`}
                onClick={() => { setFilter('all'); setQuery(item.name); }}
              >
                <span class="cp-glyph" data-tone={tone(item.id)} aria-hidden="true">{initials(item.name)}</span>
                <span class="cp-project-copy">
                  <strong>{item.name}</strong>
                  <small>{item.sessions} {item.sessions === 1 ? 'session' : 'sessions'}{item.running > 0 ? ` · ${item.running} running` : ''}</small>
                </span>
              </button>)}</div>
            </section>
          </div>
        </div>
      </div>}
    </main>

    <Dialog open={renaming !== null} title="Rename session" onClose={() => setRenaming(null)} class="dialog-confirm">
      {renaming ? <form
        class="confirm-body"
        onSubmit={(event) => { event.preventDefault(); void rename(renaming, renameDraft); }}
      >
        <label class="cp-dialog-label" for="cp-rename-input">Title</label>
        <input
          id="cp-rename-input"
          class="cp-dialog-input"
          value={renameDraft}
          aria-label="Session title"
          onInput={(event) => setRenameDraft(event.currentTarget.value)}
        />
        <div class="dialog-actions">
          <button type="button" class="button" onClick={() => setRenaming(null)}>Cancel</button>
          <button type="submit" class="button button-primary">Rename</button>
        </div>
      </form> : null}
    </Dialog>

    <Dialog open={deleting !== null} title="Delete session?" onClose={() => setDeleting(null)} class="dialog-confirm">
      {deleting ? <div class="confirm-body">
        <p>
          Delete <strong>{chatTitle(deleting)}</strong>? Its messages, runs and approvals are removed.
          This cannot be undone.
        </p>
        <div class="dialog-actions">
          <button type="button" class="button" data-autofocus onClick={() => setDeleting(null)}>Cancel</button>
          <button type="button" class="button button-danger" onClick={() => void remove(deleting)}>
            Delete session
          </button>
        </div>
      </div> : null}
    </Dialog>
  </div>;
}

/** One flat list of `provider:model` choices, so the select value is never parsed. */
interface ModelChoice {
  key: string;
  providerId: string;
  modelId: string;
  label: string;
  canReason: boolean;
}

function modelChoices(config: Config | null): ModelChoice[] {
  if (!config) return [];
  return config.providers.flatMap((profile) => profile.models.map((model) => ({
    key: `${profile.id}::${model.id}`,
    providerId: profile.id,
    modelId: model.id,
    label: `${model.name?.trim() || model.id} · ${profile.name}`,
    // models.dev's `reasoning: false` is a positive "cannot reason"; unknown
    // stays selectable and the endpoint decides at run time (same rule as
    // `ReasoningSelector`).
    canReason: model.reasoning !== false,
  })));
}

/**
 * The launch box's model and reasoning choice.
 *
 * The chat does not exist yet, so this cannot be the composer's session pin: it
 * is a local choice that `launch()` writes onto the session immediately after it
 * is opened, before the draft is handed over.
 */
function LaunchModelPicker({ value, effort, onModel, onEffort }: {
  value: string;
  effort: string;
  onModel: (key: string) => void;
  onEffort: (value: string) => void;
}): JSX.Element | null {
  const config = store.config.value;
  const choices = useMemo(() => modelChoices(config), [config]);
  if (choices.length === 0) return null;
  const chosen = choices.find((choice) => choice.key === value) ?? choices[0];
  return <>
    <label class="cp-launch-pick">
      <span class="cp-launch-mini">Model</span>
      <select
        aria-label="Model for the new chat"
        value={chosen?.key ?? ''}
        onChange={(event) => onModel(event.currentTarget.value)}
      >
        {choices.map((choice) => <option value={choice.key} key={choice.key}>{choice.label}</option>)}
      </select>
    </label>
    {chosen?.canReason === false ? null : <label class="cp-launch-pick">
      <span class="cp-launch-mini">Reasoning</span>
      <select
        aria-label="Reasoning for the new chat"
        value={effort}
        onChange={(event) => onEffort(event.currentTarget.value)}
      >
        {LAUNCH_REASONING.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}
      </select>
    </label>}
  </>;
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

function shortMoment(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** The control chat's first-run state: its job is starting sessions, not exploring code. */
function ControlChatIntro({ onStart }: { onStart: (project: { id: string; name: string }) => void }): JSX.Element {
  const projects = store.projects.value;
  return <div class="cp-intro">
    <span class="cp-intro-mark" aria-hidden="true">✳</span>
    <h2>What should we build?</h2>
    <p>
      Mention a project with <strong>@</strong>, or just name it. The control chat resolves it, opens a
      normal session in that project’s workspace with its own tools and approval policy, and the new
      session shows up here.
    </p>
    <div class="cp-intro-projects">
      <span class="cp-eyebrow">Start in</span>
      {projects.length === 0
        ? <span class="cp-intro-empty">Add a project first.</span>
        : projects.map((project) => <button type="button" key={project.id} onClick={() => onStart(project)}>{project.name}</button>)}
    </div>
    <p class="cp-intro-hint">You can also ask for a status summary, or discuss the plan before anything starts.</p>
  </div>;
}

/** A ready-to-send instruction that names the target project by id, so it is never guessed. */
export function controlStarter(project: { id: string; name: string }): string {
  return `Start work in project "${project.name}" (project_id: ${project.id}).\n\n`;
}

/**
 * Facts beside an open conversation. On a wide monitor the transcript is a
 * centred column, so this panel carries the session's own numbers and the two
 * actions that would otherwise need a trip back to the project.
 */
function SessionFacts({ session, control, onOpenInProject }: {
  session: SessionSummary | null;
  control: boolean;
  onOpenInProject: () => void;
}): JSX.Element {
  const status = session ? sessionStatus(session) : null;
  const project = session?.project_id
    ? store.projects.value.find((item) => item.id === session.project_id) ?? null
    : null;
  const copyId = (): void => {
    if (!session) return;
    void copyText(session.id).then((ok) =>
      store.pushToast(ok ? 'Session id copied.' : 'Could not copy the session id.', ok ? 'info' : 'error'));
  };
  return <section class="cp-facts" aria-label="Session details">
    <div class="cp-eyebrow">Session</div>
    <dl>
      <div>
        <dt>Where</dt>
        <dd>{control ? 'Control plane' : project?.name ?? 'No project'}</dd>
      </div>
      {project ? <div><dt>Path</dt><dd class="cp-facts-path">{project.path}</dd></div> : null}
      <div>
        <dt>State</dt>
        <dd>{status ? <span class={`cp-state ${status.tone}`}><i aria-hidden="true" />{status.label}</span> : 'Unknown'}</dd>
      </div>
      <div><dt>Messages</dt><dd>{session ? session.message_count.toLocaleString() : '—'}</dd></div>
      <div><dt>Last activity</dt><dd>{session ? shortMoment(session.updated_at) : '—'}</dd></div>
      <div>
        <dt>Session id</dt>
        <dd class="cp-facts-id"><span>{session?.id ?? '—'}</span>
          <button type="button" class="cp-link" onClick={copyId} disabled={!session}>Copy</button>
        </dd>
      </div>
    </dl>
    <div class="cp-facts-actions">
      {control ? null : <button type="button" class="cp-secondary" onClick={onOpenInProject}>Open in project ↗</button>}
      {session?.running ? <button type="button" class="cp-secondary" onClick={() => void actions.stopRun()}>Stop this run</button> : null}
    </div>
  </section>;
}

/**
 * The subagents behind the open conversation.
 *
 * An agent belongs to whichever session ran it, so a control chat that delegated
 * work lists its children here — the transcript itself only shows the spawn call.
 * `agents` is the session's own read (`GET /api/agents?session=`), so a finished
 * child stays on the list; while that read has not landed the snapshot's live
 * agents (filtered to this session) answer instead, which is why the badge in the
 * rail and this list can never tell two different stories.
 */
function Subagents({ sessionId, control, agents: read, claimedTotal }: {
  sessionId: string;
  control: boolean;
  agents: AgentSummary[] | null;
  claimedTotal: number | null;
}): JSX.Element {
  const agents = read ?? agentsForControlChat(sessionId);
  const total = Math.max(claimedTotal ?? 0, agents.length);
  const live = agents.filter((agent) => agent.status === 'running' || agent.status === 'paused').length;
  const noun = control ? 'chat' : 'session';
  return <section class="cp-subagents" aria-label="Subagents of this chat">
    <div class="cp-eyebrow">Subagents{total > 0 ? ` · ${total}` : ''}{live > 0 ? ` · ${live} live` : ''}</div>
    {agents.length === 0
      ? <p class="cp-note">{total === 0
          ? `No subagents yet — this ${noun} hasn't delegated anything.`
          : `This ${noun} has ${total} subagent${total === 1 ? '' : 's'}, none of them loaded yet.`}</p>
      : <ul class="cp-agent-list">{agents.map((agent) => <AgentRow agent={agent} key={agent.id} />)}</ul>}
  </section>;
}

/** One subagent: who it is, what it says it is doing, and its own detail. */
function AgentRow({ agent }: { agent: AgentSummary }): JSX.Element {
  const open = store.selectedAgentId.value === agent.id;
  return <li class="cp-agent">
    <div class="cp-agent-head">
      <span class={`cp-agent-dot ${agent.status === 'paused' ? 'paused' : ''}`} aria-hidden="true" />
      <span class="cp-agent-title">
        <strong>{agent.name || 'subagent'}</strong>
        <small>{agent.role || 'agent'} · {agentStatusWord(agent.status)} · {agent.iterations} iteration{agent.iterations === 1 ? '' : 's'}</small>
      </span>
      <button
        type="button"
        class="cp-link"
        aria-expanded={open}
        title={open ? 'Close this subagent' : 'Open this subagent’s transcript'}
        onClick={() => { if (open) actions.closeAgentDetail(); else void actions.openAgentDetail(agent.id); }}
      >{open ? 'Close' : 'Details'}</button>
    </div>
    {agent.note ? <p class="cp-agent-note">{agent.note}</p> : <p class="cp-agent-note muted" title={agent.task}>{agent.task}</p>}
    {open ? <AgentPeek agent={agent} /> : null}
  </li>;
}

/** The status as a word, never a colour: `completed` reads better as "done". */
export function agentStatusWord(status: AgentSummary['status']): string {
  switch (status) {
    case 'completed': return 'done';
    case 'running': return 'running';
    case 'paused': return 'paused';
    case 'failed': return 'failed';
    case 'cancelled': return 'cancelled';
    case 'interrupted': return 'interrupted';
    default: return status;
  }
}

/** The newest messages of one subagent, read by `actions.openAgentDetail`. */
function AgentPeek({ agent }: { agent: AgentSummary }): JSX.Element {
  const history = store.agentHistory.value;
  if (!history || history.id !== agent.id) {
    return <p class="cp-note">Reading {agent.name || 'this subagent'}’s transcript…</p>;
  }
  if (history.error) return <p class="cp-note" role="status">{history.error}</p>;
  const recent = history.messages.slice(-5);
  return <div class="cp-agent-detail">
    {recent.length === 0
      ? <p class="cp-note">It has not written a message yet.</p>
      : recent.map((message) => <div class="cp-agent-message" key={message.id}>
          <span class="cp-agent-role">{message.role}</span>
          <p>{message.content.length > 700 ? `${message.content.slice(0, 700)}…` : message.content}</p>
        </div>)}
    <p class="cp-note">{history.truncated ? 'Showing its newest messages.' : 'Its whole transcript.'}</p>
  </div>;
}
