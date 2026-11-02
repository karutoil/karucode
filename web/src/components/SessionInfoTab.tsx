import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import type { SessionInfo, SessionInfoAgent, UsageTotals } from '../api/types';
import {
  cacheDetail,
  cacheLabel,
  contextFill,
  emptyUsage,
  estimateCost,
  formatCost,
  formatPercent,
  formatSpan,
  formatTokens,
  orderAgents,
  orderedStatuses,
  shareOfSession,
  totalTokens,
  turnCounts,
  turnsPerPrompt,
} from '../lib/sessionInfo';
import { formatDuration, relativeTime } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { BoltIcon, CpuIcon, RefreshIcon } from './icons';

/**
 * The inspector's **Session** tab: what this session *is* and what it *cost*.
 *
 * Every number here is a server-side fact rather than a fold over the transcript
 * on screen, because the transcript is a bounded window and token totals are
 * persisted per provider call (a `usage` event is live only). The tab therefore
 * answers three questions the chat cannot:
 *
 *   * how much work was this — messages, turns, tool calls, runs, working time;
 *   * where did the tokens go — input, output, cached, split per agent;
 *   * how close is the window — the newest prompt against the model's own
 *     `context_window`.
 *
 * The rule it keeps throughout: a metric the server did not report is *absent*
 * ("—", "Not reported"), never zero. A fabricated 0% cache hit rate teaches the
 * reader to distrust the other numbers.
 */

/** A metric tile: one big number over a label, with an optional note. */
function Metric({
  label,
  value,
  sub,
  title,
  tone,
}: {
  label: string;
  value: string;
  sub?: string;
  title?: string;
  tone?: 'ok' | 'warn' | 'danger';
}): JSX.Element {
  return (
    <div class="metric si-metric" data-tone={tone}>
      <div class="metric-num" title={title ?? undefined}>
        {value}
        {sub ? <span class="metric-sub">{sub}</span> : null}
      </div>
      <div class="metric-label">{label}</div>
    </div>
  );
}

/** A labelled row in one of the small definition lists. */
function Row({
  label,
  value,
  title,
}: {
  label: string;
  value: JSX.Element | string;
  title?: string;
}): JSX.Element {
  return (
    <div>
      <dt>{label}</dt>
      <dd title={title}>{value}</dd>
    </div>
  );
}

/** The context-window meter: fill, percentage, and why it may be absent. */
function ContextSection({ info }: { info: SessionInfo }): JSX.Element {
  const fill = contextFill(info);
  const window = info.context.window;
  return (
    <section class="si-section" aria-label="Main agent context">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Main agent context</span>
        <span class="inspector-kicker-note">{info.context.model || 'no model'}</span>
      </div>

      <div class="si-window">
        <div
          class="si-window-meter"
          role={fill.fraction === null ? undefined : 'meter'}
          aria-valuemin={fill.fraction === null ? undefined : 0}
          aria-valuemax={fill.fraction === null ? undefined : 100}
          aria-valuenow={
            fill.fraction === null ? undefined : Math.round(fill.fraction * 100)
          }
          aria-label={fill.fraction === null ? undefined : 'Context window used'}
          data-tone={fill.tone}
        >
          <span
            class="si-window-fill"
            style={{ width: fill.fraction === null ? '0%' : `${Math.max(2, fill.fraction * 100)}%` }}
          />
        </div>
        <div class="si-window-legend">
          <span class="si-window-percent">
            {fill.fraction === null ? '—' : formatPercent(fill.fraction)}
          </span>
          <span class="si-window-numbers">{fill.label}</span>
        </div>
      </div>
      {fill.note ? <p class="si-note">{fill.note}</p> : null}
      <p class="si-note">
        The fill is the newest model call's prompt — the provider's own count, not an estimate
        from the transcript. It is a different number from the token totals below, which add up
        every call.
        {window !== null ? ` The window is this model's declared ${formatTokens(window)}.` : ''}
      </p>
    </section>
  );
}

/** Input / output / cached / cache-hit tiles for one scope. */
function UsageTiles({ usage, label }: { usage: UsageTotals; label: string }): JSX.Element {
  return (
    <div class="si-tiles" role="group" aria-label={`${label} tokens`}>
      <Metric label="Input" value={formatTokens(usage.input_tokens)} sub={`${usage.calls} calls`} />
      <Metric label="Output" value={formatTokens(usage.output_tokens)} />
      <Metric label="Cached" value={formatTokens(usage.cached_tokens)} title={cacheDetail(usage)} />
      <Metric
        label="Cache hit"
        value={cacheLabel(usage)}
        title={cacheDetail(usage)}
        tone={usage.cache_hit_ratio !== null && usage.cache_hit_ratio >= 0.5 ? 'ok' : undefined}
      />
    </div>
  );
}

/** The blended cache hit rate across main and every subagent. */
function UsageSection({ info }: { info: SessionInfo }): JSX.Element {
  const main = info.usage.main ?? emptyUsage();
  const total = info.usage.total ?? emptyUsage();
  const delegated = totalTokens(total) - totalTokens(main);
  // The main agent's share of the session; the rest is delegated. Passing the
  // *total* here and inverting it would always be 100%, which reads as "nothing
  // was delegated" even when a subagent spent a quarter of the tokens.
  const share = shareOfSession(main, info);

  // The model entry's prices, when the user configured any. A cost is only shown
  // for a model that declares at least one rate - "$0.00" for an unpriced model
  // would be a stronger claim than "unknown".
  const entry = store.selectedModelEntry.value;
  const sessionModel = store.config.value?.providers
    .flatMap((profile) => profile.models)
    .find((model) => model.id === info.context.model);
  const prices = sessionModel ?? entry;
  const cost = estimateCost(total, {
    input: prices?.cost_input ?? null,
    output: prices?.cost_output ?? null,
  });

  return (
    <section class="si-section" aria-label="Token usage">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Tokens</span>
        <span class="inspector-kicker-note">
          {total.calls} {total.calls === 1 ? 'call' : 'calls'}
        </span>
      </div>

      <UsageTiles usage={main} label="Main agent" />

      <dl class="si-meta">
        <Row
          label="Whole session"
          value={`${formatTokens(main.input_tokens + main.output_tokens)} main${
            delegated > 0 ? ` · ${formatTokens(delegated)} delegated` : ''
          }`}
        />
        <Row
          label="Blended cache"
          value={cacheLabel(total)}
          title={cacheDetail(total)}
        />
        {cost ? (
          <Row
            label="Est. cost"
            value={`${formatCost(cost.total)}${cost.partial ? ' +' : ''}`}
            title={
              `Input ${formatCost(cost.input)} at the model's input rate, output ` +
              `${formatCost(cost.output)} at its output rate.${
                cost.partial ? ' One rate is not declared, so this is a floor.' : ''
              } Cached tokens are already inside the input count and are not billed twice.`
            }
          />
        ) : (
          <Row
            label="Est. cost"
            value="—"
            title="This model entry declares no per-million prices, so there is no cost to show rather than a misleading zero."
          />
        )}
        {share !== null && delegated > 0 ? (
          <Row
            label="Delegated"
            value={`${formatPercent(1 - share, 0)} of tokens`}
            title="The share of this session's tokens spent by subagents rather than by the main agent."
          />
        ) : null}
      </dl>
    </section>
  );
}

/** Messages, turns, tool calls and working time. */
function WorkSection({ info }: { info: SessionInfo }): JSX.Element {
  const counts = turnCounts(info);
  const perPrompt = turnsPerPrompt(info);
  const stats = info.stats;
  const tools = info.tools;

  return (
    <section class="si-section" aria-label="Work">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Work</span>
        <span class="inspector-kicker-note">
          {stats.messages_total} {stats.messages_total === 1 ? 'message' : 'messages'}
        </span>
      </div>

      <div class="si-tiles">
        <Metric label="Your prompts" value={String(counts.prompts)} />
        <Metric label="Agent turns" value={String(counts.answers)} sub={perPrompt !== null ? `${perPrompt.toFixed(1)}×` : undefined} />
        <Metric label="Model calls" value={String(counts.calls)} />
        <Metric label="Tool calls" value={formatTokens(tools.total)} />
      </div>

      <dl class="si-meta">
        <Row
          label="Runs"
          value={`${stats.runs_total}${
            perPrompt !== null && stats.runs_total !== counts.prompts
              ? ` for ${counts.prompts} prompt${counts.prompts === 1 ? '' : 's'}`
              : ''
          }`}
          title="A run is one agent loop: a prompt, a retry or a note can each start their own."
        />
        <Row
          label="Outcome"
          value={
            orderedStatuses(info.runs.by_status)
              .map(([status, count]) => `${count} ${status.replace('_', ' ')}`)
              .join(' · ') || '—'
          }
        />
        <Row
          label="Working time"
          value={formatSpan(info.runs.working_ms)}
          title="Time a run was actually in flight, summed. Idle gaps between runs are excluded, and a run still going is not counted until it ends."
        />
        <Row
          label="Messages"
          value={`${stats.messages_main} main${
            stats.messages_subagent > 0 ? ` · ${stats.messages_subagent} delegated` : ''
          }`}
          title="Whole-session counts, not the transcript page: a long session's real totals."
        />
      </dl>

      {tools.by_name.length > 0 ? (
        <div class="si-tools">
          <span class="inspector-kicker">Tools called</span>
          <ul class="si-tool-list">
            {tools.by_name.map((tool) => (
              <li key={tool.name} class="si-tool">
                <span class="si-tool-name mono" title={tool.name}>
                  {tool.name}
                </span>
                <span class="si-tool-count">{tool.count}</span>
              </li>
            ))}
          </ul>
          {tools.truncated ? (
            <p class="si-note">
              Showing the {tools.by_name.length} most-called tools; the total above counts all{' '}
              {tools.total}.
            </p>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

/** One subagent row: who it was, how long, what it cost. */
function AgentRowView({ agent, info }: { agent: SessionInfoAgent; info: SessionInfo }): JSX.Element {
  const share = shareOfSession(agent.usage, info);
  const running = agent.status === 'running';
  return (
    <li class="si-agent" data-status={agent.status}>
      <div class="si-agent-head">
        <span class="si-agent-icon" aria-hidden="true">
          <CpuIcon size={13} />
        </span>
        <div class="si-agent-titles">
          <span class="si-agent-name" title={agent.name || 'subagent'}>
            {agent.name || 'subagent'}
          </span>
          <span class="si-agent-role" title={agent.role}>
            {agent.role || 'agent'}
          </span>
        </div>
        <span class="si-agent-status" data-status={agent.status}>
          {running ? 'running' : agent.status}
        </span>
      </div>
      <dl class="si-agent-meta">
        <div>
          <dt>Tokens</dt>
          <dd title={`Cache hit: ${cacheDetail(agent.usage)}`}>
            {formatTokens(totalTokens(agent.usage))}
            {share !== null ? <span class="si-agent-share"> · {formatPercent(share, 0)}</span> : null}
          </dd>
        </div>
        <div>
          <dt>Calls</dt>
          <dd>{agent.usage.calls}</dd>
        </div>
        <div>
          <dt>Cache</dt>
          <dd>{cacheLabel(agent.usage)}</dd>
        </div>
        <div>
          <dt>Messages</dt>
          <dd>{agent.messages}</dd>
        </div>
        <div>
          <dt>Took</dt>
          <dd>{running ? '—' : formatSpan(agent.duration_ms ?? 0)}</dd>
        </div>
        <div>
          <dt>Iterations</dt>
          <dd>{agent.iterations}</dd>
        </div>
      </dl>
    </li>
  );
}

function AgentsSection({ info }: { info: SessionInfo }): JSX.Element | null {
  const agents = orderAgents(info.agents);
  if (agents.length === 0) return null;
  const now = store.clock.value;
  const running = agents.filter((agent) => agent.status === 'running').length;
  const delegated = agents.reduce(
    (sum, agent) => sum + agent.usage.input_tokens + agent.usage.output_tokens,
    0,
  );
  // The newest *activity*, not the newest row: a running agent's `finished_at`
  // is null and its `created_at` is what "most recent" should mean for it.
  const newestAt = Math.max(...agents.map((agent) => agent.finished_at ?? agent.created_at));

  return (
    <section class="si-section" aria-label="Subagents">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">
          Subagents
          <span class="inspector-kicker-note">
            {agents.length} total{running > 0 ? ` · ${running} running` : ''}
          </span>
        </span>
      </div>
      <dl class="si-meta">
        <Row label="Delegated tokens" value={formatTokens(delegated)} />
        <Row label="Most recent" value={relativeTime(newestAt, now)} />
      </dl>
      <ul class="si-agent-list">
        {agents.map((agent) => (
          <AgentRowView key={agent.id} agent={agent} info={info} />
        ))}
      </ul>
      <p class="si-note">
        A subagent has its own context window and its own transcript, so its token share is
        counted here and never folded into the main agent's window fill.
      </p>
    </section>
  );
}

/** The most recent runs, as a short timeline. */
function RecentRuns({ info }: { info: SessionInfo }): JSX.Element | null {
  const runs = info.runs.recent;
  if (runs.length === 0) return null;
  const now = store.clock.value;
  return (
    <section class="si-section" aria-label="Recent runs">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Recent runs</span>
        <span class="inspector-kicker-note">{runs.length}</span>
      </div>
      <ul class="si-run-list">
        {runs.map((run) => {
          const duration =
            // `!= null`, not `!== null`: `finished_at` is `skip_serializing_if =
            // "Option::is_none"` in Rust, so a run still in flight sends no key at
            // all. `undefined !== null` was true, and the row then rendered
            // `NaNms` from `undefined - started_at`.
            run.finished_at != null ? formatDuration(run.finished_at - run.started_at) : null;
          return (
            <li key={run.id} class="si-run" data-status={run.status}>
              <span class="si-run-status">{run.status.replace('_', ' ')}</span>
              <span class="si-run-prompt" title={run.prompt}>
                {run.prompt.trim() || '—'}
              </span>
              <span class="si-run-meta">
                {duration ? `${duration} · ` : ''}
                {relativeTime(run.finished_at ?? run.started_at, now)}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function EmptyInfo(): JSX.Element {
  return (
    <div class="empty-inspector">
      <p class="empty-inspector-title">No session open</p>
      <p>
        Open or create a session and this tab describes it: how many messages and runs it has,
        what it spent on input and output tokens, how much of that the prompt cache served, and
        how full the model's context window is.
      </p>
    </div>
  );
}

function ErrorInfo({ message }: { message: string }): JSX.Element {
  return (
    <div class="si-error" role="alert">
      <span class="inspector-kicker">Could not read this session's numbers</span>
      <p>{message}</p>
      <button
        type="button"
        class="button button-small"
        onClick={() => void actions.refreshSessionInfo()}
      >
        Try again
      </button>
    </div>
  );
}

export function SessionInfoTab(): JSX.Element {
  const sessionId = store.currentSessionId.value;
  const info = store.sessionInfo.value;
  const loading = store.sessionInfoLoading.value;
  const error = store.sessionInfoError.value;
  const tick = store.sessionInfoTick.value;

  // The tab is mounted lazily, so the first read happens on open. `tick` is
  // bumped by every usage/run/agent event, so an open tab keeps up with the run
  // without polling: the fetch itself is coalesced in `actions.ts`.
  useEffect(() => {
    if (!sessionId) return;
    void actions.refreshSessionInfo();
  }, [sessionId, tick]);

  if (!sessionId) return <div class="inspector-section"><EmptyInfo /></div>;
  if (error) return <div class="inspector-section"><ErrorInfo message={error} /></div>;

  // A read for another session - or the first read - shows a loading line rather
  // than a page of zeros, which would read as "this session cost nothing".
  if (!info || info.session.id !== sessionId) {
    return (
      <div class="inspector-section">
        <div class="inspector-kicker-row">
          <span class="inspector-kicker">Session</span>
        </div>
        <p class="si-note">Reading this session's numbers…</p>
      </div>
    );
  }

  const refreshing = loading;
  const summaryTitle =
    `${info.session.title || 'Untitled session'} — ${info.stats.messages_total} messages, ` +
    `${info.runs.total} runs, ${formatTokens(info.usage.total.input_tokens)} input tokens`;

  return (
    <div class="inspector-section si-info" aria-busy={refreshing}>
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">
          Session
          {info.running ? <span class="si-live">live</span> : null}
        </span>
        <button
          type="button"
          class="icon-button inspector-mini"
          aria-label="Refresh session information"
          title={summaryTitle}
          disabled={refreshing}
          onClick={() => void actions.refreshSessionInfo()}
        >
          <RefreshIcon size={13} class={refreshing ? 'spin' : undefined} />
        </button>
      </div>

      <ContextSection info={info} />
      <UsageSection info={info} />
      <WorkSection info={info} />
      <AgentsSection info={info} />
      <RecentRuns info={info} />

      <p class="inspector-note">
        <BoltIcon size={12} /> Every figure is read from the server, so it survives a reload and
        covers the whole session — not only the transcript pages this browser has loaded.
      </p>
    </div>
  );
}

/**
 * The Session tab's hover text: the one number worth seeing without opening the
 * panel, as a sentence because a tab label has no room for a labelled pill.
 */
export const sessionInfoTooltip = (info: SessionInfo | null): string => {
  if (!info) return 'Session information';
  const fill = contextFill(info);
  return `Session information — ${
    fill.fraction === null ? 'window unknown' : `${formatPercent(fill.fraction)} of the context window`
  }`;
};
