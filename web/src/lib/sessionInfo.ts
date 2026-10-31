/* Pure derivations for the inspector's Session tab.
 *
 * Kept out of the component so every number and every "unknown" rule is
 * testable without rendering, and so the rules live in one place: the tab, a
 * test and (if it ever needs one) a tooltip all read the same function.
 *
 * The governing rule is that a metric the server did not report is *absent*,
 * never zero. A 0% cache hit rate and "the endpoint does not report caching" are
 * different facts, and a UI that renders them identically teaches the reader to
 * distrust every other number on the page.
 */

import type { SessionInfo, SessionInfoAgent, UsageTotals } from '../api/types';

/** `1 234` / `12.4k` / `1.2M` — the scale a token count is read at. */
export function formatTokens(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  if (value < 0) return '—';
  if (value < 1_000) return String(Math.round(value));
  if (value < 1_000_000) {
    const thousands = value / 1_000;
    return `${thousands >= 100 ? Math.round(thousands) : thousands.toFixed(1)}k`;
  }
  const millions = value / 1_000_000;
  return `${millions >= 10 ? Math.round(millions) : millions.toFixed(1)}M`;
}

/** A ratio as a percentage; `null` stays `—` rather than becoming `0%`. */
export function formatPercent(ratio: number | null | undefined, digits = 1): string {
  if (ratio === null || ratio === undefined || !Number.isFinite(ratio)) return '—';
  const percent = Math.min(1, Math.max(0, ratio)) * 100;
  // Above 10% one decimal is signal; below it, it is noise.
  return percent >= 10 ? `${percent.toFixed(digits)}%` : `${percent.toFixed(1)}%`;
}

/** `3m 12s` / `1h 04m` / `—` — a duration at the scale a human reads it. */
export function formatSpan(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) return '—';
  if (ms < 1_000) return `${Math.round(ms)}ms`;
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    const rest = seconds % 60;
    return rest === 0 ? `${minutes}m` : `${minutes}m ${rest}s`;
  }
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h ${String(rest).padStart(2, '0')}m`;
}

/** An empty total, for a scope with no recorded call. */
export function emptyUsage(): UsageTotals {
  return {
    calls: 0,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    cached_known_calls: 0,
    cache_hit_ratio: null,
    last_input_tokens: 0,
  };
}

/**
 * What to say about the prompt cache.
 *
 * Three distinct answers, which is the whole point: a ratio when the endpoint
 * reported one, "not reported" when no call did, and "no calls yet" before
 * anything ran. Never "0%".
 */
export function cacheLabel(usage: UsageTotals): string {
  if (usage.calls === 0) return 'No calls yet';
  if (usage.cache_hit_ratio === null) {
    return usage.cached_known_calls === 0 ? 'Not reported' : 'Unknown';
  }
  return formatPercent(usage.cache_hit_ratio);
}

/** `cached / input` as the tooltip explains it, or why there is no figure. */
export function cacheDetail(usage: UsageTotals): string {
  if (usage.calls === 0) return 'This session has not called a model yet.';
  if (usage.cache_hit_ratio === null) {
    return (
      'The endpoint did not report prompt-cache hits for any call, so there is no ' +
      'hit rate. That is not the same as a 0% hit.'
    );
  }
  if (usage.cached_known_calls < usage.calls) {
    return (
      `${formatTokens(usage.cached_tokens)} of ${formatTokens(usage.input_tokens)} prompt ` +
      `tokens were cached. Only ${usage.cached_known_calls} of ${usage.calls} calls ` +
      `reported a figure, so this rate covers those calls.`
    );
  }
  return `${formatTokens(usage.cached_tokens)} of ${formatTokens(usage.input_tokens)} prompt tokens were served from the prefix cache.`;
}

/** How full the context window is, or why that cannot be answered. */
export interface ContextFill {
  /** 0..1, or `null` when either number is missing. */
  fraction: number | null;
  /** The fill band, used to colour the meter. */
  tone: 'calm' | 'warn' | 'hot';
  /** `41k / 128k` — both real numbers, or `—`. */
  label: string;
  /** Why the percentage is missing, when it is. */
  note: string | null;
}

export function contextFill(info: Pick<SessionInfo, 'context'>): ContextFill {
  const { used, window, model } = info.context;
  const known = typeof window === 'number' && Number.isFinite(window) && window > 0;
  if (!known) {
    return {
      fraction: null,
      tone: 'calm',
      label: used > 0 ? formatTokens(used) : '—',
      note: `The model entry for ${model || 'this session'} declares no context window, so a percentage would be invented.`,
    };
  }
  if (used <= 0) {
    return {
      fraction: null,
      tone: 'calm',
      label: `— / ${formatTokens(window)}`,
      note: 'No model call has reported a prompt size yet.',
    };
  }
  const fraction = Math.min(1, Math.max(0, used / window));
  return {
    fraction,
    tone: fraction >= 0.9 ? 'hot' : fraction >= 0.7 ? 'warn' : 'calm',
    label: `${formatTokens(used)} / ${formatTokens(window)}`,
    note: null,
  };
}

/** `input + output` — the tokens a scope was billed for in total. */
export function totalTokens(usage: UsageTotals): number {
  return usage.input_tokens + usage.output_tokens;
}

/**
 * `assistant / user` — model turns per prompt, the ratio that says whether a
 * session was one long tool loop or a series of short exchanges.
 */
export function turnsPerPrompt(info: Pick<SessionInfo, 'messages'>): number | null {
  const prompts = info.messages.main_by_role['user'] ?? 0;
  const answers = info.messages.main_by_role['assistant'] ?? 0;
  if (prompts <= 0) return null;
  return answers / prompts;
}

/** The turn counts the tab shows, with the absence cases named. */
export interface TurnCounts {
  /** Prompts the user actually sent (main conversation only). */
  prompts: number;
  /** Runs the agent started, which can exceed prompts via retry or a note. */
  runs: number;
  /** Times a provider call was made by the main agent. */
  calls: number;
  /** Assistant turns in the main conversation. */
  answers: number;
}

export function turnCounts(info: Pick<SessionInfo, 'messages' | 'usage' | 'runs'>): TurnCounts {
  return {
    prompts: info.messages.main_by_role['user'] ?? 0,
    runs: info.runs.total,
    calls: info.usage.main.calls,
    answers: info.messages.main_by_role['assistant'] ?? 0,
  };
}

/** What a run-status map is worth showing as: a stable, readable order. */
const RUN_STATUS_ORDER = [
  'completed',
  'running',
  'failed',
  'cancelled',
  'interrupted',
  'limit_reached',
];

/** Status counts in a deliberate order, so the row does not reshuffle. */
export function orderedStatuses(byStatus: Record<string, number>): Array<[string, number]> {
  const seen = new Set(RUN_STATUS_ORDER);
  const ordered: Array<[string, number]> = [];
  for (const status of RUN_STATUS_ORDER) {
    const count = byStatus[status] ?? 0;
    if (count > 0) ordered.push([status, count]);
  }
  // A status this build does not know is still a fact; show it last.
  for (const [status, count] of Object.entries(byStatus)) {
    if (!seen.has(status) && count > 0) ordered.push([status, count]);
  }
  return ordered;
}

/** The subagents ordered for reading: running first, then newest. */
export function orderAgents(agents: SessionInfoAgent[]): SessionInfoAgent[] {
  return [...agents].sort((a, b) => {
    const aRunning = a.status === 'running' ? 1 : 0;
    const bRunning = b.status === 'running' ? 1 : 0;
    if (aRunning !== bRunning) return bRunning - aRunning;
    return b.created_at - a.created_at;
  });
}

/**
 * Share of total tokens one scope represents.
 *
 * `null` when the session has not been billed anything, so a caller shows
 * "unknown" rather than a 0% bar.
 */
export function shareOfSession(usage: UsageTotals, info: Pick<SessionInfo, 'usage'>): number | null {
  const total = totalTokens(info.usage.total);
  if (total <= 0) return null;
  return Math.min(1, totalTokens(usage) / total);
}

/** `3 of 5 agents finished` — a sentence, so a zero reads as a fact. */
export function agentSummary(agents: SessionInfoAgent[]): string {
  if (agents.length === 0) return 'No subagents';
  const running = agents.filter((agent) => agent.status === 'running').length;
  const failed = agents.filter((agent) => agent.status === 'failed').length;
  const done = agents.filter((agent) => agent.status === 'completed').length;
  const parts = [`${done}/${agents.length} done`];
  if (running > 0) parts.push(`${running} running`);
  if (failed > 0) parts.push(`${failed} failed`);
  return parts.join(' · ');
}

/** Human labels for the role keys the server reports. */
export function roleLabel(role: string): string {
  switch (role) {
    case 'user':
      return 'your prompts';
    case 'assistant':
      return 'agent turns';
    case 'tool':
      return 'tool results';
    case 'system':
      return 'system';
    default:
      return role;
  }
}

/** A cost estimate from per-million prices, or `null` when either is missing. */
export interface CostEstimate {
  input: number;
  output: number;
  cached: number;
  total: number;
  /** True when a price was missing, so the total is a floor rather than a total. */
  partial: boolean;
}

/**
 * `usage` priced with a model entry's per-million rates.
 *
 * Returns `null` when the model declares neither price: a cost of `$0.00` for an
 * unpriced model would be a lie, and "free" is a much more dangerous claim than
 * "unknown". Cached tokens are priced at the input rate — the API does not carry
 * a separate cached rate, and pretending it does would be guessing.
 */
export function estimateCost(
  usage: UsageTotals,
  prices: { input?: number | null; output?: number | null },
): CostEstimate | null {
  const input = prices.input;
  const output = prices.output;
  const hasInput = typeof input === 'number' && Number.isFinite(input) && input >= 0;
  const hasOutput = typeof output === 'number' && Number.isFinite(output) && output >= 0;
  if (!hasInput && !hasOutput) return null;

  const perMillion = 1_000_000;
  const inputCost = hasInput ? (usage.input_tokens / perMillion) * (input as number) : 0;
  const outputCost = hasOutput ? (usage.output_tokens / perMillion) * (output as number) : 0;
  // Cached tokens are a *subset* of the prompt, already included in
  // `input_tokens`; pricing them again would double-count, so the figure is
  // reported for display only and deliberately excluded from the total.
  const cachedCost = hasInput ? (usage.cached_tokens / perMillion) * (input as number) : 0;
  return {
    input: inputCost,
    output: outputCost,
    cached: cachedCost,
    total: inputCost + outputCost,
    partial: !hasInput || !hasOutput,
  };
}

/** `$0.0123` — small amounts keep their significant digits. */
export function formatCost(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  if (value === 0) return '$0';
  if (value < 0.01) return `$${value.toFixed(4)}`;
  if (value < 1) return `$${value.toFixed(3)}`;
  return `$${value.toFixed(2)}`;
}
