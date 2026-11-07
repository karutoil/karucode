import type { JSX } from 'preact';
import type { SessionSummary } from '../../api/types';
import { useMemo } from 'preact/hooks';
import type {
  Config,
  ControlOverview as ControlSnapshot,
  Project,
} from '../../api/types';
import * as store from '../../state/store';
import { absoluteTime, relativeTime, truncate } from '../../lib/format';
import { REASONING_OPTIONS } from '../ModelSwitcher';
import { AgentsIcon, ArrowRightIcon, SparkIcon } from '../icons';
import { agentStatusWord, type ModelChoice, initials, sessionStatus, tone } from './controlModel';
import type { SessionStatus } from './controlModel';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../../lib/menu';
import { copyText } from '../../lib/clipboard';
import '../../styles/control-overview.css';

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
 *
 * The reasoning list is `REASONING_OPTIONS` from `ModelSwitcher.tsx` — the same
 * array the composer's popover renders, imported rather than copied. A `<select>`
 * cannot use the popover's markup, but it can and must use the same *source*: a
 * second table here is how the launch box ends up offering an effort the composer
 * does not (or missing one the server accepts). Only `value`/`label` are read; the
 * canonical `description` sentence is what the popover shows under each label and
 * a native option row has nowhere to put it.
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
      <span class="eyebrow">Model</span>
      <select
        class="input cp-launch-select"
        aria-label="Model for the new chat"
        value={chosen?.key ?? ''}
        onChange={(event) => onModel(event.currentTarget.value)}
      >
        {choices.map((choice) => <option value={choice.key} key={choice.key}>{choice.label}</option>)}
      </select>
    </label>
    {chosen?.canReason === false ? null : <label class="cp-launch-pick">
      <span class="eyebrow">Reasoning</span>
      <select
        class="input cp-launch-select"
        aria-label="Reasoning for the new chat"
        value={effort}
        onChange={(event) => onEffort(event.currentTarget.value)}
      >
        {REASONING_OPTIONS.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}
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
  /** A row's "Open in project": the plane's own jump, for project sessions only. */
  onOpenInProject?: (session: { id: string; project_id: string | null }) => void;
  /** A project tile filters the rail down to that project's name. */
  onFilterProject: (name: string) => void;
}

/**
 * The "Running now" card. The card is one button, so its right-click menu is
 * portalled (the float layer) rather than nested inside it: a `<div role="menu">`
 * cannot be a button's child.
 */
function RunningCard({ session, projectName, onNavigate, onOpenInProject }: {
  session: SessionSummary;
  projectName: (id: string | null | undefined) => string;
  onNavigate: (session: { id: string; project_id: string | null }) => void;
  onOpenInProject?: (session: { id: string; project_id: string | null }) => void;
}): JSX.Element {
  const menu = useMenu();
  const title = session.title || 'Untitled session';
  return <>
    <button
      type="button"
      class="cp-card agent-card"
      onClick={() => onNavigate(session)}
      aria-haspopup="menu"
      ref={(el) => { menu.triggerRef.current = el; }}
      {...contextMenuProps(menu, () => true, () => menu.triggerRef.current)}
    >
      <span class="cp-card-top">
        <span class="rail-avatar" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
        <span class="session-state session-state-running"><span class="status-dot status-running" aria-hidden="true" />Running</span>
      </span>
      <strong class="cp-card-title">{title}</strong>
      <span class="cp-card-meta">{projectName(session.project_id)} · {relativeTime(session.updated_at)} · {session.message_count.toLocaleString()} msgs</span>
      <span class="cp-card-action">Open conversation<ArrowRightIcon size={12} /></span>
    </button>
    <Menu menu={menu} float label={`Session actions for ${title}`}>
      <MenuItem menu={menu} action={{ label: 'Open conversation', onSelect: () => onNavigate(session) }} />
      {onOpenInProject && session.project_id
        ? <MenuItem menu={menu} action={{ label: 'Open in project', onSelect: () => onOpenInProject(session) }} />
        : null}
      <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle(title) }} />
    </Menu>
  </>;
}

/** One row of the "Recent sessions" table, for the same reason. */
function RecentRow({ session, projectName, onNavigate, onOpenInProject }: {
  session: SessionSummary;
  projectName: (id: string | null | undefined) => string;
  onNavigate: (session: { id: string; project_id: string | null }) => void;
  onOpenInProject?: (session: { id: string; project_id: string | null }) => void;
}): JSX.Element {
  const menu = useMenu();
  const status = sessionStatus(session);
  const title = session.title || 'Untitled session';
  return <>
    <button
      type="button"
      role="row"
      class="cp-table-row"
      title={title}
      aria-haspopup="menu"
      ref={(el) => { menu.triggerRef.current = el; }}
      onClick={() => onNavigate(session)}
      {...contextMenuProps(menu, () => true, () => menu.triggerRef.current)}
    >
      <span class="cp-table-session" role="cell">
        <span class="rail-avatar" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
        <strong class="cp-table-title">{title}</strong>
      </span>
      <span class="cp-table-cell cp-col-meta" role="cell">{projectName(session.project_id)}</span>
      <span class="cp-table-cell cp-col-meta" role="cell">{relativeTime(session.updated_at)}</span>
      <span class="cp-table-cell cp-col-meta num" role="cell">{session.message_count.toLocaleString()}</span>
      <span class={`session-state session-state-${status.tone}`} role="cell">{status.label}</span>
    </button>
    <Menu menu={menu} float label={`Session actions for ${title}`}>
      <MenuItem menu={menu} action={{ label: 'Open conversation', onSelect: () => onNavigate(session) }} />
      {onOpenInProject && session.project_id
        ? <MenuItem menu={menu} action={{ label: 'Open in project', onSelect: () => onOpenInProject(session) }} />
        : null}
      <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle(title) }} />
    </Menu>
  </>;
}

/** One copy-title action, shared by every dashboard row: same label, same toast. */
async function copyTitle(title: string): Promise<void> {
  const copied = await copyText(title);
  store.pushToast(copied ? 'Session title copied.' : 'Could not copy the title.',
    copied ? 'success' : 'error');
}

/**
 * A session row on the dashboard: the workbench's row grammar, with the avatar in
 * its own column so the row keeps the product's 41px height instead of
 * stretching to fit a tile inside the title line.
 *
 * The row answers a right-click with the actions that otherwise live only on the
 * open session's header - open it, open it in its own project, copy its title.
 * `onOpenInProject` is optional because a control chat has no project to go to.
 */
function SessionRowCard({ session, rowClass, projectName, onNavigate, onOpenInProject, statusWord }: {
  session: SessionSummary;
  rowClass: string;
  projectName: (id: string | null | undefined) => string;
  onNavigate: (session: { id: string; project_id: string | null }) => void;
  onOpenInProject?: (session: { id: string; project_id: string | null }) => void;
  statusWord?: SessionStatus;
}): JSX.Element {
  const menu = useMenu();
  const title = session.title || 'Untitled session';
  const copyTitle = async (): Promise<void> => {
    const copied = await copyText(title);
    store.pushToast(copied ? 'Session title copied.' : 'Could not copy the title.',
      copied ? 'success' : 'error');
  };
  const label = `Session actions for ${title}`;
  return <li
    class={rowClass}
    data-session-id={session.id}
    {...contextMenuProps(menu, () => true, () => menu.triggerRef.current)}
  >
    <button
      type="button"
      ref={(el) => { menu.triggerRef.current = el; }}
      class="session-open cp-side-row"
      title={title}
      aria-haspopup="menu"
      onClick={() => onNavigate(session)}
    >
      <span class="cp-row-body">
        <span class="rail-avatar" data-tone={tone(session.project_id)} aria-hidden="true">{initials(projectName(session.project_id))}</span>
        <span class="cp-row-text">
          <span class="session-head">
            <span class="session-title">{title}</span>
          </span>
          <span class="session-meta">
            {statusWord
              ? <span class={`session-state session-state-${statusWord.tone}`}>
                  <span class={`status-dot status-${statusWord.tone}`} aria-hidden="true" />{statusWord.label}
                </span>
              : <span class="session-state session-state-warn">
                  <span class="status-dot status-warn" aria-hidden="true" />Needs you
                </span>}
            <span class="meta-dot" aria-hidden="true" />
            <span>{projectName(session.project_id)}</span>
            <span class="meta-dot" aria-hidden="true" />
            <span>{absoluteTime(session.updated_at)}</span>
          </span>
        </span>
      </span>
    </button>
    <Menu menu={menu} float label={label}>
      <MenuItem menu={menu} action={{ label: 'Open conversation', onSelect: () => onNavigate(session) }} />
      {onOpenInProject && session.project_id
        ? <MenuItem menu={menu} action={{ label: 'Open in project', onSelect: () => onOpenInProject(session) }} />
        : null}
      <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle() }} />
    </Menu>
  </li>;
}

/**
 * The overview: what is happening across every project right now.
 *
 * Everything on it is one snapshot, never five derived lists, so the counters and
 * the lists beneath them cannot disagree with each other. A failed read marks the
 * snapshot stale and keeps it on screen (the shell draws that banner), because
 * "nothing is running anywhere" is a different and wrong answer.
 *
 * **The vocabulary is the workbench's, not this file's** (§5A of
 * `docs/internal/DESIGN-SYSTEM.md`). Every element that plays a role the product
 * already has wears the product's class and gets no local redraw:
 *
 * | here | the shared class |
 * | --- | --- |
 * | hero + section kickers, launch field labels, table head | `.eyebrow` (base.css) |
 * | hero counters | `.metric` / `.metric-num` / `.metric-label` (inspector.css) |
 * | launch submit | `.button .button-primary` (base.css); the rest are `.input` |
 * | section counts | `.badge` (base.css) |
 * | running + project tiles | `.agent-card` for the chrome, `.rail-avatar` for the avatar |
 * | "nothing here" | `.empty-state` / `.empty-copy` (chat.css) |
 * | live agents | `.agent-card` / `.agent-head` / `.agent-name` / … (inspector.css, as `AgentsTab` draws them) |
 * | recent-session rows | `.session-row` / `.session-open` / `.session-title` / `.session-meta` (layout.css) |
 * | status in a row | `.status-dot status-<tone>` (base.css) |
 * | glyphs | `SparkIcon` / `ArrowRightIcon` / `AgentsIcon`, never a text character |
 *
 * | status word | `.session-state` + `.session-state-<tone>` (layout.css) |
 *
 * What stays `.cp-*` is what the workbench genuinely has no equivalent for: this
 * dashboard grid, the launch box and the recent-sessions *table* (see the comment
 * above the table itself).
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
  onOpenInProject,
  onFilterProject,
}: ControlOverviewProps): JSX.Element {
  const runningList = overview?.running ?? [];
  const recentList = overview?.recent ?? [];
  const attentionList = overview?.attention ?? [];
  const waitingList = overview?.waiting ?? [];
  const liveAgents = overview?.live_agents ?? [];
  const projectRows = overview?.projects ?? [];
  const sessionCounts = overview?.sessions;

  /**
   * The five counters, from the snapshot and nothing else.
   *
   * A list rather than five hand-written cells so the grid, the `.metric` recipe
   * and the order are stated once. The order is a contract:
   * `scripts/verify_control_plane.py` reads the workspace summary's text and
   * expects the chat count immediately before "chats".
   */
  const counters: { key: string; label: string; value: number }[] = [
    { key: 'running', label: 'running', value: sessionCounts?.running ?? 0 },
    { key: 'chats', label: 'chats', value: chatCount },
    { key: 'sessions', label: 'sessions', value: sessionCounts?.total ?? 0 },
    { key: 'agents', label: 'agents', value: overview?.agents.live ?? 0 },
    { key: 'projects', label: 'projects', value: projectRows.length },
  ];

  return <div class="cp-overview">
    <header class="cp-hero">
      <div class="cp-hero-copy">
        <p class="eyebrow">All projects</p>
        <h1>What are we building<span>.</span></h1>
        <p class="cp-hero-line">{(sessionCounts?.running ?? 0) > 0
          ? `${sessionCounts?.running} session${sessionCounts?.running === 1 ? '' : 's'} working right now across ${projectRows.length} projects.`
          : 'Nothing is running. Pick a session below, or ask control chat to start work.'}
          {chatCount > 0
            ? ` ${chatCount} control chat${chatCount === 1 ? '' : 's'} stand beside the projects.`
            : null}</p>
      </div>
      <div class="cp-stats" aria-label="Workspace summary">
        {counters.map((counter) => <div class="metric" key={counter.key}>
          <div class="metric-num">{counter.value}</div>
          <div class="metric-label">{counter.label}</div>
        </div>)}
      </div>
    </header>

    {/* A run parked on an approval question is the one thing on this page that
        cannot make progress without a person, so it is said in words above the
        dashboard rather than left to a count in a corner. It is inked with the
        status colour for "needs you", not the accent: the accent owns the primary
        action (the launch box), and §2 keeps status off the accent. */}
    {(overview?.approvals?.pending ?? 0) > 0
      ? <p class="cp-pending" role="status">
          <strong>{overview?.approvals?.pending}</strong> approval{(overview?.approvals?.pending ?? 0) === 1 ? '' : 's'} waiting on you — open the session to answer.
        </p>
      : null}

    {/* The launch box is the one hand-built surface left in the plane, so every
        control inside it is a product control: `.eyebrow` for the labels,
        `.input` for the fields, `.button .button-primary` for the submit, and
        `SparkIcon`/`ArrowRightIcon` for the two marks. */}
    <form class="cp-launch" onSubmit={onLaunch}>
      <label class="eyebrow cp-launch-label" for="cp-launch-input">Start something with control chat</label>
      <div class="cp-launch-row">
        <SparkIcon size={16} class="cp-launch-spark" />
        <select
          class="input cp-launch-project"
          title={projects.find((item) => item.id === launchProject)?.name ?? undefined}
          aria-label="Project for control chat"
          value={launchProject}
          onChange={(event) => onLaunchProject(event.currentTarget.value)}
        >
          {projects.length === 0 ? <option value="">No projects</option> : null}
          {projects.map((project) => <option value={project.id} key={project.id}>{project.name}</option>)}
        </select>
        <input
          id="cp-launch-input"
          class="input cp-launch-input"
          value={draft}
          placeholder="Describe work for any project — you’ll confirm the project before it starts…"
          onInput={(event) => onDraft(event.currentTarget.value)}
        />
        <button type="submit" class="button button-primary cp-launch-submit" disabled={opening}>
          {draft.trim() === '' ? 'Open chat' : 'Continue'}
          <ArrowRightIcon size={14} />
        </button>
      </div>
      <div class="cp-launch-model">
        <LaunchModelPicker
          value={launchModelKey}
          effort={launchEffort}
          onModel={onLaunchModel}
          onEffort={onLaunchEffort}
        />
      </div>
      <p class="field-hint cp-launch-hint">The chat is pinned to this model before your prompt lands. Control chat names the project explicitly and never starts work without your review, according to the approval policy.</p>
    </form>

    <div class="cp-dash">
      <div class="cp-dash-main">
        <section class="cp-section" aria-label="Running now">
          <div class="cp-section-head"><h2 class="eyebrow">Running now</h2><span class="badge">{runningList.length}</span></div>
          {runningList.length === 0
            ? <div class="empty-state cp-empty"><p class="empty-copy">No active runs. Recently active sessions are listed below.</p></div>
            : <div class="cp-cards">{runningList.slice(0, 8).map((session) => (
                <RunningCard
                  key={session.id}
                  session={session}
                  projectName={projectName}
                  onNavigate={onNavigate}
                  onOpenInProject={onOpenInProject}
                />
              ))}</div>}
        </section>

        <section class="cp-section" aria-label="Recent sessions">
          <div class="cp-section-head"><h2 class="eyebrow">Recent sessions</h2><span class="badge">{recentList.length}</span></div>
          {recentList.length === 0
            ? <div class="empty-state cp-empty"><p class="empty-copy">No other sessions yet.</p></div>
            /* Deliberately a table, not a list of `.session-row`s. The workbench's
               row grammar is one project's vertical list: a title, a meta line and
               a dot, read top to bottom. This block answers a different question -
               compare twelve sessions *across* projects - and the aligned columns
               are the answer: scanning the State column for the failures, or the
               Msgs column for the heavyweight, only works because the columns line
               up. Turning it into `.session-row`s would either lose that
               comparison (each value a fragment in a run-on meta line) or rebuild
               the same grid inside every row. So `.cp-table*` stays, and stays
               cheap: shared tokens for type/spacing/radius, and an `.eyebrow` head
               rather than a fourth micro-kicker definition. */
            : <div class="cp-table" role="table" aria-label="Recent sessions">
                <div class="cp-table-head eyebrow" role="row">
                  <span role="columnheader">Session</span>
                  <span role="columnheader" class="cp-col-meta">Project</span>
                  <span role="columnheader" class="cp-col-meta">Updated</span>
                  <span role="columnheader" class="cp-col-meta">Msgs</span>
                  <span role="columnheader">State</span>
                </div>
                {recentList.slice(0, 12).map((session) => (
                  <RecentRow
                    key={session.id}
                    session={session}
                    projectName={projectName}
                    onNavigate={onNavigate}
                    onOpenInProject={onOpenInProject}
                  />
                ))}
              </div>}
        </section>
      </div>

      <div class="cp-dash-side">
        {waitingList.length > 0 ? <section class="cp-section" aria-label="Waiting on you">
          <div class="cp-section-head"><h2 class="eyebrow">Waiting on you</h2><span class="badge">{waitingList.length}</span></div>
          <ul class="session-list">
            {waitingList.slice(0, 6).map((session) => (
              <SessionRowCard
                key={session.id}
                session={session}
                rowClass="session-row needs"
                projectName={projectName}
                onNavigate={onNavigate}
                onOpenInProject={onOpenInProject}
              />
            ))}
          </ul>
        </section> : null}

        {attentionList.length > 0 ? <section class="cp-section" aria-label="Needs attention">
          <div class="cp-section-head"><h2 class="eyebrow">Needs attention</h2><span class="badge">{attentionList.length}</span></div>
          <ul class="session-list">
            {attentionList.slice(0, 6).map((session) => {
              const status = sessionStatus(session);
              return <SessionRowCard
                key={session.id}
                session={session}
                rowClass={`session-row ${status.tone === 'danger' ? 'failed' : 'needs'}`}
                projectName={projectName}
                onNavigate={onNavigate}
                onOpenInProject={onOpenInProject}
                statusWord={status}
              />;
            })}
          </ul>
        </section> : null}

        {liveAgents.length > 0 ? <section class="cp-section" aria-label="Live agents">
          <div class="cp-section-head"><h2 class="eyebrow">Agents working</h2><span class="badge">{liveAgents.length}</span></div>
          {/* The workbench's own agent card, not a one-line row of this region's
              own: same classes, same `data-status` paint (running/paused/failed),
              same foot with the 24px action. The overview is where an agent is
              *discovered*, so it carries only what identifies one plus the way
              back to its session. */}
          <div class="agent-list">{liveAgents.slice(0, 6).map((agent) => <article class="agent-card" data-status={agent.status} key={agent.id}>
            <header class="agent-head">
              <div class="agent-title-row">
                <span class="agent-icon" aria-hidden="true"><AgentsIcon size={15} /></span>
                <div class="agent-titles">
                  <h3 class="agent-name" title={agent.name || 'subagent'}>{agent.name || 'subagent'}</h3>
                  <p class="agent-role" title={agent.role || 'agent'}>{agent.role || 'agent'} · {truncate(agent.session_title, 34)}</p>
                </div>
                <span class="agent-live">{agentStatusWord(agent.status)}</span>
              </div>
            </header>
            {agent.task ? <p class="agent-task">{truncate(agent.task, 180)}</p> : null}
            {agent.note ? <p class="agent-note" title={agent.note}>{agent.note}</p> : null}
            {typeof agent.progress === 'number' && Number.isFinite(agent.progress)
              ? <span
                  class="agent-progress"
                  role="progressbar"
                  aria-label={`Progress for ${agent.name || 'subagent'}`}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(Math.max(0, Math.min(1, agent.progress)) * 100)}
                  aria-valuetext={`${Math.round(Math.max(0, Math.min(1, agent.progress)) * 100)}% complete`}
                >
                  <span class="agent-progress-fill" style={{ width: `${Math.round(Math.max(0, Math.min(1, agent.progress)) * 100)}%` }} />
                </span>
              : null}
            <div class="agent-card-foot">
              <span>
                {agent.iterations} {agent.iterations === 1 ? 'iteration' : 'iterations'}
                {agent.background ? ' · background' : ''}
              </span>
              <button
                type="button"
                class="agent-card-open"
                title={`Open ${agent.session_title}`}
                aria-label={`Open ${agent.session_title}`}
                onClick={() => onNavigate({ id: agent.session_id, project_id: agent.project_id ?? null })}
              >
                <span class="agent-card-open-label">Open</span>
                <ArrowRightIcon size={12} />
              </button>
            </div>
          </article>)}</div>
        </section> : null}

        <section class="cp-section" aria-label="Projects">
          <div class="cp-section-head"><h2 class="eyebrow">Projects</h2><span class="badge">{projectRows.length}</span></div>
          {projectRows.length === 0
            ? <div class="empty-state cp-empty"><p class="empty-copy">No projects yet. Add one from the project rail.</p></div>
            : <div class="cp-projects">{projectRows.map((item) => <button
                type="button"
                key={item.id}
                class="cp-project agent-card"
                title={`Filter sessions to ${item.name}`}
                onClick={() => onFilterProject(item.name)}
              >
                <span class="rail-avatar" data-tone={tone(item.id)} aria-hidden="true">{initials(item.name)}</span>
                <span class="cp-project-copy">
                  <strong>{item.name}</strong>
                  <small>{item.sessions} {item.sessions === 1 ? 'session' : 'sessions'}{item.running > 0 ? ` · ${item.running} running` : ''}</small>
                </span>
              </button>)}</div>}
        </section>
      </div>
    </div>
  </div>;
}
