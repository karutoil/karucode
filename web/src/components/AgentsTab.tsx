import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import * as actions from '../state/actions';
import { agentStatusTone } from '../state/selectors';
import * as store from '../state/store';
import { AgentsIcon, RefreshIcon } from './icons';

export function AgentsTab(): JSX.Element {
  const agents = store.chat.value.agents;
  const [refreshing, setRefreshing] = useState(false);

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
      <div class="section-toolbar">
        <div class="section-heading">
          <span class="eyebrow">Agents</span>
          <span class="section-path">
            {agents.length} subagent{agents.length === 1 ? '' : 's'}
          </span>
        </div>
        <button
          type="button"
          class="icon-button"
          aria-label="Refresh subagents"
          title="Refresh subagents"
          disabled={refreshing}
          onClick={() => void refresh()}
        >
          <RefreshIcon class={refreshing ? 'spin' : undefined} />
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
          {agents.map((agent) => {
            const tone = agentStatusTone(agent.status);
            return (
              <article class="agent-card" key={agent.id} data-status={agent.status}>
                <header class="agent-head">
                  <div class="agent-title-row">
                    <span class="agent-icon" aria-hidden="true">
                      <AgentsIcon size={15} />
                    </span>
                    <h3 class="agent-name">{agent.name || 'subagent'}</h3>
                    <span class={`chip chip-${tone}`}>
                      <span class={`status-dot status-${tone}`} aria-hidden="true" />
                      {agent.status}
                    </span>
                  </div>
                  <p class="agent-role">{agent.role || 'agent'}</p>
                </header>

                {agent.task ? <p class="agent-task">{agent.task}</p> : null}

                <dl class="agent-meta">
                  <div>
                    <dt>Iterations</dt>
                    <dd>{agent.iterations}</dd>
                  </div>
                  <div>
                    <dt>Model</dt>
                    <dd>{agent.model || '—'}</dd>
                  </div>
                  {agent.parent_run_id ? (
                    <div>
                      <dt>Parent run</dt>
                      <dd class="mono">{agent.parent_run_id}</dd>
                    </div>
                  ) : null}
                </dl>

                {agent.owned_paths.length > 0 ? (
                  <div class="agent-paths">
                    <span class="eyebrow">Owned paths</span>
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
                    <span class="eyebrow">Summary</span>
                    <p>{agent.summary}</p>
                  </div>
                ) : null}

                {agent.error ? (
                  <div class="agent-error" role="alert">
                    <span class="eyebrow">Error</span>
                    <p>{agent.error}</p>
                  </div>
                ) : null}
              </article>
            );
          })}
        </div>
      )}

      <p class="inspector-note">Subagents share this workspace. Owned paths are advisory, not file locks.</p>
    </div>
  );
}