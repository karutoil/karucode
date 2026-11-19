import type { CatalogLookupMatch, CatalogModel, ModelEntry } from '../../api/types';
import type { StatusDotTone } from '../../ui';
import type { AddProviderDraft, TriState } from './types';

/** Build a `ModelEntry` from a catalog lookup match (or bare id when unmatched). */
export function modelEntryFromLookup(modelId: string, detected: CatalogLookupMatch | null): ModelEntry {
  return detected
    ? {
        id: modelId,
        name: detected.model.name.trim() === '' ? null : detected.model.name.trim(),
        context_window: detected.model.context_window ?? null,
        max_output_tokens: detected.model.max_output_tokens ?? null,
        tool_call: detected.model.tool_call ?? null,
        reasoning: detected.model.reasoning ?? null,
        vision: detected.model.vision ?? null,
        cost_input: detected.model.cost_input ?? null,
        cost_output: detected.model.cost_output ?? null,
        catalog_provider: detected.provider,
      }
    : { id: modelId, name: null };
}

/** Merge refreshed catalog metadata onto an existing `ModelEntry` without touching its display name. */
export function applyRefreshedMetadata(entry: ModelEntry, detected: CatalogLookupMatch): ModelEntry {
  return {
    ...entry,
    context_window: detected.model.context_window ?? null,
    max_output_tokens: detected.model.max_output_tokens ?? null,
    tool_call: detected.model.tool_call ?? null,
    reasoning: detected.model.reasoning ?? null,
    vision: detected.model.vision ?? null,
    cost_input: detected.model.cost_input ?? null,
    cost_output: detected.model.cost_output ?? null,
    catalog_provider: detected.provider,
  };
}

export const DEFAULT_PROVIDER_ID = 'openai_compatible';
/** Mirrors the server's bound on a curated model list (kc-server validate_profiles). */
export const MAX_MODELS_PER_PROFILE = 64;

export const MODEL_TOKEN_LIMIT = 100_000_000;
export const MODEL_COST_LIMIT = 1_000_000;
export const MODEL_NAME_LIMIT = 200;

export function statusTone(status: string): StatusDotTone {
  if (status === 'ready' || status === 'connected') return 'ok';
  if (status === 'failed') return 'danger';
  if (status === 'starting' || status === 'disabling' || status === 'reconnecting') return 'warn';
  return 'muted';
}

export function emptyDraft(): AddProviderDraft {
  return { id: '', name: '', provider: '', baseUrl: '', apiKey: '' };
}

export function formatTokenCount(value: number | null | undefined): string | null {
  if (value === null || value === undefined || !Number.isFinite(value) || value <= 0) return null;
  if (value >= 1_000_000) {
    const millions = value / 1_000_000;
    return `${Number.isInteger(millions) ? millions : millions.toFixed(1)}M`;
  }
  if (value >= 1_000) {
    const thousands = value / 1_000;
    return `${Number.isInteger(thousands) ? thousands : thousands.toFixed(1)}k`;
  }
  return String(value);
}

export function formatCost(value: number | null | undefined): string | null {
  if (value === null || value === undefined || !Number.isFinite(value)) return null;
  const digits = value > 0 && value < 0.1 ? 3 : 2;
  return `$${value.toFixed(digits)}`;
}

export function latencyLabel(ms: number): string {
  return ms <= 0 ? '<1 ms' : `${ms} ms`;
}

export function slugifyProfileId(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[^a-z0-9]+/, '')
    .replace(/[-._]+$/, '')
    .slice(0, 40);
}

/** Stable slug for a new profile: the name when usable, else the endpoint host. */
export function deriveProfileId(name: string, baseUrl: string, taken: string[]): string {
  let base = slugifyProfileId(name);
  if (base === '') {
    try {
      base = slugifyProfileId(new URL(baseUrl).hostname.replace(/\./g, '-'));
    } catch {
      base = '';
    }
  }
  if (base === '') base = 'provider';
  let candidate = base;
  let suffix = 2;
  while (taken.includes(candidate)) {
    candidate = `${base}-${suffix}`;
    suffix += 1;
  }
  return candidate.slice(0, 64);
}

export function baseUrlError(value: string): string | null {
  const trimmed = value.trim();
  if (trimmed === '') return 'Base URL is required.';
  if (!/^https?:\/\//i.test(trimmed)) return 'Use an http:// or https:// URL.';
  return null;
}

export function providerIdError(value: string, taken: string[]): string | null {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(value)) {
    return 'Use letters, digits, dot, underscore or dash, starting with a letter or digit.';
  }
  if (value.length > 64) return 'Keep the id to 64 characters or fewer.';
  if (taken.includes(value)) return 'That id is already in use.';
  return null;
}

export function modelMetaParts(entry: ModelEntry): string[] {
  const parts: string[] = [];
  const context = formatTokenCount(entry.context_window);
  if (context) parts.push(`ctx ${context}`);
  const output = formatTokenCount(entry.max_output_tokens);
  if (output) parts.push(`out ${output}`);
  const input = formatCost(entry.cost_input);
  const outCost = formatCost(entry.cost_output);
  if (input || outCost) {
    parts.push(`${input ?? '?'} in / ${outCost ?? '?'} out per M`);
  }
  return parts;
}

export function catalogModelMeta(model: CatalogModel): string {
  return modelMetaParts({
    id: model.id,
    context_window: model.context_window ?? null,
    max_output_tokens: model.max_output_tokens ?? null,
    cost_input: model.cost_input ?? null,
    cost_output: model.cost_output ?? null,
  }).join(' · ');
}

export function triStateOf(value: boolean | null | undefined): TriState {
  return value === null || value === undefined ? '' : value ? 'true' : 'false';
}

export function triStateValue(value: TriState): boolean | null {
  return value === '' ? null : value === 'true';
}

/** Empty string stays null; otherwise an integer within [min, max]. */
export function boundedInt(raw: string, min: number, max: number): number | null | undefined {
  const trimmed = raw.trim();
  if (trimmed === '') return null;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) return undefined;
  return parsed;
}

/** Empty string stays null; otherwise a finite number within [0, max]. */
export function boundedCost(raw: string, max: number): number | null | undefined {
  const trimmed = raw.trim();
  if (trimmed === '') return null;
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed) || parsed < 0 || parsed > max) return undefined;
  return parsed;
}
