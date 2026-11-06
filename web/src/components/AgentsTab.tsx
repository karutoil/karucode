import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { AgentAction, AgentRecord, AgentStatus, AgentSummary } from '../api/types';
import { isAgentLive } from '../api/types';
import { formatDuration, jsonSummary, relativeTime, truncate } from '../lib/format';
import * as actions from '../state/actions';
import {
  AGENT_ACTIVITY_LIMIT,
  emptyActivity,
  activityFromMessages,
  mergeActivities,
  type AgentActivity,
  type AgentActivityItem,
} from '../state/agentConsole';
import { agentStatusLabel } from '../state/selectors';
import * as store from '../state/store';
import {
  AgentsIcon,
  ArrowRightIcon,
  ChevronLeftIcon,
  CpuIcon,
  EyeIcon,
  RefreshIcon,
  TerminalIcon,
} from './icons';

type AgentRow = AgentRecord & {
  session_title?: string;
  workspace?: string;
  project_id?: string | null;
};

interface FilterChip {
  id: store.AgentFilter;
  label: string;
}

const FILTERS: FilterChip[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'paused', label: 'Paused' },
  { id: 'completed', label: 'Done' },
  { id: 'failed', label: 'Failed' },
  { id: 'cancelled', label: 'Cancelled' },
  { id: 'interrupted', label: 'Interrupted' },
];

function agentGlyph(role: string | null | undefined, name: string | null | undefined): JSX.Element {
  // `role`/`name` come straight off an agent row and the reducer's placeholders
  // leave them empty; a missing one must not throw the whole tab away.
  const needle = `${role ?? ''} ${name ?? ''}`.toLowerCase();
  if (needle.includes('review') || needle.includes('audit')) return <EyeIcon size={15} />;
  if (needle.includes('test') || needle.includes('verif') || needle.includes('shell')) {
    return <TerminalIcon size={15} />;
  }
  if (needle.includes('agent') || needle.includes('build') || needle.includes('impl')) {
    return <CpuIcon size={15} />;
  }
  return <AgentsIcon size={15} />;
}

/**
 * How many agent rows are rendered before the list asks to be expanded.
 *
 * `subagents.max_concurrent = 0` means "no ceiling", so a fan-out can be
 * hundreds of children and the roster is a plain `map`. Sixty rows is enough to
 * fill the inspector several times over; past it the user asks for the rest
 * instead of the tab building every card, every live label and every progress
 * bar on a timer. The cap is safe because `sortAgents` puts live agents first -
 * the rows that can still change are always inside the first page.
 */
const AGENT_RENDER_CAP = 60;

/**
 * Live agents first (a paused one is still live), then newest first: the ones
 * that can still change are what the user opened this tab for.
 */
function sortAgents(agents: AgentRow[]): AgentRow[] {
  return [...agents].sort((a, b) => {
    const aLive = isAgentLive(a.status) ? 1 : 0;
    const bLive = isAgentLive(b.status) ? 1 : 0;
    if (aLive !== bLive) return bLive - aLive;
    return (b.created_at ?? 0) - (a.created_at ?? 0);
  });
}

function statusCounts(agents: AgentRow[]): Record<store.AgentFilter, number> {
  const counts: Record<store.AgentFilter, number> = {
    all: agents.length,
    running: 0,
    paused: 0,
    completed: 0,
    failed: 0,
    cancelled: 0,
    interrupted: 0,
  };
  for (const agent of agents) {
    // A status this build does not know about (a newer server) counts toward the
    // total only: `counts[status] += 1` on a missing key would write NaN.
    const status: AgentStatus = agent.status;
    if (status in counts) counts[status] += 1;
  }
  return counts;
}

/**
 * What this agent is doing right now, derived from its own live feed.
 *
 * A paused agent answers "paused" even though its feed still has a tool marked
 * running: that tool finished or was never resumed, and saying `running a
 * command` about a parked agent is the one thing this label must not do.
 *
 * Only the *live* half lives here — it reads the agent's own activity feed, which
 * only this tab has. The settled word for every status comes from
 * `agentStatusLabel` in `state/selectors.ts`, so this file cannot drift from the
 * vocabulary the rest of the app (the control plane included) uses.
 */
function liveLabel(agent: AgentRow, activity: AgentActivity): string {
  if (agent.status === 'paused') return 'paused';
  // `agentStatusLabel` (not a local map) so an unknown status from a newer
  // server echoes its wire word instead of rendering `undefined`.
  if (agent.status !== 'running') return agentStatusLabel(agent.status);
  const tail = [...activity.items].reverse();
  const active = tail.find(
    (item): item is Extract<AgentActivityItem, { kind: 'tool' | 'command' }> =>
      (item.kind === 'tool' || item.kind === 'command') && item.status === 'running',
  );
  if (active) {
    return active.kind === 'command' ? 'running a command' : `running ${active.name}`;
  }
  const approval = tail.find(
    (item) => item.kind === 'approval' && item.approval.status === 'pending',
  );
  if (approval) return 'waiting for approval';
  const streaming = tail.find((item) => item.kind === 'message' && item.streaming);
  if (streaming) return 'writing';
  // The server omits `note` entirely when the agent has never reported one, so
  // this is `undefined` in practice — never assume it is a string.
  const note = agent.note?.trim() ?? '';
  if (note !== '') return note;
  return (agent.iterations ?? 0) > 0 ? 'thinking' : 'starting';
}

/**
 * A progress bar for the agent's own claim, when it made one.
 *
 * Returning `null` when the claim is missing or not a finite number is what
 * keeps `<ProgressBar value={undefined}>` from rendering `width: 'NaN%'`: the
 * server omits `progress` rather than sending `null`, which slips past a bare
 * `!== null` check.
 */
function ProgressBar({
  value,
  label = 'Agent progress',
}: {
  value: number | null | undefined;
  label?: string;
}): JSX.Element | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null;
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  // A bar is a picture of a number, so the number is what assistive tech needs:
  // the width alone says nothing, and the `title` was the only place the value
  // was written down. `aria-valuetext` carries the same sentence for a reader
  // that does not do percentages.
  return (
    <span
      class="agent-progress"
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct}
      aria-valuetext={`${pct}% complete`}
      title={`${pct}% complete, according to the agent`}
    >
      <span class="agent-progress-fill" style={{ width: `${pct}%` }} />
    </span>
  );
}

function elapsed(agent: AgentRow, now: number): string {
  const end = agent.finished_at ?? now;
  // A missing timestamp would make this `NaN` and render "NaNm" in the footer.
  const started = Number.isFinite(agent.created_at) ? agent.created_at : now;
  return formatDuration(Math.max(0, end - started));
}

function argsPreview(item: Extract<AgentActivityItem, { kind: 'tool' }>): string {
  const summary = jsonSummary(item.args, 120);
  return summary || item.name;
}

function outputBlock(output: string): JSX.Element | null {
  if (output.trim() === '') return null;
  if (output.length <= 600) return <pre class="agent-event-output">{output}</pre>;
  return (
    <details class="agent-event-more">
      <summary>Show {output.length.toLocaleString()} characters of output</summary>
      <pre class="agent-event-output">{truncate(output, 12_000)}</pre>
    </details>
  );
}

function ActivityItemView({ item }: { item: AgentActivityItem }): JSX.Element | null {
  if (item.kind === 'message') {
    if (item.role === 'user') {
      return (
        <div class="agent-bubble agent-bubble-task">
          <span class="agent-bubble-role">Task</span>
          <div class="agent-bubble-body">{item.content}</div>
        </div>
      );
    }
    if (item.reasoning.trim() !== '') {
      return (
        <div class="agent-bubble agent-bubble-thought">
          <details>
            <summary>Thinking</summary>
            <pre class="agent-bubble-reasoning">{item.reasoning}</pre>
          </details>
          {item.content.trim() !== '' ? <div class="agent-bubble-body">{item.content}</div> : null}
        </div>
      );
    }
    if (item.content.trim() === '') return null;
    return (
      <div class="agent-bubble agent-bubble-answer">
        <div class="agent-bubble-body">{item.content}</div>
      </div>
    );
  }

  if (item.kind === 'approval') {
    return (
      <div class="agent-event agent-event-approval" data-status={item.approval.status}>
        <header class="agent-event-head">
          <span class="agent-event-name">{item.approval.title}</span>
          <span class="agent-event-status">{item.approval.status}</span>
        </header>
        <p class="agent-event-detail">{item.approval.detail}</p>
        {item.approval.decision ? (
          <p class="agent-event-note">decision: {item.approval.decision}</p>
        ) : null}
      </div>
    );
  }

  if (item.kind === 'command') {
    return (
      <div class="agent-event agent-event-command" data-status={item.status}>
        <header class="agent-event-head">
          <span class="agent-event-name mono">$ {item.command || 'command'}</span>
          <span class="agent-event-status">{item.status}</span>
        </header>
        <p class="agent-event-note mono" title={item.cwd}>
          {item.cwd}
          {item.exitCode !== null ? ` · exit ${item.exitCode}` : ''}
          {item.durationMs !== null ? ` · ${formatDuration(item.durationMs)}` : ''}
        </p>
        {outputBlock([item.stdout, item.stderr].filter(Boolean).join('\n') || item.output)}
      </div>
    );
  }

  return (
    <div class="agent-event agent-event-tool" data-status={item.status}>
      <header class="agent-event-head">
        <span class="agent-event-name mono">{item.name}</span>
        <span class="agent-event-status">{item.status}</span>
      </header>
      <p class="agent-event-detail mono" title={argsPreview(item)}>
        {argsPreview(item)}
      </p>
      {outputBlock(item.output)}
    </div>
  );
}

function AgentsEmpty({ filtered }: { filtered: boolean }): JSX.Element {
  if (filtered) {
    return (
      <div class="empty-inspector">
        <p class="empty-inspector-title">No agents match this filter</p>
        <p>Clear the status filter to see the rest of this project's agents.</p>
        <button type="button" class="button button-small" onClick={() => actions.setAgentFilter('all')}>
          Show all statuses
        </button>
      </div>
    );
  }
  return (
    <div class="empty-inspector">
      <p class="empty-inspector-title">No subagents yet</p>
      <p>
        Nothing has been delegated in this project. When a session calls <code>spawn_agent</code>, the
        agent appears here — including agents running in a session you are not looking at.
      </p>
      <button type="button" class="button button-small" onClick={actions.openSettings}>
        Subagent settings
      </button>
    </div>
  );
}

/**
 * Pause / resume / stop for one live agent.
 *
 * Rendered only while the loop exists: a settled agent's buttons would be dead
 * controls, and the 404 that followed would look like a failure.
 *
 * These are siblings of the card's own **Details** button, not children of it:
 * a native button inside another clickable surface is the nested-interactive
 * pattern that made Enter on Pause also open the drill-in.
 */
function AgentControls({ agent }: { agent: AgentRow }): JSX.Element | null {
  const pending = store.agentControlPending.value[agent.id];
  if (!isAgentLive(agent.status)) return null;
  const paused = agent.status === 'paused';
  const run = (action: AgentAction) => () => {
    void actions.controlAgent(agent.id, action);
  };
  return (
    <div class="agent-controls">
      <button
        type="button"
        class="button button-small"
        aria-label={paused ? `Resume ${agent.name}` : `Pause ${agent.name}`}
        disabled={pending !== undefined}
        onClick={run(paused ? 'resume' : 'pause')}
      >
        {pending === 'pause' || pending === 'resume' ? '…' : paused ? 'Resume' : 'Pause'}
      </button>
      <button
        type="button"
        class="button button-small button-danger"
        aria-label={`Stop ${agent.name}`}
        disabled={pending !== undefined}
        onClick={run('cancel')}
      >
        {pending === 'cancel' ? 'Stopping…' : 'Stop'}
      </button>
    </div>
  );
}

function AgentCard({
  agent,
  activity,
  now,
  showSession,
  onOpen,
}: {
  agent: AgentRow;
  activity: AgentActivity;
  now: number;
  showSession: boolean;
  onOpen: () => void;
}): JSX.Element {
  return (
    <article class="agent-card" data-status={agent.status}>
      <header class="agent-head">
        <div class="agent-title-row">
          <span class="agent-icon" aria-hidden="true">
            {agentGlyph(agent.role, agent.name)}
          </span>
          <div class="agent-titles">
            <h3 class="agent-name" title={agent.name || 'subagent'}>
              {agent.name || 'subagent'}
            </h3>
            <p class="agent-role" title={agent.role || 'agent'}>
              {agent.role || 'agent'}
              {showSession && agent.session_title ? ` · ${truncate(agent.session_title, 34)}` : ''}
            </p>
          </div>
          <span class="agent-live" title={liveLabel(agent, activity)}>
            {agent.status === 'running' ? liveLabel(agent, activity) : agentStatusLabel(agent.status)}
          </span>
        </div>
      </header>

      {agent.task ? <p class="agent-task">{truncate(agent.task, 180)}</p> : null}

      {agent.note ? (
        <p class="agent-note" title={agent.note}>
          {agent.note}
        </p>
      ) : null}

      {agent.progress !== null && agent.progress !== undefined ? (
        <ProgressBar value={agent.progress} label={`Progress for ${agent.name || 'subagent'}`} />
      ) : null}

      <div class="agent-card-foot">
        <span>
          {agent.iterations ?? 0} {(agent.iterations ?? 0) === 1 ? 'iteration' : 'iterations'}
        </span>
        {agent.background ? <span class="agent-badge">background</span> : null}
        <span>
          {agent.finished_at ? relativeTime(agent.finished_at, now) : elapsed(agent, now)}
        </span>
        {/* The card's one action, and a real button: the article itself is not
            in the tab order, so Enter/Space on Pause or Stop cannot reach it. */}
        <button
          type="button"
          class="agent-card-open"
          title={`Open details for ${agent.name || 'subagent'}`}
          aria-label={`Open details for ${agent.name || 'subagent'}`}
          onClick={onOpen}
        >
          <span class="agent-card-open-label">Details</span>
          <ArrowRightIcon size={12} />
        </button>
      </div>

      <AgentControls agent={agent} />
    </article>
  );
}

function AgentDetail({
  agent,
  activity,
  now,
}: {
  agent: AgentRow;
  activity: AgentActivity;
  now: number;
}): JSX.Element {
  const history = store.agentHistory.value?.id === agent.id ? store.agentHistory.value : null;
  const items = history ? mergeActivities(activityFromMessages(history.messages, agent.id, true), activity, true).items : activity.items;
  const running = agent.status === 'running';
  const steps = items.filter((item) => !(item.kind === 'message' && item.role === 'user'));
  const trimmed = history ? history.truncated : items.length >= AGENT_ACTIVITY_LIMIT;

  return (
    <div class="agent-detail">
      <div class="agent-detail-top">
        <button type="button" class="icon-button inspector-mini" onClick={actions.closeAgentDetail}>
          <ChevronLeftIcon size={14} />
          <span class="sr-only">Back to all agents</span>
        </button>
        <span class="agent-detail-kicker">Agent detail</span>
        <button
          type="button"
          class="icon-button inspector-mini"
          title="Open the session this agent belongs to"
          aria-label="Open session"
          onClick={() => void actions.openAgentSession(agent as AgentSummary)}
        >
          <ArrowRightIcon size={13} />
        </button>
      </div>

      <header class="agent-detail-head">
        <span class="agent-icon" aria-hidden="true">
          {agentGlyph(agent.role, agent.name)}
        </span>
        <div class="agent-titles">
          <h3 class="agent-name">{agent.name || 'subagent'}</h3>
          <p class="agent-role">{agent.role || 'agent'}</p>
        </div>
        <span class="agent-live" data-status={agent.status}>
          {isAgentLive(agent.status) ? liveLabel(agent, activity) : agentStatusLabel(agent.status)}
        </span>
      </header>

      <AgentControls agent={agent} />

      {agent.session_title ? (
        <p class="agent-detail-session" title={agent.session_title}>
          {agent.session_title}
        </p>
      ) : null}

      {agent.note ? (
        <div class="agent-note agent-detail-note">
          <span class="inspector-kicker">Says</span>
          <p>{agent.note}</p>
          {agent.progress !== null && agent.progress !== undefined ? (
            <ProgressBar
              value={agent.progress}
              label={`Progress for ${agent.name || 'subagent'}`}
            />
          ) : null}
        </div>
      ) : null}

      {agent.task ? (
        <details class="agent-bubble agent-bubble-task agent-detail-task">
          <summary>Task</summary>
          <div class="agent-bubble-body">{agent.task}</div>
        </details>
      ) : null}

      <dl class="agent-meta">
        <div>
          <dt>Model</dt>
          <dd title={agent.model || undefined}>{agent.model || '—'}</dd>
        </div>
        <div>
          <dt>Iterations</dt>
          <dd>
            {agent.iterations ?? 0} · {elapsed(agent, now)}
          </dd>
        </div>
        <div>
          <dt>Started</dt>
          <dd>{relativeTime(agent.created_at ?? now, now)}</dd>
        </div>
        <div>
          <dt>{agent.parent_agent_id ? 'Parent agent' : 'Parent run'}</dt>
          <dd class="mono" title={agent.parent_agent_id || agent.parent_run_id || agent.session_id}>
            {agent.parent_agent_id || agent.parent_run_id || agent.session_id}
          </dd>
        </div>
      </dl>

      {agent.owned_paths && agent.owned_paths.length > 0 ? (
        <div class="agent-paths">
          <span class="inspector-kicker">Owned paths</span>
          <ul>
            {agent.owned_paths.map((path) => (
              <li key={path} class="path-chip">
                {path}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <div class="agent-activity" aria-label="Agent transcript">
        <div class="agent-activity-head">
          <span class="inspector-kicker">Activity</span>
          <span class="agent-activity-count">
            {steps.length === 0
              ? 'waiting…'
              : `${steps.length}${trimmed ? '+' : ''} steps${running ? ' · live' : ''}`}
          </span>
        </div>
        {history?.truncated ? <div>
          <p>Earlier history is not loaded.</p>
          {history.before !== null ? <button type="button" class="button button-small" disabled={history.loading} onClick={() => void actions.loadOlderAgent()}>{history.loading ? 'Loading…' : 'Load older history'}</button> : <p>Older history is unavailable from this server.</p>}
          {history.error ? <p role="alert">{history.error}</p> : null}
        </div> : null}
        {store.agentDetailError.value ? <p role="alert">{store.agentDetailError.value}</p> : null}
        {steps.length === 0 ? (
          <p class="agent-activity-empty">
            {running
              ? 'No output yet — the subagent is starting up.'
              : 'No transcript was stored for this agent.'}
          </p>
        ) : (
          steps.map((item) => <ActivityItemView key={item.id} item={item} />)
        )}
      </div>

      {agent.summary ? (
        <div class="agent-summary">
          <span class="inspector-kicker">Result</span>
          <p>{agent.summary}</p>
        </div>
      ) : null}

      {agent.error ? (
        <div class="agent-error" role="alert">
          <span class="inspector-kicker">Error</span>
          <p>{agent.error}</p>
        </div>
      ) : null}
    </div>
  );
}

export function AgentsTab(): JSX.Element {
  const scope = store.agentScope.value;
  const filter = store.agentFilter.value;
  const now = store.clock.value;
  const loading = store.agentsLoading.value;
  const error = store.agentsError.value;
  const selectedId = store.selectedAgentId.value;
  const activities = store.agentActivities.value;
  const session = store.activeSession.value;
  const sessionAgents: AgentRow[] = store.chat.value.agents;
  const workspaceAgents: AgentRow[] = store.agentsAll.value;
  const agents = scope === 'session' ? sessionAgents : workspaceAgents;
  const counts = statusCounts(agents);
  const visible = sortAgents(filter === 'all' ? agents : agents.filter((agent) => agent.status === filter));

  // A large fan-out must not build every card at once, so the list renders a
  // page and offers the rest. Switching scope or filter is a new question, so the
  // next list starts collapsed again.
  const [showAll, setShowAll] = useState(false);
  useEffect(() => {
    setShowAll(false);
  }, [scope, filter]);
  const capped = !showAll && visible.length > AGENT_RENDER_CAP;
  const shown = capped ? visible.slice(0, AGENT_RENDER_CAP) : visible;

  // The tab is mounted lazily: make sure the workspace list is current when it
  // opens, and let SSE keep it live afterwards.
  useEffect(() => {
    void actions.refreshAgents();
  }, []);

  const selected =
    selectedId === null
      ? undefined
      : [...workspaceAgents, ...sessionAgents].find((agent) => agent.id === selectedId);

  if (selected) {
    return (
      <div class="inspector-section">
        <AgentDetail
          agent={{ session_title: session?.title, ...selected }}
          activity={activities[selected.id] ?? emptyActivity()}
          now={now}
        />
      </div>
    );
  }

  const refresh = (): void => {
    if (scope === 'session') void actions.refreshSessionAgents();
    else void actions.refreshAgents();
  };

  return (
    <div class="inspector-section">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">
          Agents
          <span class="inspector-kicker-note">
            {counts.running} running
            {counts.paused > 0 ? ` · ${counts.paused} paused` : ''} · {counts.all} total
          </span>
        </span>
        <button
          type="button"
          class="icon-button inspector-mini"
          aria-label="Refresh agents"
          title="Refresh agents"
          disabled={loading}
          onClick={refresh}
        >
          <RefreshIcon size={13} class={loading ? 'spin' : undefined} />
        </button>
      </div>

      <div class="agent-scope" role="group" aria-label="Agent scope">
        <button
          type="button"
          class={`agent-scope-button${scope === 'workspace' ? ' is-active' : ''}`}
          aria-pressed={scope === 'workspace'}
          onClick={() => actions.setAgentScope('workspace')}
        >
          Project
        </button>
        <button
          type="button"
          class={`agent-scope-button${scope === 'session' ? ' is-active' : ''}`}
          aria-pressed={scope === 'session'}
          onClick={() => actions.setAgentScope('session')}
        >
          This session
        </button>
      </div>

      <div class="agent-filters" role="group" aria-label="Filter agents by status">
        {FILTERS.map((chip) => (
          <button
            key={chip.id}
            type="button"
            class={`agent-filter${filter === chip.id ? ' is-active' : ''}`}
            aria-pressed={filter === chip.id}
            data-status={chip.id === 'all' ? undefined : chip.id}
            disabled={chip.id !== 'all' && counts[chip.id] === 0}
            onClick={() => actions.setAgentFilter(chip.id)}
          >
            {chip.label}
            <span class="agent-filter-count">{counts[chip.id]}</span>
          </button>
        ))}
      </div>

      {error ? (
        <p class="agent-detail-error" role="alert">
          {error}
        </p>
      ) : null}

      {visible.length === 0 ? (
        <AgentsEmpty filtered={agents.length > 0} />
      ) : (
        <div class="agent-list">
          {shown.map((agent) => (
            <AgentCard
              key={agent.id}
              agent={agent}
              activity={activities[agent.id] ?? emptyActivity()}
              now={now}
              showSession={scope === 'workspace'}
              onOpen={() => void actions.openAgentDetail(agent.id)}
            />
          ))}
          {capped ? (
            <div class="agent-list-more">
              <p class="agent-list-more-note">
                Showing {AGENT_RENDER_CAP} of {visible.length} agents. Live agents are listed
                first, so anything still running is already here.
              </p>
              <button
                type="button"
                class="button"
                onClick={() => setShowAll(true)}
                aria-label={`Show all ${visible.length} agents`}
              >
                Show all {visible.length}
              </button>
            </div>
          ) : null}
        </div>
      )}

      <p class="inspector-note">
        Subagents share this workspace. Owned paths are advisory, not file locks.
      </p>
    </div>
  );
}
