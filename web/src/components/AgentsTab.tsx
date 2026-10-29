import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { relativeTime } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { AgentsIcon, CpuIcon, EyeIcon, RefreshIcon, TerminalIcon } from './icons';

function agentGlyph(role: string, name: string): JSX.Element {
  const needle = `${role} ${name}`.toLowerCase();
  if (needle.includes('review') || needle.includes('audit')) return <EyeIcon size={15} />;
  if (needle.includes('test') || needle.includes('verif') || needle.includes('shell')) {
    return <TerminalIcon size={15} />;
  }
  if (needle.includes('agent') || needle.includes('build') || needle.includes('impl')) {
    return <CpuIcon size={15} />;
  }
  return <AgentsIcon size={15} />;
}

function progressWidth(status: string, iterations: number): string {
  if (status === 'running') return '40%';
  if (status === 'completed' || status === 'failed') return '100%';
  if (status === 'cancelled' || status === 'interrupted') return '100%';
  return iterations > 0 ? '15%' : '6%';
}

export function AgentsTab(): JSX.Element {
  const agents = store.chat.value.agents;
  const now = store.clock.value;
  const [refreshing, setRefreshing] = useState(false);
  const completed = agents.filter((agent) => agent.status === 'completed').length;

  const refresh = async (): Promise<void> => {
    setRefreshing(true);
    try {
      await actions.refreshSessionAgents();
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div class="inspector-section">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">
          Agent activity
          {completed > 0 ? <span class="inspector-kicker-note">{completed} completed</span> : null}
        </span>
        <button
          type="button"
          class="icon-button inspector-mini"
          aria-label="Refresh subagents"
          title="Refresh subagents"
          disabled={refreshing}
          onClick={() => void refresh()}
        >
          <RefreshIcon size={13} class={refreshing ? 'spin' : undefined} />
        </button>
      </div>

      {agents.length === 0 ? (
        <div class="empty-inspector">
          <p class="empty-inspector-title">No subagents</p>
          <p>
            Nothing has been delegated in this session. Subagents run in the same workspace; their task, status,
            iterations and owned paths will appear here live.
          </p>
          <button type="button" class="button button-small" onClick={actions.openSettings}>
            Subagent settings
          </button>
        </div>
      ) : (
        <div class="agent-list">
          {agents.map((agent) => (
            <article class="agent-card" key={agent.id} data-status={agent.status}>
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
                    </p>
                  </div>
                  <span class="agent-live">{agent.status}</span>
                </div>
              </header>

              {agent.task ? <p class="agent-task">{agent.task}</p> : null}

              <div class="agent-progress" aria-hidden="true">
                <div
                  class="agent-progress-bar"
                  style={{ width: progressWidth(agent.status, agent.iterations) }}
                />
              </div>

              <div class="agent-card-foot">
                <span>{agent.iterations} iterations</span>
                <span>{agent.finished_at ? relativeTime(agent.finished_at, now) : 'in progress'}</span>
              </div>

              <dl class="agent-meta">
                <div>
                  <dt>Model</dt>
                  <dd title={agent.model || undefined}>{agent.model || '—'}</dd>
                </div>
                <div>
                  <dt>{agent.parent_run_id ? 'Parent run' : 'Session'}</dt>
                  <dd class="mono" title={agent.parent_run_id ?? agent.session_id}>
                    {agent.parent_run_id ?? agent.session_id}
                  </dd>
                </div>
              </dl>

              {agent.owned_paths.length > 0 ? (
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

              {agent.summary ? (
                <div class="agent-summary">
                  <span class="inspector-kicker">Summary</span>
                  <p>{agent.summary}</p>
                </div>
              ) : null}

              {agent.error ? (
                <div class="agent-error" role="alert">
                  <span class="inspector-kicker">Error</span>
                  <p>{agent.error}</p>
                </div>
              ) : null}
            </article>
          ))}
        </div>
      )}

      <p class="inspector-note">Subagents share this workspace. Owned paths are advisory, not file locks.</p>
    </div>
  );
}