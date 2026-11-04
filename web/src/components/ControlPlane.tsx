import type { JSX } from 'preact';
import { Fragment } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { apiRequest } from '../api/client';
import { listAgents, listSessions } from '../api/endpoints';
import type { AgentSummary, SessionSummary } from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { copyText } from '../lib/clipboard';
import { controlPlaneOpen } from '../state/controlPlane';
import { ChatView } from './ChatView';
import '../styles/session-control.css';

type Filter = 'all' | 'running' | 'finished';

const FILTERS: { id: Filter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'finished', label: 'Finished' },
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

const isLiveAgent = (agent: AgentSummary): boolean =>
  agent.status === 'running' || agent.status === 'paused';

/** Server-wide session browser: open any conversation, steer it, or start one from control chat. */
export function ControlPlane(): JSX.Element {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [controlSessions, setControlSessions] = useState<SessionSummary[]>([]);
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState('');
  const [launchProject, setLaunchProject] = useState('');
  const generation = useRef(0);
  const navigation = useRef<Promise<void>>(Promise.resolve());
  const mounted = useRef(true);
  const refreshTick = useRef(0);

  const refresh = async (): Promise<void> => {
    const token = ++refreshTick.current;
    const [sessionResult, agentResult, controlResult] = await Promise.allSettled([
      listSessions(),
      listAgents(),
      apiRequest<{ sessions: SessionSummary[] }>('/api/control/sessions'),
    ]);
    if (!mounted.current || token !== refreshTick.current) return;
    if (sessionResult.status === 'fulfilled') { setSessions(sessionResult.value); setListError(null); }
    else setListError(sessionResult.reason instanceof Error ? sessionResult.reason.message : 'Could not load sessions.');
    if (agentResult.status === 'fulfilled') setAgents(agentResult.value);
    if (controlResult.status === 'fulfilled') setControlSessions(controlResult.value.sessions);
    setLoading(false);
  };

  useEffect(() => {
    if (launchProject === '' || !store.projects.value.some((project) => project.id === launchProject)) {
      setLaunchProject(store.currentProjectId.value ?? store.projects.value[0]?.id ?? '');
    }
  }, [store.projects.value, launchProject]);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => { mounted.current = false; generation.current++; window.clearInterval(timer); };
  }, []);

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
      let target = controlSessions[0] ?? null;
      if (createNew || !target) {
        const created = await apiRequest<{ session_id: string; project_id: string | null }>(
          '/api/control/sessions', { method: 'POST', body: {} },
        );
        if (!mounted.current || token !== generation.current) return null;
        await refresh();
        target = {
          id: created.session_id,
          title: 'Control chat',
          workspace: '',
          project_id: created.project_id,
          created_at: Date.now(),
          updated_at: Date.now(),
          message_count: 0,
          running: false,
        };
      }
      await navigate({ id: target.id, project_id: target.project_id ?? null });
      return target.id;
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
    setDraft('');
    const id = await openControlSession(false);
    // Only fill the composer once the control session is really the open one.
    if (id && store.currentSessionId.value === id) actions.fillComposer(instruction);
  };

  const chosen = sessions.find((item) => item.id === selected)
    ?? controlSessions.find((item) => item.id === selected)
    ?? null;
  const isControl = selected !== null && controlSessions.some((item) => item.id === selected);
  const activeAgents = agents.filter(isLiveAgent);
  const sorting = (a: SessionSummary, b: SessionSummary): number =>
    Number(b.running) - Number(a.running) || b.updated_at - a.updated_at;
  const ordered = useMemo(() => [...sessions].sort(sorting), [sessions]);
  // Control chats are their own set, listed above; they are never project rows.
  const controlIds = new Set(controlSessions.map((session) => session.id));
  const visible = ordered.filter((session) => !controlIds.has(session.id));
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return visible.filter((session) => {
      if (filter === 'running' && !session.running) return false;
      if (filter === 'finished' && (session.running || session.last_run_status !== 'completed')) return false;
      if (needle === '') return true;
      return `${session.title} ${projectName(session.project_id)}`.toLowerCase().includes(needle);
    });
  }, [visible, filter, query, store.projects.value]);
  const recent = visible.filter((session) => !session.running).slice(0, 12);
  const attention = visible.filter((session) => !session.running
    && (session.last_run_status === 'failed' || session.last_run_status === 'interrupted' || session.last_run_status === 'limit_reached'));
  const running = visible.filter((session) => session.running);
  const counts: Record<Filter, number> = {
    all: visible.length,
    running: running.length,
    finished: visible.filter((session) => !session.running && session.last_run_status === 'completed').length,
  };
  const groups: { key: string; label: string; items: SessionSummary[] }[] = [
    { key: 'running', label: 'Running now', items: filtered.filter((session) => session.running) },
    { key: 'recent', label: filter === 'all' ? 'Recently active' : 'Sessions', items: filtered.filter((session) => !session.running) },
  ].filter((group) => group.items.length > 0);

  const row = (session: SessionSummary, control = false): JSX.Element => {
    const status = sessionStatus(session);
    const project = control ? 'Control plane' : projectName(session.project_id);
    return <li key={session.id}>
      <button
        type="button"
        class={`cp-row${selected === session.id ? ' selected' : ''}${session.running ? ' is-running' : ''}`}
        aria-pressed={selected === session.id}
        title={session.title}
        onClick={() => navigate(session)}
      >
        <span class={`cp-glyph${control ? ' control' : ''}`} data-tone={tone(session.project_id)} aria-hidden="true">{control ? 'k.' : initials(project)}</span>
        <span class="cp-row-main">
          <span class="cp-row-title">{session.title}</span>
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
    </li>;
  };

  const openInProject = (): void => {
    if (!selected) return;
    const target = chosen ?? store.activeSession.value;
    if (!target) return;
    navigate({ id: target.id, project_id: target.project_id }, true);
  };

  const back = (): void => { setSelected(null); setError(null); };

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
          {controlSessions.length === 0
            ? <button type="button" class="cp-control-entry" onClick={() => void openControlSession(true)}>
                <span class="cp-spark" aria-hidden="true">✳</span>
                <span class="cp-control-copy"><strong>New control chat</strong><small>Act on any project from one place</small></span>
                <span class="cp-chev" aria-hidden="true">→</span>
              </button>
            : <ul class="cp-list">{controlSessions.map((session) => <Fragment key={session.id}>{row(session, true)}</Fragment>)}</ul>}
        </section>
        {loading && sessions.length === 0 ? <p class="cp-note">Loading sessions…</p> : null}
        {!loading && sessions.length === 0 && !listError ? <p class="cp-note">No sessions yet. Start one from control chat.</p> : null}
        {groups.map((group) => <section key={group.key} aria-label={group.label}>
          <div class="cp-group"><span>{group.label}</span><span class="cp-group-count">{group.items.length}</span></div>
          <ul class="cp-list">{group.items.map((session) => row(session))}</ul>
        </section>)}
        {!loading && filtered.length === 0 && query.trim() !== '' ? <p class="cp-note">No sessions match “{query}”.</p> : null}
      </div>
      <footer class="cp-browser-foot">
        <span class="cp-live"><i aria-hidden="true" />Live</span>
        <span class="cp-foot-projects">{store.projects.value.length} projects</span>
      </footer>
    </aside>

    <main class="cp-pane">
      {listError ? <div class="cp-error" role="alert">Could not refresh sessions: {listError}. The list may be stale.</div> : null}
      {error ? <div class="cp-error" role="alert">{error} <button type="button" onClick={() => setError(null)}>Dismiss</button></div> : null}
      {selected ? <section class="cp-session" aria-label="Open session">
        <header class="cp-session-head">
          <button type="button" class="cp-back" onClick={back} aria-label="Back to overview">←</button>
          <span class={`cp-glyph${isControl ? ' control' : ''}`} aria-hidden="true">{isControl ? 'k.' : initials(projectName(chosen?.project_id ?? null))}</span>
          <div class="cp-session-id">
            <div class="cp-eyebrow">{isControl ? 'Across all projects' : projectName(chosen?.project_id ?? null)}</div>
            <h2>{chosen?.title ?? (isControl ? 'Control chat' : 'Session')}</h2>
          </div>
          <div class="cp-session-actions">
            {chosen ? <span class={`cp-state ${sessionStatus(chosen).tone}`}><i aria-hidden="true" />{sessionStatus(chosen).label}</span> : null}
            {chosen ? <span class="cp-session-count">{chosen.message_count.toLocaleString()} messages</span> : null}
            {isControl ? null : <button type="button" class="cp-secondary" onClick={openInProject}>Open in project ↗</button>}
          </div>
        </header>
        {opening && !chosen ? <div class="cp-loading"><span class="cp-spinner" aria-hidden="true" />Opening session…</div> : null}
        {store.currentSessionId.value === selected
          ? <div class="cp-session-body">
              <div class="cp-chat"><ChatView
                emptyContent={isControl ? <ControlChatIntro onStart={(project) => actions.fillComposer(controlStarter(project))} /> : undefined}
                mentionProjects={isControl ? store.projects.value.map((project) => ({ id: project.id, name: project.name })) : undefined}
              /></div>
              <SessionFacts
                session={chosen}
                control={isControl}
                onOpenInProject={openInProject}
              />
            </div>
          : !opening ? <div class="cp-loading">This session is not open yet. <button type="button" class="cp-secondary" onClick={() => { if (chosen) navigate(chosen); }}>Try again</button></div> : null}
      </section> : <div class="cp-overview">
        <header class="cp-hero">
          <div>
            <div class="cp-eyebrow">All projects</div>
            <h1>What are we building<span>.</span></h1>
            <p>{running.length > 0
              ? `${running.length} session${running.length === 1 ? '' : 's'} working right now across ${store.projects.value.length} projects.`
              : 'Nothing is running. Pick a session below, or ask control chat to start work.'}</p>
          </div>
          <div class="cp-stats" aria-label="Workspace summary">
            <div><strong>{running.length}</strong><span>running</span></div>
            <div><strong>{visible.length + controlSessions.length}</strong><span>sessions</span></div>
            <div><strong>{activeAgents.length}</strong><span>agents</span></div>
            <div><strong>{store.projects.value.length}</strong><span>projects</span></div>
          </div>
        </header>

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
          <p class="cp-launch-hint">Control chat names the project explicitly and never starts work without your review, according to the approval policy.</p>
        </form>

        <div class="cp-dash">
          <div class="cp-dash-main">
            <section class="cp-section" aria-label="Running now">
              <div class="cp-section-head"><h3>Running now</h3><span>{running.length}</span></div>
              {running.length === 0
                ? <p class="cp-note">No active runs. Recently active sessions are beside this.</p>
                : <div class="cp-cards">{running.slice(0, 8).map((session) => <button
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
              <div class="cp-section-head"><h3>Recent sessions</h3><span>{recent.length}</span></div>
              {recent.length === 0
                ? <p class="cp-note">No other sessions yet.</p>
                : <div class="cp-table" role="table" aria-label="Recent sessions">
                    <div class="cp-table-head" role="row">
                      <span role="columnheader">Session</span>
                      <span role="columnheader">Project</span>
                      <span role="columnheader">Updated</span>
                      <span role="columnheader">Msgs</span>
                      <span role="columnheader">State</span>
                    </div>
                    {recent.map((session) => {
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
            {attention.length > 0 ? <section class="cp-section" aria-label="Needs attention">
              <div class="cp-section-head"><h3>Needs attention</h3><span>{attention.length}</span></div>
              <ul class="cp-recent">{attention.slice(0, 6).map((session) => {
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

            {activeAgents.length > 0 ? <section class="cp-section" aria-label="Live agents">
              <div class="cp-section-head"><h3>Agents working</h3><span>{activeAgents.length}</span></div>
              <ul class="cp-agents">{activeAgents.slice(0, 6).map((agent) => <li key={agent.id}>
                <span class={`cp-agent-dot ${agent.status === 'paused' ? 'paused' : ''}`} aria-hidden="true" />
                <span class="cp-agent-name">{agent.name}</span>
                <span class="cp-agent-task" title={agent.task}>{agent.task}</span>
                <button type="button" class="cp-link" onClick={() => navigate({ id: agent.session_id, project_id: agent.project_id ?? null })}>{agent.session_title} →</button>
              </li>)}</ul>
            </section> : null}

            <section class="cp-section" aria-label="Projects">
              <div class="cp-section-head"><h3>Projects</h3><span>{store.projects.value.length}</span></div>
              <div class="cp-projects">{store.projects.value.map((item) => {
                const own = visible.filter((session) => session.project_id === item.id);
                const live = own.filter((session) => session.running).length;
                return <button
                  type="button"
                  key={item.id}
                  class="cp-project"
                  title={`Filter sessions to ${item.name}`}
                  onClick={() => { setFilter('all'); setQuery(item.name); }}
                >
                  <span class="cp-glyph" data-tone={tone(item.id)} aria-hidden="true">{initials(item.name)}</span>
                  <span class="cp-project-copy">
                    <strong>{item.name}</strong>
                    <small>{own.length} {own.length === 1 ? 'session' : 'sessions'}{live > 0 ? ` · ${live} running` : ''}</small>
                  </span>
                </button>;
              })}</div>
            </section>
          </div>
        </div>
      </div>}
    </main>
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
  return <aside class="cp-facts" aria-label="Session details">
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
  </aside>;
}
