import type { JSX } from 'preact';
import { useMemo } from 'preact/hooks';
import type {
  Config,
  ControlOverview as ControlSnapshot,
  Project,
} from '../../api/types';
import * as store from '../../state/store';
import { absoluteTime, relativeTime } from '../../lib/format';
import { type ModelChoice, initials, sessionStatus, tone } from './controlModel';
import '../../styles/control-overview.css';

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

export function modelChoices(config: Config | null): ModelChoice[] {
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

export interface ControlOverviewProps {
  /**
   * The one snapshot (`GET /api/control/overview`), or `null` before the first
   * successful read.
   */
  overview: ControlSnapshot | null;
  /** How many control chats exist; the hero counts them beside the sessions. */
  chatCount: number;
  /** Registered projects, as the launch box offers them. */
  projects: Project[];
  /** The launch draft and the three choices the form carries. */
  draft: string;
  onDraft: (value: string) => void;
  launchProject: string;
  onLaunchProject: (id: string) => void;
  launchModelKey: string;
  onLaunchModel: (key: string) => void;
  launchEffort: string;
  onLaunchEffort: (effort: string) => void;
  /** A launch is in flight; the submit button is disabled but stays visible. */
  opening: boolean;
  onLaunch: (event: JSX.TargetedSubmitEvent<HTMLFormElement>) => void;
  /** A session's project name, or "No project". */
  projectName: (id: string | null | undefined) => string;
  onNavigate: (session: { id: string; project_id: string | null }) => void;
  /** A project tile filters the rail down to that project's name. */
  onFilterProject: (name: string) => void;
}

/**
 * The overview: what is happening across every project right now.
 *
 * Everything on it is one snapshot, never five derived lists, so the counters and
 * the lists beneath them cannot disagree with each other. A failed read marks the
 * snapshot stale and keeps it on screen (the shell draws that banner), because
 * "nothing is running anywhere" is a different and wrong answer.
 */
export function ControlOverview({
  overview,
  chatCount,
  projects,
  draft,
  onDraft,
  launchProject,
  onLaunchProject,
  launchModelKey,
  onLaunchModel,
  launchEffort,
  onLaunchEffort,
  opening,
  onLaunch,
  projectName,
  onNavigate,
  onFilterProject,
}: ControlOverviewProps): JSX.Element {
  const runningList = overview?.running ?? [];
  const recentList = overview?.recent ?? [];
  const attentionList = overview?.attention ?? [];
  const waitingList = overview?.waiting ?? [];
  const liveAgents = overview?.live_agents ?? [];
  const projectRows = overview?.projects ?? [];
  const sessionCounts = overview?.sessions;

  return <div class="cp-overview">
    <header class="cp-hero">
      <div>
        <div class="cp-eyebrow">All projects</div>
        <h1>What are we building<span>.</span></h1>
        <p>{(sessionCounts?.running ?? 0) > 0
          ? `${sessionCounts?.running} session${sessionCounts?.running === 1 ? '' : 's'} working right now across ${projectRows.length} projects.`
          : 'Nothing is running. Pick a session below, or ask control chat to start work.'}
          {chatCount > 0
            ? ` ${chatCount} control chat${chatCount === 1 ? '' : 's'} stand beside the projects.`
            : null}</p>
      </div>
      <div class="cp-stats" aria-label="Workspace summary">
        <div><strong>{sessionCounts?.running ?? 0}</strong><span>running</span></div>
        <div><strong>{chatCount}</strong><span>chats</span></div>
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

    <form class="cp-launch" onSubmit={onLaunch}>
      <label class="cp-launch-label" for="cp-launch-input">Start something with control chat</label>
      <div class="cp-launch-row">
        <span class="cp-spark" aria-hidden="true">✳</span>
        <select
          class="cp-launch-project"
          aria-label="Project for control chat"
          value={launchProject}
          onChange={(event) => onLaunchProject(event.currentTarget.value)}
        >
          {projects.length === 0 ? <option value="">No projects</option> : null}
          {projects.map((project) => <option value={project.id} key={project.id}>{project.name}</option>)}
        </select>
        <input
          id="cp-launch-input"
          value={draft}
          placeholder="Describe work for any project — you’ll confirm the project before it starts…"
          onInput={(event) => onDraft(event.currentTarget.value)}
        />
        <button type="submit" class="cp-primary" disabled={opening}>{draft.trim() === '' ? 'Open chat' : 'Continue'} →</button>
      </div>
      <div class="cp-launch-model">
        <LaunchModelPicker
          value={launchModelKey}
          effort={launchEffort}
          onModel={onLaunchModel}
          onEffort={onLaunchEffort}
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
                onClick={() => onNavigate(session)}
              >
                <span class="cp-card-top">
                  <span class="cp-glyph" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                  <span class="cp-state running"><i aria-hidden="true" />Running</span>
                </span>
                <strong>{session.title}</strong>
                <span class="cp-card-meta">{projectName(session.project_id)} · {relativeTime(session.updated_at)} · {session.message_count.toLocaleString()} msgs</span>
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
                    onClick={() => onNavigate(session)}
                  >
                    <span class="cp-table-session" role="cell">
                      <span class="cp-glyph small" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                      <strong>{session.title}</strong>
                    </span>
                    <span class="cp-table-cell" role="cell">{projectName(session.project_id)}</span>
                    <span class="cp-table-cell" role="cell">{relativeTime(session.updated_at)}</span>
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
            <button type="button" title={session.title} onClick={() => onNavigate(session)}>
              <span class="cp-glyph small" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
              <span class="cp-recent-main"><strong>{session.title}</strong><small>{projectName(session.project_id)} · {absoluteTime(session.updated_at)}</small></span>
              <span class="cp-state warn"><i aria-hidden="true" />Needs you</span>
            </button>
          </li>)}</ul>
        </section> : null}

        {attentionList.length > 0 ? <section class="cp-section" aria-label="Needs attention">
          <div class="cp-section-head"><h3>Needs attention</h3><span>{attentionList.length}</span></div>
          <ul class="cp-recent">{attentionList.slice(0, 6).map((session) => {
            const status = sessionStatus(session);
            return <li key={session.id}>
              <button type="button" title={session.title} onClick={() => onNavigate(session)}>
                <span class="cp-glyph small" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
                <span class="cp-recent-main"><strong>{session.title}</strong><small>{projectName(session.project_id)} · {absoluteTime(session.updated_at)}</small></span>
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
            <button type="button" class="cp-link" onClick={() => onNavigate({ id: agent.session_id, project_id: agent.project_id ?? null })}>{agent.session_title} →</button>
          </li>)}</ul>
        </section> : null}

        <section class="cp-section" aria-label="Projects">
          <div class="cp-section-head"><h3>Projects</h3><span>{projectRows.length}</span></div>
          <div class="cp-projects">{projectRows.map((item) => <button
            type="button"
            key={item.id}
            class="cp-project"
            title={`Filter sessions to ${item.name}`}
            onClick={() => onFilterProject(item.name)}
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
  </div>;
}
