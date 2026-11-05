import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { ApiError, toErrorMessage } from '../api/client';
import { listMcpServers, listPlugins, reloadPlugin, rescanPlugins, setPluginEnabled } from '../api/endpoints';
import type {
  ApprovalMode,
  NotificationsPatch,
  CatalogModel,
  CatalogProviderSummary,
  Config,
  ConfigPatch,
  Limits,
  McpServer,
  ModelEntry,
  PluginDiagnostics,
  ProviderPatch,
  ProviderProfile,
  ProviderTestResult,
} from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as THEME from '../theme';
import type {
  ThemeDensity,
  ThemeFont,
  ThemeGrouping,
  ThemeMode,
  ThemeMono,
  ThemeThinking,
  ThemeToolStyle,
} from '../theme';
import { SettingsAdvanced } from './SettingsAdvanced';
import { Dialog } from './Dialog';
import {
  NotificationsDeliverySection,
  NotificationsDeviceSection,
  NotificationsSection,
} from './NotificationsSettings';
import { TabPanel, Tabs, type TabItem } from './Tabs';
import { Toggle } from './Toggle';
import { AlertIcon, CheckIcon, EditIcon, PaletteIcon, PlusIcon, RefreshIcon } from './icons';

/** Density labels, matching the Theme studio so one setting reads the same in both. */
const DENSITY_LABELS: Record<ThemeDensity, string> = {
  compact: 'Compact',
  comfortable: 'Comfortable',
  spacious: 'Spacious',
};

/** Every limit the sheet edits as a number (`auto_continue` is a toggle). */
type NumericLimitKey = Exclude<keyof Limits, 'auto_continue'>;

interface FormState {
  /** Whether a temperature is sent at all; when off, the field is not sent. */
  sendTemperature: boolean;
  temperature: string;
  maxTokens: string;
  topP: string;
  requestTimeout: string;
  parallelToolCalls: boolean;
  /** Install-wide default for image input; a per-model `vision` wins. */
  supportsImages: boolean;
  approvalMode: ApprovalMode;
  allowCommands: string[];
  autoApprovePaths: string[];
  approvalTimeout: string;
  limits: Record<NumericLimitKey, string>;
  autoContinue: boolean;
  subagentConcurrent: string;
  subagentIterations: string;
  subagentContinues: string;
  allowRecursive: boolean;
  subagentMaxDepth: string;
  subagentTimeout: string;
  allowBackground: boolean;
  subagentMaxBackground: string;
  subagentWaitTimeout: string;
}

const LIMIT_FIELDS: { key: NumericLimitKey; label: string }[] = [
  { key: 'max_iterations', label: 'Max iterations per segment' },
  { key: 'max_run_secs', label: 'Max run seconds (whole run)' },
  { key: 'max_continues', label: 'Auto-continues per run' },
  { key: 'max_tool_output_bytes', label: 'Max tool output (bytes)' },
  { key: 'max_command_output_bytes', label: 'Max command output (bytes)' },
  { key: 'command_timeout_secs', label: 'Command timeout (s)' },
  { key: 'max_command_timeout_secs', label: 'Max command timeout (s)' },
  { key: 'max_file_read_bytes', label: 'Max file read (bytes)' },
  { key: 'search_max_results', label: 'Search results' },
  { key: 'context_max_chars', label: 'Context budget (chars, 0 = from the model)' },
  { key: 'max_tool_calls_per_turn', label: 'Tool calls per turn' },
];

const APPROVAL_MODES: { value: ApprovalMode; label: string; description: string }[] = [
  {
    value: 'ask',
    label: 'Ask (default)',
    description: 'Ask for shell commands and mutating tools. Paths listed below may be pre-approved.',
  },
  {
    value: 'ask_commands',
    label: 'Ask for commands only',
    description: 'File writes inside the workspace are allowed; shell commands still ask.',
  },
  {
    value: 'auto',
    label: 'Auto (no approval prompts)',
    description: 'Everything is allowed and recorded in the event log. Only use this in disposable environments.',
  },
  {
    value: 'read_only',
    label: 'Read only',
    description: 'Only read-only tools are allowed. Shell commands and mutations are denied.',
  },
];

function formFromConfig(config: Config): FormState {
  return {
    sendTemperature: config.model.send_temperature,
    temperature: String(config.model.temperature),
    maxTokens: config.model.max_tokens === null ? '' : String(config.model.max_tokens),
    topP: config.model.top_p === null ? '' : String(config.model.top_p),
    requestTimeout: String(config.model.request_timeout_secs),
    parallelToolCalls: config.model.parallel_tool_calls,
    supportsImages: config.model.supports_images,
    approvalMode: config.approvals.mode,
    allowCommands: [...config.approvals.allow_commands],
    autoApprovePaths: [...config.approvals.auto_approve_paths],
    approvalTimeout: String(config.approvals.timeout_secs),
    limits: Object.fromEntries(
      LIMIT_FIELDS.map((field) => [field.key, String(config.limits[field.key])]),
    ) as Record<NumericLimitKey, string>,
    autoContinue: config.limits.auto_continue,
    subagentConcurrent: String(config.subagents.max_concurrent),
    subagentIterations: String(config.subagents.max_iterations),
    subagentContinues: String(config.subagents.max_continues),
    allowRecursive: config.subagents.allow_recursive,
    subagentMaxDepth: String(config.subagents.max_depth),
    subagentTimeout: String(config.subagents.timeout_secs),
    allowBackground: config.subagents.allow_background,
    subagentMaxBackground: String(config.subagents.max_background),
    subagentWaitTimeout: String(config.subagents.wait_timeout_secs),
  };
}

function numberValue(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === '') return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

function integerAtLeast(value: string, min: number, max: number): boolean {
  const parsed = numberValue(value);
  return parsed !== null && Number.isInteger(parsed) && parsed >= min && parsed <= max;
}

/**
 * Bounds for the `[subagents]` numeric fields, mirroring `SUBAGENT_BOUNDS` in
 * `crates/kc-server/src/settings.rs`.
 *
 * These two lists had drifted apart: this form accepted up to 32 concurrent
 * agents while the server rejected anything over 16, so the local validation
 * passed and the save came back as a 400 the user could not act on. Keep them in
 * step, and keep `0` meaning what the server's doc comments say it means -
 * "no ceiling" for the two counts, "no delegation" for depth.
 */
const SUBAGENT_BOUNDS = {
  concurrent: { min: 0, max: 1024 },
  maxDepth: { min: 0, max: 255 },
  maxBackground: { min: 0, max: 1024 },
} as const;

function validate(form: FormState): Record<string, string> {
  const errors: Record<string, string> = {};

  if (form.sendTemperature) {
    const temperature = numberValue(form.temperature);
    if (temperature === null || temperature < 0 || temperature > 2) {
      errors.temperature = 'Temperature must be between 0 and 2.';
    }
  }
  if (form.maxTokens.trim() !== '') {
    const tokens = numberValue(form.maxTokens);
    if (tokens === null || !Number.isInteger(tokens) || tokens < 1) {
      errors.maxTokens = 'Max tokens must be a positive integer, or empty.';
    }
  }
  if (form.topP.trim() !== '') {
    const topP = numberValue(form.topP);
    if (topP === null || topP < 0 || topP > 1) errors.topP = 'Top-p must be between 0 and 1.';
  }
  if (!integerAtLeast(form.requestTimeout, 1, 3600)) {
    errors.requestTimeout = 'Request timeout must be 1–3600 seconds.';
  }

  for (const field of LIMIT_FIELDS) {
    // `context_max_chars: 0` is meaningful: derive the budget from the model's
    // own context window. Every other limit needs a positive value.
    const min = field.key === 'context_max_chars' ? 0 : 1;
    if (!integerAtLeast(form.limits[field.key], min, 100_000_000)) {
      errors[`limits.${field.key}`] =
        min === 0
          ? `${field.label} must be 0 or a positive integer.`
          : `${field.label} must be a positive integer.`;
    }
  }
  if (
    integerAtLeast(form.limits.command_timeout_secs, 1, 100_000_000) &&
    integerAtLeast(form.limits.max_command_timeout_secs, 1, 100_000_000) &&
    Number(form.limits.max_command_timeout_secs) < Number(form.limits.command_timeout_secs)
  ) {
    errors['limits.max_command_timeout_secs'] = 'Max command timeout must be at least the default timeout.';
  }
  if (!integerAtLeast(form.approvalTimeout, 1, 86_400)) {
    errors.approvalTimeout = 'Approval timeout must be 1–86400 seconds.';
  }
  if (
    !integerAtLeast(
      form.subagentConcurrent,
      SUBAGENT_BOUNDS.concurrent.min,
      SUBAGENT_BOUNDS.concurrent.max,
    )
  ) {
    errors.subagentConcurrent = 'Concurrency must be 0 (no ceiling) or 1–1024.';
  }
  if (!integerAtLeast(form.subagentIterations, 1, 10_000)) {
    errors.subagentIterations = 'Iterations must be 1–10000.';
  }
  if (!integerAtLeast(form.subagentContinues, 0, 1_000)) {
    errors.subagentContinues = 'Auto-continues must be 0–1000.';
  }
  if (
    !integerAtLeast(
      form.subagentMaxDepth,
      SUBAGENT_BOUNDS.maxDepth.min,
      SUBAGENT_BOUNDS.maxDepth.max,
    )
  ) {
    errors.subagentMaxDepth = 'Max depth must be 0–255 (0 = no delegation).';
  }
  if (!integerAtLeast(form.subagentTimeout, 1, 604_800)) {
    errors.subagentTimeout = 'Timeout must be 1–604800 seconds.';
  }
  if (
    !integerAtLeast(
      form.subagentMaxBackground,
      SUBAGENT_BOUNDS.maxBackground.min,
      SUBAGENT_BOUNDS.maxBackground.max,
    )
  ) {
    errors.subagentMaxBackground = 'Live background agents must be 0 (no ceiling) or 1–1024.';
  }
  if (!integerAtLeast(form.subagentWaitTimeout, 1, 3_600)) {
    errors.subagentWaitTimeout = 'Wait timeout must be 1–3600 seconds.';
  }

  return errors;
}

function buildPatch(form: FormState): ConfigPatch {
  const limits: Partial<Limits> = {};
  for (const field of LIMIT_FIELDS) {
    limits[field.key] = Number(form.limits[field.key]);
  }
  limits.auto_continue = form.autoContinue;
  return {
    // Connection details live on provider profiles now; only the sampling
    // tunables (which are install-wide) stay in the flat model section.
    model: {
      // Off by default: an invented sampling value can push a reasoning model
      // into repeating itself instead of acting. Only send one when asked.
      send_temperature: form.sendTemperature,
      ...(form.sendTemperature ? { temperature: Number(form.temperature) } : {}),
      max_tokens: form.maxTokens.trim() === '' ? null : Number(form.maxTokens),
      top_p: form.topP.trim() === '' ? null : Number(form.topP),
      request_timeout_secs: Number(form.requestTimeout),
      parallel_tool_calls: form.parallelToolCalls,
      supports_images: form.supportsImages,
    },
    approvals: {
      mode: form.approvalMode,
      allow_commands: form.allowCommands,
      auto_approve_paths: form.autoApprovePaths,
      timeout_secs: Number(form.approvalTimeout),
    },
    limits,
    subagents: {
      max_concurrent: Number(form.subagentConcurrent),
      max_iterations: Number(form.subagentIterations),
      max_continues: Number(form.subagentContinues),
      allow_recursive: form.allowRecursive,
      max_depth: Number(form.subagentMaxDepth),
      timeout_secs: Number(form.subagentTimeout),
      allow_background: form.allowBackground,
      max_background: Number(form.subagentMaxBackground),
      wait_timeout_secs: Number(form.subagentWaitTimeout),
    },
  };
}

function Field({
  id,
  label,
  hint,
  error,
  wide = false,
  children,
}: {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  wide?: boolean;
  children: ComponentChildren;
}): JSX.Element {
  return (
    <div class={`field${wide ? ' field-wide' : ''}`}>
      <label class="field-label" for={id}>
        {label}
      </label>
      {children}
      {hint ? (
        <p class="field-hint" id={`${id}-hint`}>
          {hint}
        </p>
      ) : null}
      {error ? (
        <p class="field-error" id={`${id}-error`} role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function ListEditor({
  label,
  hint,
  items,
  placeholder,
  disabled,
  onChange,
}: {
  label: string;
  hint: string;
  items: string[];
  placeholder: string;
  disabled: boolean;
  onChange: (items: string[]) => void;
}): JSX.Element {
  const [draft, setDraft] = useState('');
  const add = (): void => {
    const value = draft.trim();
    if (value === '' || items.includes(value)) {
      setDraft('');
      return;
    }
    onChange([...items, value]);
    setDraft('');
  };
  return (
    <div class="list-editor">
      <span class="field-label">{label}</span>
      {items.length > 0 ? (
        <ul class="chip-list" aria-label={label}>
          {items.map((item) => (
            <li class="chip chip-removable" key={item}>
              <code>{item}</code>
              <button
                type="button"
                class="chip-remove"
                aria-label={`Remove ${item}`}
                disabled={disabled}
                onClick={() => onChange(items.filter((candidate) => candidate !== item))}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p class="field-hint">None.</p>
      )}
      <div class="list-editor-add">
        <input
          type="text"
          class="input"
          value={draft}
          placeholder={placeholder}
          disabled={disabled}
          aria-label={`Add ${label.toLowerCase()}`}
          onInput={(event) => setDraft(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              add();
            }
          }}
        />
        <button type="button" class="button button-small" disabled={disabled || draft.trim() === ''} onClick={add}>
          Add
        </button>
      </div>
      <p class="field-hint">{hint}</p>
    </div>
  );
}

function statusTone(status: string): string {
  if (status === 'ready' || status === 'connected') return 'ok';
  if (status === 'failed') return 'danger';
  if (status === 'starting' || status === 'disabling' || status === 'reconnecting') return 'warn';
  return 'muted';
}

// ---------------------------------------------------------------------------
// Provider profiles
// ---------------------------------------------------------------------------

const DEFAULT_PROVIDER_ID = 'openai_compatible';
/** Mirrors the server's bound on a curated model list (kc-server validate_profiles). */
const MAX_MODELS_PER_PROFILE = 64;

interface TestState {
  running: boolean;
  result: ProviderTestResult | null;
  error: string | null;
}

interface AddProviderDraft {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  apiKey: string;
}

function emptyDraft(): AddProviderDraft {
  return { id: '', name: '', provider: '', baseUrl: '', apiKey: '' };
}

function formatTokenCount(value: number | null | undefined): string | null {
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

function formatCost(value: number | null | undefined): string | null {
  if (value === null || value === undefined || !Number.isFinite(value)) return null;
  const digits = value > 0 && value < 0.1 ? 3 : 2;
  return `$${value.toFixed(digits)}`;
}

function latencyLabel(ms: number): string {
  return ms <= 0 ? '<1 ms' : `${ms} ms`;
}

function slugifyProfileId(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[^a-z0-9]+/, '')
    .replace(/[-._]+$/, '')
    .slice(0, 40);
}

/** Stable slug for a new profile: the name when usable, else the endpoint host. */
function deriveProfileId(name: string, baseUrl: string, taken: string[]): string {
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

function baseUrlError(value: string): string | null {
  const trimmed = value.trim();
  if (trimmed === '') return 'Base URL is required.';
  if (!/^https?:\/\//i.test(trimmed)) return 'Use an http:// or https:// URL.';
  return null;
}

function providerIdError(value: string, taken: string[]): string | null {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(value)) {
    return 'Use letters, digits, dot, underscore or dash, starting with a letter or digit.';
  }
  if (value.length > 64) return 'Keep the id to 64 characters or fewer.';
  if (taken.includes(value)) return 'That id is already in use.';
  return null;
}

function modelMetaParts(entry: ModelEntry): string[] {
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

function catalogModelMeta(model: CatalogModel): string {
  return modelMetaParts({
    id: model.id,
    context_window: model.context_window ?? null,
    max_output_tokens: model.max_output_tokens ?? null,
    cost_input: model.cost_input ?? null,
    cost_output: model.cost_output ?? null,
  }).join(' · ');
}

interface CatalogPickerProps {
  profile: ProviderProfile;
  existingIds: string[];
  onAdd: (entries: ModelEntry[]) => Promise<boolean>;
  onClose: () => void;
}

/* ------------------------------------------------------------------ *
 * Per-model metadata editing. Bounds mirror the server's validators
 * (kc-server settings.rs) so mistakes are caught before the PATCH.
 * ------------------------------------------------------------------ */

const MODEL_TOKEN_LIMIT = 100_000_000;
const MODEL_COST_LIMIT = 1_000_000;
const MODEL_NAME_LIMIT = 200;

type TriState = '' | 'true' | 'false';

function triStateOf(value: boolean | null | undefined): TriState {
  return value === null || value === undefined ? '' : value ? 'true' : 'false';
}

function triStateValue(value: TriState): boolean | null {
  return value === '' ? null : value === 'true';
}

/** Empty string stays null; otherwise an integer within [min, max]. */
function boundedInt(raw: string, min: number, max: number): number | null | undefined {
  const trimmed = raw.trim();
  if (trimmed === '') return null;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) return undefined;
  return parsed;
}

/** Empty string stays null; otherwise a finite number within [0, max]. */
function boundedCost(raw: string, max: number): number | null | undefined {
  const trimmed = raw.trim();
  if (trimmed === '') return null;
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed) || parsed < 0 || parsed > max) return undefined;
  return parsed;
}

interface ModelEditDialogProps {
  profile: ProviderProfile;
  model: ModelEntry;
  onSave: (entry: ModelEntry) => Promise<boolean>;
  onClose: () => void;
}

/** Edit one model entry: the label plus every prefilled metadata field. */
function ModelEditDialog({ profile, model, onSave, onClose }: ModelEditDialogProps): JSX.Element {
  const [name, setName] = useState(model.name ?? '');
  const [contextWindow, setContextWindow] = useState(model.context_window?.toString() ?? '');
  const [maxOutput, setMaxOutput] = useState(model.max_output_tokens?.toString() ?? '');
  const [costInput, setCostInput] = useState(model.cost_input?.toString() ?? '');
  const [costOutput, setCostOutput] = useState(model.cost_output?.toString() ?? '');
  const [toolCall, setToolCall] = useState<TriState>(triStateOf(model.tool_call));
  const [reasoning, setReasoning] = useState<TriState>(triStateOf(model.reasoning));
  const [vision, setVision] = useState<TriState>(triStateOf(model.vision));
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);

  const submit = async (): Promise<void> => {
    const context = boundedInt(contextWindow, 1, MODEL_TOKEN_LIMIT);
    const output = boundedInt(maxOutput, 1, MODEL_TOKEN_LIMIT);
    const inputCost = boundedCost(costInput, MODEL_COST_LIMIT);
    const outputCost = boundedCost(costOutput, MODEL_COST_LIMIT);
    const next: Record<string, string> = {};
    if (name.trim().length > MODEL_NAME_LIMIT) {
      next.name = `Keep the name to ${MODEL_NAME_LIMIT} characters or fewer.`;
    }
    if (context === undefined) {
      next.context = `Whole tokens between 1 and ${MODEL_TOKEN_LIMIT.toLocaleString()}, or empty.`;
    }
    if (output === undefined) {
      next.output = `Whole tokens between 1 and ${MODEL_TOKEN_LIMIT.toLocaleString()}, or empty.`;
    }
    if (inputCost === undefined) next.costInput = 'A non-negative number (USD per million), or empty.';
    if (outputCost === undefined) next.costOutput = 'A non-negative number (USD per million), or empty.';
    setErrors(next);
    if (Object.keys(next).length > 0) return;
    setSaving(true);
    const ok = await onSave({
      id: model.id,
      name: name.trim() === '' ? null : name.trim(),
      context_window: context ?? null,
      max_output_tokens: output ?? null,
      tool_call: triStateValue(toolCall),
      reasoning: triStateValue(reasoning),
      vision: triStateValue(vision),
      cost_input: inputCost ?? null,
      cost_output: outputCost ?? null,
      catalog_provider: model.catalog_provider ?? null,
    });
    setSaving(false);
    if (ok) onClose();
  };

  return (
    <Dialog open title={`Edit ${model.id}`} onClose={onClose} class="dialog-model-edit">
      <p class="field-hint">
        Custom values for this model on {profile.name}. Refreshing the metadata later updates the
        catalog fields but leaves this label untouched.
      </p>
      <div class="field-grid">
        <Field id="model-edit-name" label="Display name" error={errors.name} wide>
          <input
            id="model-edit-name"
            class="input"
            type="text"
            value={name}
            placeholder={model.id}
            data-autofocus
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-context" label="Context window" error={errors.context}>
          <input
            id="model-edit-context"
            class="input mono"
            type="text"
            inputmode="numeric"
            value={contextWindow}
            placeholder="128000"
            onInput={(event) => setContextWindow(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-output" label="Max output tokens" error={errors.output}>
          <input
            id="model-edit-output"
            class="input mono"
            type="text"
            inputmode="numeric"
            value={maxOutput}
            placeholder="16384"
            onInput={(event) => setMaxOutput(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-cost-in" label="Input cost / M tokens" error={errors.costInput}>
          <input
            id="model-edit-cost-in"
            class="input mono"
            type="text"
            inputmode="decimal"
            value={costInput}
            placeholder="0.15"
            onInput={(event) => setCostInput(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-cost-out" label="Output cost / M tokens" error={errors.costOutput}>
          <input
            id="model-edit-cost-out"
            class="input mono"
            type="text"
            inputmode="decimal"
            value={costOutput}
            placeholder="0.60"
            onInput={(event) => setCostOutput(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-tools" label="Tool calling">
          <select
            id="model-edit-tools"
            class="input"
            value={toolCall}
            onChange={(event) => setToolCall(event.currentTarget.value as TriState)}
          >
            <option value="">Unknown</option>
            <option value="true">Supported</option>
            <option value="false">Not supported</option>
          </select>
        </Field>
        <Field id="model-edit-reasoning" label="Reasoning">
          <select
            id="model-edit-reasoning"
            class="input"
            value={reasoning}
            onChange={(event) => setReasoning(event.currentTarget.value as TriState)}
          >
            <option value="">Unknown</option>
            <option value="true">Supported</option>
            <option value="false">Not supported</option>
          </select>
        </Field>
        <Field
          id="model-edit-vision"
          label="Image input (vision)"
          hint={
            vision === ''
              ? 'Not declared: the install-wide “Send images to models” default applies.'
              : vision === 'true'
                ? 'read_file attaches a PNG/JPEG/GIF/WebP for this model to look at.'
                : 'read_file refuses images and says the model cannot see them.'
          }
        >
          <select
            id="model-edit-vision"
            class="input"
            value={vision}
            onChange={(event) => setVision(event.currentTarget.value as TriState)}
            aria-describedby="model-edit-vision-hint"
          >
            <option value="">Unknown (use the default)</option>
            <option value="true">Accepted</option>
            <option value="false">Text only</option>
          </select>
        </Field>
      </div>
      <footer class="dialog-actions">
        <button type="button" class="button button-small" onClick={onClose} disabled={saving}>
          Cancel
        </button>
        <button
          type="button"
          class="button button-small button-primary"
          disabled={saving}
          onClick={() => void submit()}
        >
          {saving ? 'Saving…' : 'Save model'}
        </button>
      </footer>
    </Dialog>
  );
}

/** Two-step models.dev browser: provider list, then a searchable model checklist. */
function CatalogPickerDialog({ profile, existingIds, onAdd, onClose }: CatalogPickerProps): JSX.Element {
  const [stage, setStage] = useState<'providers' | 'models'>('providers');
  const [providers, setProviders] = useState<CatalogProviderSummary[]>([]);
  const [provider, setProvider] = useState<CatalogProviderSummary | null>(null);
  const [models, setModels] = useState<CatalogModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  const loadProviders = async (refresh: boolean): Promise<void> => {
    setLoading(true);
    setError(null);
    setNotice(null);
    const response = await actions.loadCatalog(undefined, refresh);
    setLoading(false);
    if (!response) {
      setError('Could not reach the models.dev catalog. You can still add model ids by hand.');
      return;
    }
    setProviders(response.providers);
    if (!response.ok) {
      setError(response.error ?? 'The models.dev catalog is unavailable. You can still add model ids by hand.');
    } else {
      setNotice(response.error ? `Showing a cached catalog: ${response.error}` : response.stale ? 'Showing a cached catalog.' : null);
    }
  };

  useEffect(() => {
    void loadProviders(false);
    // The picker is mounted fresh per profile; load once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const chooseProvider = async (summary: CatalogProviderSummary): Promise<void> => {
    setStage('models');
    setProvider(summary);
    setModels([]);
    setSelected([]);
    setQuery('');
    setLoading(true);
    setError(null);
    setNotice(null);
    const response = await actions.loadCatalog(summary.id, false);
    setLoading(false);
    if (!response) {
      setError('Could not load models for this provider. You can still add model ids by hand.');
      return;
    }
    setModels(response.models);
    setProvider(response.provider ?? summary);
    if (!response.ok) {
      setError(response.error ?? 'The models.dev catalog is unavailable for this provider.');
    } else if (response.error) {
      setNotice(`Showing a cached catalog: ${response.error}`);
    }
  };

  const toggle = (id: string): void => {
    setSelected((current) =>
      current.includes(id) ? current.filter((candidate) => candidate !== id) : [...current, id],
    );
  };

  const submit = async (): Promise<void> => {
    const chosen = models.filter((model) => selected.includes(model.id) && !existingIds.includes(model.id));
    if (chosen.length === 0) return;
    const entries: ModelEntry[] = chosen.map((model) => ({
      id: model.id,
      name: model.name.trim() === '' ? null : model.name.trim(),
      context_window: model.context_window ?? null,
      max_output_tokens: model.max_output_tokens ?? null,
      tool_call: model.tool_call ?? null,
      reasoning: model.reasoning ?? null,
      // models.dev is the source of truth for "does this model take images", so
      // the picker prefills the per-model flag instead of making the user find it.
      vision: model.vision ?? null,
      cost_input: model.cost_input ?? null,
      cost_output: model.cost_output ?? null,
      catalog_provider: provider?.id ?? null,
    }));
    setSaving(true);
    const ok = await onAdd(entries);
    setSaving(false);
    if (ok) onClose();
  };

  const needle = query.trim().toLowerCase();
  const visibleProviders = providers.filter(
    (candidate) =>
      needle === '' ||
      candidate.name.toLowerCase().includes(needle) ||
      candidate.id.toLowerCase().includes(needle),
  );
  const visibleModels = models.filter(
    (candidate) =>
      needle === '' ||
      candidate.name.toLowerCase().includes(needle) ||
      candidate.id.toLowerCase().includes(needle),
  );
  const addable = models.filter((model) => selected.includes(model.id) && !existingIds.includes(model.id)).length;

  return (
    <Dialog
      open
      title={`models.dev: ${profile.name}`}
      onClose={onClose}
      class="dialog-catalog"
      labelId="catalog-title"
    >
      <div class="catalog-body">
        {stage === 'providers' ? (
          <div class="catalog-stage">
            <div class="catalog-toolbar">
              <span class="eyebrow">Providers</span>
              <button
                type="button"
                class="button button-small"
                disabled={loading}
                onClick={() => void loadProviders(true)}
              >
                <RefreshIcon size={12} class={loading ? 'spin' : undefined} /> Refresh catalog
              </button>
            </div>
            <Field
              id="catalog-provider-search"
              label="Search providers"
              hint="Pick a models.dev provider to browse its models."
            >
              <input
                id="catalog-provider-search"
                class="input"
                type="search"
                value={query}
                data-autofocus
                placeholder="anthropic"
                aria-describedby="catalog-provider-search-hint"
                onInput={(event) => setQuery(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') event.preventDefault();
                }}
              />
            </Field>
            {loading ? (
              <div class="catalog-skeletons" aria-busy="true" aria-label="Loading catalog providers">
                {[0, 1, 2].map((index) => (
                  <div class="skeleton catalog-skeleton" key={index} />
                ))}
              </div>
            ) : null}
            {error ? (
              <div class="inline-error" role="alert">
                <p>{error}</p>
              </div>
            ) : null}
            {notice ? <p class="catalog-notice">{notice}</p> : null}
            {!loading && visibleProviders.length === 0 && !error ? (
              <p class="field-hint">No catalog provider matches “{query}”.</p>
            ) : null}
            <ul class="catalog-provider-list">
              {visibleProviders.map((candidate) => (
                <li key={candidate.id}>
                  <button type="button" class="catalog-provider" onClick={() => void chooseProvider(candidate)}>
                    <span class="catalog-provider-name">{candidate.name}</span>
                    <code class="catalog-provider-id">{candidate.id}</code>
                    <span class="catalog-count">
                      {candidate.model_count} model{candidate.model_count === 1 ? '' : 's'}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <div class="catalog-stage">
            <div class="catalog-models-head">
              <button
                type="button"
                class="button button-small"
                onClick={() => {
                  setStage('providers');
                  setQuery('');
                  setSelected([]);
                }}
              >
                ← Providers
              </button>
              <span class="catalog-models-provider">
                {provider?.name ?? provider?.id ?? ''}
                {provider ? ` · ${provider.model_count} models` : ''}
              </span>
            </div>
            <Field
              id="catalog-model-search"
              label="Search models"
              hint="Tick one or more models, then add them to the profile."
            >
              <input
                id="catalog-model-search"
                class="input"
                type="search"
                value={query}
                data-autofocus
                placeholder="qwen3"
                aria-describedby="catalog-model-search-hint"
                onInput={(event) => setQuery(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') event.preventDefault();
                }}
              />
            </Field>
            {loading ? (
              <div class="catalog-skeletons" aria-busy="true" aria-label="Loading catalog models">
                {[0, 1, 2, 3, 4].map((index) => (
                  <div class="skeleton catalog-skeleton" key={index} />
                ))}
              </div>
            ) : null}
            {error ? (
              <div class="inline-error" role="alert">
                <p>{error}</p>
              </div>
            ) : null}
            {notice ? <p class="catalog-notice">{notice}</p> : null}
            {!loading && visibleModels.length === 0 && !error ? (
              <p class="field-hint">No catalog model matches “{query}”.</p>
            ) : null}
            <ul class="catalog-model-list">
              {visibleModels.map((model) => {
                const added = existingIds.includes(model.id);
                const meta = catalogModelMeta(model);
                return (
                  <li class="catalog-model" key={model.id}>
                    <label class="catalog-model-label">
                      <input
                        type="checkbox"
                        checked={added || selected.includes(model.id)}
                        disabled={added}
                        onChange={() => toggle(model.id)}
                      />
                      <span class="catalog-model-text">
                        <span class="catalog-model-name">{model.name}</span>
                        <code class="catalog-model-id">{model.id}</code>
                        {meta ? <span class="catalog-model-meta">{meta}</span> : null}
                      </span>
                      {added ? <span class="chip chip-muted">added</span> : null}
                      {!added && model.tool_call ? <span class="chip">tools</span> : null}
                      {!added && model.reasoning ? <span class="chip">reasoning</span> : null}
                      {!added && model.vision ? (
                        <span class="chip chip-ok" title="Takes image input; added with vision enabled">
                          vision
                        </span>
                      ) : null}
                    </label>
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        <footer class="catalog-footer">
          <span class="field-hint">
            Manual ids always work: if the catalog is down, add a model id to the profile directly.
          </span>
          <div class="catalog-footer-actions">
            <button type="button" class="button button-small" onClick={onClose}>
              Cancel
            </button>
            {stage === 'models' ? (
              <button
                type="button"
                class="button button-small button-primary"
                disabled={saving || addable === 0}
                onClick={() => void submit()}
              >
                {saving ? 'Adding…' : `Add selected${addable > 0 ? ` (${addable})` : ''}`}
              </button>
            ) : null}
          </div>
        </footer>
      </div>
    </Dialog>
  );
}

function ProvidersSection({ disabled }: { disabled: boolean }): JSX.Element | null {
  const config = store.config.value;
  const [pending, setPending] = useState<Record<string, boolean>>({});
  const [tests, setTests] = useState<Record<string, TestState>>({});
  const [actionError, setActionError] = useState<string | null>(null);
  const [keyEditor, setKeyEditor] = useState<string | null>(null);
  const [keyDraft, setKeyDraft] = useState('');
  const [confirmClearKey, setConfirmClearKey] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [modelDrafts, setModelDrafts] = useState<Record<string, string>>({});
  const [modelErrors, setModelErrors] = useState<Record<string, string>>({});
  const [catalogFor, setCatalogFor] = useState<ProviderProfile | null>(null);
  const [editingModel, setEditingModel] = useState<{ profile: ProviderProfile; model: ModelEntry } | null>(
    null,
  );
  const [showAdd, setShowAdd] = useState(false);
  const [draft, setDraft] = useState<AddProviderDraft>(emptyDraft);
  const [draftErrors, setDraftErrors] = useState<Record<string, string>>({});

  if (!config) return null;

  const profiles = config.providers;
  const active = config.active;
  const supportedProviders = config.available_providers?.length
    ? config.available_providers
    : profiles.length > 0
      ? [profiles[0]?.provider ?? DEFAULT_PROVIDER_ID]
      : [DEFAULT_PROVIDER_ID];
  const defaultProvider = supportedProviders[0] ?? DEFAULT_PROVIDER_ID;
  const takenIds = profiles.map((profile) => profile.id);
  const derivedId = deriveProfileId(draft.name, draft.baseUrl, takenIds);

  const markPending = (key: string, value: boolean): void => {
    setPending((current) => ({ ...current, [key]: value }));
  };

  const savePatch = async (
    patch: Pick<ConfigPatch, 'providers' | 'active'>,
    toast: string | null,
  ): Promise<boolean> => {
    setActionError(null);
    const result = await actions.saveProviders(patch, { toast });
    if (!result) {
      setActionError('The server rejected that change. Check the connection and try again.');
      return false;
    }
    return true;
  };

  const runTest = async (profile: ProviderProfile): Promise<void> => {
    markPending(`${profile.id}:test`, true);
    setTests((current) => ({ ...current, [profile.id]: { running: true, result: null, error: null } }));
    try {
      const result = await actions.testProvider({ profile: profile.id });
      setTests((current) => ({
        ...current,
        [profile.id]: { running: false, result, error: null },
      }));
    } catch (cause) {
      setTests((current) => ({
        ...current,
        [profile.id]: { running: false, result: null, error: toErrorMessage(cause) },
      }));
    } finally {
      markPending(`${profile.id}:test`, false);
    }
  };

  const setActive = async (profile: ProviderProfile): Promise<void> => {
    const currentModel = active.provider === profile.id ? active.model : null;
    const model = currentModel ?? profile.models[0]?.id ?? '';
    markPending(`${profile.id}:active`, true);
    const ok = await savePatch(
      { active: { provider: profile.id, model } },
      `Now using ${profile.name}.`,
    );
    markPending(`${profile.id}:active`, false);
    if (!ok) setActionError(`Could not switch to ${profile.name}.`);
  };

  const saveKey = async (profile: ProviderProfile): Promise<void> => {
    const key = keyDraft.trim();
    if (key === '') return;
    markPending(`${profile.id}:key`, true);
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, api_key: key }] } },
      `API key saved for ${profile.name}.`,
    );
    markPending(`${profile.id}:key`, false);
    if (ok) {
      setKeyEditor(null);
      setKeyDraft('');
    }
  };

  const clearKey = async (profile: ProviderProfile): Promise<void> => {
    markPending(`${profile.id}:key`, true);
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, clear_api_key: true }] } },
      `API key cleared for ${profile.name}.`,
    );
    markPending(`${profile.id}:key`, false);
    if (ok) setConfirmClearKey(null);
  };

  const removeModel = async (profile: ProviderProfile, modelId: string): Promise<void> => {
    const models = profile.models.filter((model) => model.id !== modelId);
    const patch: Pick<ConfigPatch, 'providers' | 'active'> = {
      providers: { upsert: [{ id: profile.id, models }] },
    };
    // The server refuses an active model that is missing from its profile, so
    // move the selection in the same request instead of failing the removal.
    if (active.provider === profile.id && active.model === modelId) {
      patch.active = { provider: profile.id, model: models[0]?.id ?? '' };
    }
    markPending(`${profile.id}:models`, true);
    await savePatch(patch, `Removed ${modelId} from ${profile.name}.`);
    markPending(`${profile.id}:models`, false);
  };

  const addModelById = async (profile: ProviderProfile): Promise<void> => {
    const modelId = (modelDrafts[profile.id] ?? '').trim();
    if (modelId === '') return;
    if (profile.models.some((model) => model.id === modelId)) {
      setModelErrors((current) => ({ ...current, [profile.id]: `${modelId} is already in this profile.` }));
      return;
    }
    if (profile.models.length >= MAX_MODELS_PER_PROFILE) {
      setModelErrors((current) => ({
        ...current,
        [profile.id]: `A profile can hold at most ${MAX_MODELS_PER_PROFILE} models.`,
      }));
      return;
    }
    setModelErrors((current) => ({ ...current, [profile.id]: '' }));
    markPending(`${profile.id}:models`, true);
    // Best-effort metadata detection: the server strips gateway routing
    // prefixes (a6api/model, a6api/upstream/model) until a models.dev entry
    // matches. Any failure just adds the bare id, as before.
    const lookup = await actions.lookupModel(modelId);
    const detected = lookup?.ok ? lookup.match : null;
    const entry: ModelEntry = detected
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
    const entries: ModelEntry[] = [...profile.models, entry];
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, models: entries }] } },
      detected
        ? `Added ${modelId} to ${profile.name} (metadata via ${detected.provider}).`
        : `Added ${modelId} to ${profile.name}.`,
    );
    markPending(`${profile.id}:models`, false);
    if (ok) setModelDrafts((current) => ({ ...current, [profile.id]: '' }));
  };

  const refreshModel = async (profile: ProviderProfile, model: ModelEntry): Promise<void> => {
    const key = `${profile.id}:refresh:${model.id}`;
    markPending(key, true);
    try {
      const lookup = await actions.lookupModel(model.id);
      const detected = lookup?.ok ? lookup.match : null;
      if (!detected) {
        store.pushToast(
          lookup && !lookup.ok
            ? `Could not refresh ${model.id}: ${lookup.error ?? 'the catalog is unavailable'}`
            : `No models.dev entry matches ${model.id}; its metadata is unchanged.`,
          'info',
        );
        return;
      }
      // Refresh metadata only: a custom display name stays the user's.
      const models = profile.models.map((entry) =>
        entry.id === model.id
          ? {
              ...entry,
              context_window: detected.model.context_window ?? null,
              max_output_tokens: detected.model.max_output_tokens ?? null,
              tool_call: detected.model.tool_call ?? null,
              reasoning: detected.model.reasoning ?? null,
              vision: detected.model.vision ?? null,
              cost_input: detected.model.cost_input ?? null,
              cost_output: detected.model.cost_output ?? null,
              catalog_provider: detected.provider,
            }
          : entry,
      );
      await savePatch(
        { providers: { upsert: [{ id: profile.id, models }] } },
        `Refreshed metadata for ${model.id} (via ${detected.provider}).`,
      );
    } finally {
      markPending(key, false);
    }
  };

  const saveEditedModel = async (entry: ModelEntry): Promise<boolean> => {
    const editing = editingModel;
    if (!editing) return false;
    const models = editing.profile.models.map((model) => (model.id === entry.id ? entry : model));
    return savePatch(
      { providers: { upsert: [{ id: editing.profile.id, models }] } },
      `Saved ${entry.name?.trim() || entry.id}.`,
    );
  };

  const useModel = async (profile: ProviderProfile, modelId: string): Promise<void> => {
    markPending(`${profile.id}:use`, true);
    const ok = await actions.switchModel(profile.id, modelId);
    markPending(`${profile.id}:use`, false);
    if (!ok) setActionError(`Could not switch to ${modelId}.`);
  };

  const addCatalogModels = async (profile: ProviderProfile, entries: ModelEntry[]): Promise<boolean> => {
    const merged = [...profile.models];
    for (const entry of entries) {
      if (!merged.some((model) => model.id === entry.id)) merged.push(entry);
    }
    if (merged.length === profile.models.length) return true;
    const capped = merged.slice(0, MAX_MODELS_PER_PROFILE);
    const added = capped.length - profile.models.length;
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, models: capped }] } },
      `Added ${added} model${added === 1 ? '' : 's'} to ${profile.name}.`,
    );
    return ok;
  };

  const removeProfile = async (profile: ProviderProfile): Promise<void> => {
    const remaining = profiles.filter((candidate) => candidate.id !== profile.id);
    const next = remaining[0];
    const payload: Pick<ConfigPatch, 'providers' | 'active'> = {
      providers: { delete: [profile.id] },
    };
    if (active.provider === profile.id && next) {
      payload.active = { provider: next.id, model: next.models[0]?.id ?? '' };
    }
    markPending(`${profile.id}:remove`, true);
    const ok = await savePatch(payload, `Removed ${profile.name}.`);
    markPending(`${profile.id}:remove`, false);
    if (ok) setConfirmRemove(null);
  };

  const addProvider = async (): Promise<void> => {
    const errors: Record<string, string> = {};
    const urlError = baseUrlError(draft.baseUrl);
    if (urlError) errors.baseUrl = urlError;
    const id = draft.id.trim() === '' ? derivedId : draft.id.trim();
    const idError = providerIdError(id, takenIds);
    if (idError) errors.id = idError;
    setDraftErrors(errors);
    if (Object.keys(errors).length > 0) return;

    const patch: ProviderPatch = {
      id,
      base_url: draft.baseUrl.trim(),
      provider: draft.provider || defaultProvider,
    };
    const name = draft.name.trim();
    if (name !== '') patch.name = name;
    const apiKey = draft.apiKey.trim();
    if (apiKey !== '') patch.api_key = apiKey;

    const payload: Pick<ConfigPatch, 'providers' | 'active'> = { providers: { upsert: [patch] } };
    // The first profile becomes the active one; the server requires an explicit
    // selection whenever profiles exist.
    if (profiles.length === 0) payload.active = { provider: id, model: '' };

    markPending('add', true);
    const ok = await savePatch(payload, `Added ${name === '' ? id : name}.`);
    markPending('add', false);
    if (ok) {
      setDraft(emptyDraft());
      setDraftErrors({});
      setShowAdd(false);
    }
  };

  return (
    <section class="settings-section" aria-labelledby="settings-providers">
      <div class="settings-section-head">
        <h3 id="settings-providers">Providers</h3>
        <div class="settings-section-actions">
          <span class="settings-section-note">OpenAI-compatible endpoints and their model libraries</span>
          {profiles.length > 0 ? (
            <button
              type="button"
              class="button button-small"
              aria-expanded={showAdd}
              aria-controls="provider-add-form"
              onClick={() => {
                setDraftErrors({});
                setShowAdd((current) => !current);
              }}
            >
              <PlusIcon size={12} /> Add provider
            </button>
          ) : null}
        </div>
      </div>

      {actionError ? (
        <div class="inline-error" role="alert">
          <p>{actionError}</p>
        </div>
      ) : null}

      {profiles.length === 0 ? (
        <p class="field-hint">
          No providers configured yet. Add one below, then pick its models — manually or from models.dev.
        </p>
      ) : null}

      <div class="providers-list">
        {profiles.map((profile) => {
          const isActive = active.provider === profile.id;
          const selectedModelId = isActive ? active.model ?? profile.models[0]?.id ?? null : null;
          const test = tests[profile.id];
          const headingId = `provider-${profile.id}-name`;
          const testPending = pending[`${profile.id}:test`] === true;
          const keyPending = pending[`${profile.id}:key`] === true;
          const modelsPending = pending[`${profile.id}:models`] === true;
          const usePending = pending[`${profile.id}:use`] === true;
          const activePending = pending[`${profile.id}:active`] === true;
          const removePending = pending[`${profile.id}:remove`] === true;
          const modelError = modelErrors[profile.id];

          return (
            <article class="provider-card" key={profile.id} aria-labelledby={headingId}>
              <header class="provider-card-head">
                <div class="provider-card-title">
                  <h4 class="provider-name" id={headingId}>
                    {profile.name}
                  </h4>
                  <code class="provider-id">{profile.id}</code>
                  {isActive ? <span class="badge badge-ok provider-badge">active</span> : null}
                  <span class={`chip ${profile.api_key_set ? 'chip-ok' : 'chip-muted'}`}>
                    {profile.api_key_set ? (
                      <>
                        <CheckIcon size={11} /> key set
                      </>
                    ) : (
                      'no key'
                    )}
                  </span>
                </div>
                <div class="provider-meta">
                  <span class="chip chip-muted">{profile.provider}</span>
                  <code class="provider-url" title={profile.base_url}>
                    {profile.base_url}
                  </code>
                  <span class="provider-count">
                    {profile.models.length} model{profile.models.length === 1 ? '' : 's'}
                  </span>
                </div>
              </header>

              <div class="provider-actions">
                <button
                  type="button"
                  class="button button-small"
                  disabled={disabled || testPending}
                  aria-label={`Test connection for ${profile.name}`}
                  onClick={() => void runTest(profile)}
                >
                  <RefreshIcon size={12} class={testPending ? 'spin' : undefined} />
                  {testPending ? 'Testing…' : 'Test'}
                </button>
                <button
                  type="button"
                  class="button button-small"
                  disabled={disabled || isActive || activePending}
                  aria-label={`Set active provider: ${profile.name}`}
                  onClick={() => void setActive(profile)}
                >
                  {isActive ? 'Active' : 'Set active'}
                </button>
                <button
                  type="button"
                  class="button button-small"
                  disabled={disabled}
                  aria-expanded={keyEditor === profile.id}
                  aria-label={
                    profile.api_key_set ? `Replace key for ${profile.name}` : `Add key for ${profile.name}`
                  }
                  onClick={() => {
                    setConfirmClearKey(null);
                    setConfirmRemove(null);
                    setKeyDraft('');
                    setKeyEditor((current) => (current === profile.id ? null : profile.id));
                  }}
                >
                  {profile.api_key_set ? 'Replace key' : 'Add key'}
                </button>
                {profile.api_key_set ? (
                  confirmClearKey === profile.id ? (
                    <>
                      <button
                        type="button"
                        class="button button-small button-danger"
                        disabled={disabled || keyPending}
                        aria-label={`Confirm clear key for ${profile.name}`}
                        onClick={() => void clearKey(profile)}
                      >
                        Confirm clear key
                      </button>
                      <button
                        type="button"
                        class="button button-small"
                        aria-label={`Cancel clearing the key for ${profile.name}`}
                        onClick={() => setConfirmClearKey(null)}
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      class="button button-small button-danger-ghost"
                      disabled={disabled || keyPending}
                      aria-label={`Clear key for ${profile.name}`}
                      onClick={() => {
                        setKeyEditor(null);
                        setConfirmRemove(null);
                        setConfirmClearKey(profile.id);
                      }}
                    >
                      Clear key
                    </button>
                  )
                ) : null}
                <button
                  type="button"
                  class="button button-small"
                  disabled={disabled}
                  aria-label={`Browse models.dev for ${profile.name}`}
                  onClick={() => setCatalogFor(profile)}
                >
                  Browse models.dev
                </button>
                {confirmRemove === profile.id ? (
                  <>
                    <button
                      type="button"
                      class="button button-small button-danger"
                      disabled={disabled || removePending}
                      aria-label={`Confirm remove profile ${profile.name}`}
                      onClick={() => void removeProfile(profile)}
                    >
                      Confirm remove profile
                    </button>
                    <button
                      type="button"
                      class="button button-small"
                      aria-label={`Cancel removing ${profile.name}`}
                      onClick={() => setConfirmRemove(null)}
                    >
                      Cancel
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    class="button button-small button-danger-ghost"
                    disabled={disabled}
                    aria-label={`Remove profile ${profile.name}`}
                    onClick={() => {
                      setConfirmClearKey(null);
                      setKeyEditor(null);
                      setConfirmRemove(profile.id);
                    }}
                  >
                    Remove profile
                  </button>
                )}
              </div>

              {test && (test.running || test.result || test.error) ? (
                <div
                  class={`test-result${test.result ? (test.result.ok ? ' is-ok' : ' is-error') : ''}`}
                  role="status"
                >
                  {test.running ? <span>Testing {profile.name}…</span> : null}
                  {test.result ? (
                    <>
                      <strong>{test.result.ok ? 'Connection succeeded' : 'Connection failed'}</strong>
                      <span>{latencyLabel(test.result.latency_ms)}</span>
                      {test.result.models && test.result.models.length > 0 ? (
                        <span class="test-models">
                          {test.result.models.length} model{test.result.models.length === 1 ? '' : 's'}:{' '}
                          {test.result.models.slice(0, 8).join(', ')}
                          {test.result.models.length > 8 ? '…' : ''}
                        </span>
                      ) : null}
                      {test.result.error ? <span class="test-error">{test.result.error}</span> : null}
                    </>
                  ) : null}
                  {test.error ? <span class="test-error">{test.error}</span> : null}
                </div>
              ) : null}

              {keyEditor === profile.id ? (
                <div class="provider-key-editor">
                  <label class="field-label" for={`provider-key-${profile.id}`}>
                    {profile.api_key_set ? 'New API key' : 'API key'} for {profile.name}
                  </label>
                  <div class="provider-key-row">
                    <input
                      id={`provider-key-${profile.id}`}
                      class="input mono"
                      type="password"
                      autoComplete="off"
                      placeholder={profile.api_key_set ? 'Enter a new key to replace the stored one' : 'sk-…'}
                      value={keyDraft}
                      disabled={disabled || keyPending}
                      onInput={(event) => setKeyDraft(event.currentTarget.value)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          event.preventDefault();
                          void saveKey(profile);
                        }
                      }}
                    />
                    <button
                      type="button"
                      class="button button-small button-primary"
                      disabled={disabled || keyPending || keyDraft.trim() === ''}
                      aria-label={`Save key for ${profile.name}`}
                      onClick={() => void saveKey(profile)}
                    >
                      {keyPending ? 'Saving…' : 'Save key'}
                    </button>
                    <button
                      type="button"
                      class="button button-small"
                      aria-label={`Cancel key change for ${profile.name}`}
                      onClick={() => {
                        setKeyEditor(null);
                        setKeyDraft('');
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                  <p class="field-hint">
                    Write-only: the server never returns the stored key. Saving replaces the current one.
                  </p>
                </div>
              ) : null}

              <div class="model-library">
                <div class="model-library-head">
                  <span class="eyebrow">Model library</span>
                  {profile.models.length === 0 ? (
                    <span class="field-hint">No models yet — add one below or browse models.dev.</span>
                  ) : null}
                </div>
                {profile.models.length > 0 ? (
                  <ul class="model-list" aria-label={`Models on ${profile.name}`}>
                    {profile.models.map((model) => {
                      const using = selectedModelId === model.id;
                      const label = model.name?.trim() ? model.name : model.id;
                      const meta = modelMetaParts(model);
                      const refreshPending = pending[`${profile.id}:refresh:${model.id}`] === true;
                      return (
                        <li class="model-row" key={model.id}>
                          <div class="model-row-main">
                            <span class="model-name">{label}</span>
                            {label !== model.id ? <code class="model-id">{model.id}</code> : null}
                            {using ? <span class="badge badge-ok">in use</span> : null}
                            {model.catalog_provider ? (
                              <span class="chip chip-muted">via {model.catalog_provider}</span>
                            ) : null}
                          </div>
                          <div class="model-meta">
                            {meta.length > 0 ? <span>{meta.join(' · ')}</span> : <span>metadata unknown</span>}
                            {model.tool_call ? <span class="chip">tools</span> : null}
                            {model.reasoning ? <span class="chip">reasoning</span> : null}
                            {model.vision === true ? (
                              <span class="chip chip-ok" title="read_file attaches an image for this model">
                                vision
                              </span>
                            ) : model.vision === false ? (
                              <span class="chip chip-muted" title="Text only: read_file refuses images">
                                no images
                              </span>
                            ) : (
                              <span
                                class="chip chip-muted"
                                title="Not declared: the install-wide image default applies"
                              >
                                images: default
                              </span>
                            )}
                          </div>
                          <div class="model-actions">
                            <button
                              type="button"
                              class="button button-small"
                              disabled={disabled || usePending || using}
                              aria-label={
                                using ? `In use: ${label} on ${profile.name}` : `Use ${label} on ${profile.name}`
                              }
                              onClick={() => void useModel(profile, model.id)}
                            >
                              {using ? 'In use' : 'Use'}
                            </button>
                            <button
                              type="button"
                              class="button button-small"
                              disabled={disabled || modelsPending}
                              aria-label={`Edit ${label} on ${profile.name}`}
                              onClick={() => setEditingModel({ profile, model })}
                            >
                              <EditIcon size={12} />
                              Edit
                            </button>
                            <button
                              type="button"
                              class="button button-small"
                              disabled={disabled || modelsPending || refreshPending}
                              aria-label={`Refresh metadata for ${label} on ${profile.name}`}
                              title="Re-read this model's metadata from models.dev (routing prefixes are stripped to find it)."
                              onClick={() => void refreshModel(profile, model)}
                            >
                              <RefreshIcon size={12} class={refreshPending ? 'spin' : undefined} />
                              {refreshPending ? 'Refreshing…' : 'Refresh'}
                            </button>
                            <button
                              type="button"
                              class="button button-small button-danger-ghost"
                              disabled={disabled || modelsPending}
                              aria-label={`Remove ${label} from ${profile.name}`}
                              onClick={() => void removeModel(profile, model.id)}
                            >
                              Remove
                            </button>
                          </div>
                        </li>
                      );
                    })}
                  </ul>
                ) : null}
                <div class="model-add">
                  <label class="field-label" for={`model-add-${profile.id}`}>
                    Add a model id
                  </label>
                  <div class="model-add-row">
                    <input
                      id={`model-add-${profile.id}`}
                      class="input mono"
                      type="text"
                      placeholder="qwen3-32b"
                      value={modelDrafts[profile.id] ?? ''}
                      disabled={disabled || modelsPending}
                      onInput={(event) => {
                        const value = event.currentTarget.value;
                        setModelDrafts((current) => ({ ...current, [profile.id]: value }));
                      }}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          event.preventDefault();
                          void addModelById(profile);
                        }
                      }}
                    />
                    <button
                      type="button"
                      class="button button-small"
                      disabled={disabled || modelsPending || (modelDrafts[profile.id] ?? '').trim() === ''}
                      aria-label={`Add model id to ${profile.name}`}
                      onClick={() => void addModelById(profile)}
                    >
                      Add
                    </button>
                  </div>
                  {modelError ? (
                    <p class="field-error" role="alert">
                      {modelError}
                    </p>
                  ) : null}
                </div>
              </div>
            </article>
          );
        })}
      </div>

      {showAdd || profiles.length === 0 ? (
        <div class="provider-add" id="provider-add-form">
          <div class="settings-section-head">
            <h4 class="provider-add-title">New provider</h4>
            <span class="settings-section-note">The key is write-only and never shown again</span>
          </div>
          <div class="field-grid">
            <Field id="new-provider-name" label="Name (optional)" hint="Shown in this list; defaults to the id.">
              <input
                id="new-provider-name"
                class="input"
                type="text"
                value={draft.name}
                disabled={disabled || pending['add'] === true}
                aria-describedby="new-provider-name-hint"
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setDraft((current) => ({ ...current, name: value }));
                }}
              />
            </Field>
            <Field
              id="new-provider-id"
              label="Profile id"
              error={draftErrors.id}
              hint={`Used by the API. Leave blank for “${derivedId}”.`}
            >
              <input
                id="new-provider-id"
                class="input mono"
                type="text"
                value={draft.id}
                placeholder={derivedId}
                disabled={disabled || pending['add'] === true}
                aria-invalid={draftErrors.id ? 'true' : undefined}
                aria-describedby={draftErrors.id ? 'new-provider-id-error' : 'new-provider-id-hint'}
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setDraft((current) => ({ ...current, id: value }));
                }}
              />
            </Field>
            <Field id="new-provider-provider" label="API shape" hint="The server implementation that speaks this endpoint.">
              <select
                id="new-provider-provider"
                class="input mono"
                value={draft.provider || defaultProvider}
                disabled={disabled || pending['add'] === true}
                aria-describedby="new-provider-provider-hint"
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  setDraft((current) => ({ ...current, provider: value }));
                }}
              >
                {supportedProviders.map((id) => (
                  <option key={id} value={id}>
                    {id}
                  </option>
                ))}
              </select>
            </Field>
            <Field
              id="new-provider-base-url"
              label="Base URL"
              error={draftErrors.baseUrl}
              hint="Includes the API version, e.g. https://api.example.com/v1."
            >
              <input
                id="new-provider-base-url"
                class="input mono"
                type="url"
                value={draft.baseUrl}
                placeholder="https://api.example.com/v1"
                disabled={disabled || pending['add'] === true}
                aria-invalid={draftErrors.baseUrl ? 'true' : undefined}
                aria-describedby={
                  draftErrors.baseUrl ? 'new-provider-base-url-error' : 'new-provider-base-url-hint'
                }
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setDraft((current) => ({ ...current, baseUrl: value }));
                }}
              />
            </Field>
            <Field
              id="new-provider-api-key"
              label="API key"
              wide
              hint="Write-only: stored on the server and never sent back to the browser."
            >
              <input
                id="new-provider-api-key"
                class="input mono"
                type="password"
                autoComplete="off"
                placeholder="sk-…"
                value={draft.apiKey}
                disabled={disabled || pending['add'] === true}
                aria-describedby="new-provider-api-key-hint"
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setDraft((current) => ({ ...current, apiKey: value }));
                }}
              />
            </Field>
          </div>
          <div class="provider-add-actions">
            {profiles.length > 0 ? (
              <button type="button" class="button button-small" onClick={() => setShowAdd(false)}>
                Cancel
              </button>
            ) : null}
            <button
              type="button"
              class="button button-small button-primary"
              disabled={disabled || pending['add'] === true}
              onClick={() => void addProvider()}
            >
              {pending['add'] === true ? 'Adding…' : 'Add provider'}
            </button>
          </div>
        </div>
      ) : null}

      {catalogFor ? (
        <CatalogPickerDialog
          profile={catalogFor}
          existingIds={catalogFor.models.map((model) => model.id)}
          onAdd={(entries) => addCatalogModels(catalogFor, entries)}
          onClose={() => setCatalogFor(null)}
        />
      ) : null}

      {editingModel ? (
        <ModelEditDialog
          profile={editingModel.profile}
          model={editingModel.model}
          onSave={saveEditedModel}
          onClose={() => setEditingModel(null)}
        />
      ) : null}
    </section>
  );
}
export type SettingsTab = 'general' | 'models' | 'agent' | 'notifications' | 'plugins';

const SETTINGS_TABS: readonly TabItem<SettingsTab>[] = [
  { id: 'general', label: 'General' },
  { id: 'models', label: 'Models' },
  { id: 'agent', label: 'Agent' },
  { id: 'notifications', label: 'Notifications' },
  { id: 'plugins', label: 'Plugins & MCP' },
];

/** The collapsible "Advanced" block of each tab that has one. */
interface AdvancedState {
  general: boolean;
  models: boolean;
  agent: boolean;
  notifications: boolean;
}

const ADVANCED_CLOSED: AdvancedState = {
  general: false,
  models: false,
  agent: false,
  notifications: false,
};

/**
 * Where each validated field lives: its tab, whether it sits behind that tab's
 * Advanced disclosure, and the DOM id to focus.
 *
 * The Save button is in the footer and validates the whole form, but only one tab
 * is on screen. Without this map a rejected value would be reported against a
 * control the user cannot see, so `revealErrors` uses it to open the right tab,
 * expand the disclosure and focus the field.
 */
interface FieldLocation {
  tab: SettingsTab;
  advanced?: keyof AdvancedState;
  anchor: string;
}

const FIELD_LOCATIONS: Record<string, FieldLocation> = {
  temperature: { tab: 'models', advanced: 'models', anchor: 'temperature' },
  maxTokens: { tab: 'models', advanced: 'models', anchor: 'max-tokens' },
  topP: { tab: 'models', advanced: 'models', anchor: 'top-p' },
  requestTimeout: { tab: 'models', advanced: 'models', anchor: 'request-timeout' },
  approvalTimeout: { tab: 'agent', anchor: 'approval-timeout' },
  // Approvals are open on the Agent tab; the subagent budget and the raw limits
  // are both inside its Advanced fold.
  subagentConcurrent: { tab: 'agent', advanced: 'agent', anchor: 'subagent-concurrent' },
  subagentIterations: { tab: 'agent', advanced: 'agent', anchor: 'subagent-iterations' },
  subagentContinues: { tab: 'agent', advanced: 'agent', anchor: 'subagent-continues' },
  subagentMaxDepth: { tab: 'agent', advanced: 'agent', anchor: 'subagent-max-depth' },
  subagentTimeout: { tab: 'agent', advanced: 'agent', anchor: 'subagent-timeout' },
  subagentMaxBackground: { tab: 'agent', advanced: 'agent', anchor: 'subagent-max-background' },
  subagentWaitTimeout: { tab: 'agent', advanced: 'agent', anchor: 'subagent-wait-timeout' },
  ...Object.fromEntries(
    LIMIT_FIELDS.map((field) => [
      `limits.${field.key}`,
      { tab: 'agent', advanced: 'agent', anchor: `limit-${field.key}` } satisfies FieldLocation,
    ]),
  ),
};

/** The tab a validated field belongs to, for the footer's error summary. */
function tabOf(errorKey: string): SettingsTab {
  return FIELD_LOCATIONS[errorKey]?.tab ?? 'general';
}

/** The human label of a tab, for the footer's error summary. */
function tabLabel(tab: SettingsTab): string {
  return SETTINGS_TABS.find((item) => item.id === tab)?.label ?? tab;
}

export function SettingsSheet(): JSX.Element | null {
  const open = store.settingsOpen.value;
  const [form, setForm] = useState<FormState | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);
  const [tab, setTab] = useState<SettingsTab>('general');
  const [advanced, setAdvanced] = useState<AdvancedState>(ADVANCED_CLOSED);
  const [plugins, setPlugins] = useState<PluginDiagnostics[] | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServer[] | null>(null);
  const [diagnosticsError, setDiagnosticsError] = useState<string | null>(null);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [configReloadKey, setConfigReloadKey] = useState(0);
  const [diagnosticsKey, setDiagnosticsKey] = useState(0);
  const [pluginBusy, setPluginBusy] = useState<string | null>(null);
  const [pluginActionError, setPluginActionError] = useState<string | null>(null);

  /**
   * Run one plugin lifecycle action and re-read the diagnostics afterwards.
   *
   * Reloading is a POST that replaces processes, so the list on screen is stale
   * the moment it returns; bumping `diagnosticsKey` is what makes the row show
   * the new status and pid instead of the old one.
   */
  const runPluginAction = (
    name: string,
    action: 'reload' | 'enable' | 'disable' | 'rescan',
  ): void => {
    setPluginBusy(`${action}:${name}`);
    setPluginActionError(null);
    const request =
      action === 'rescan'
        ? rescanPlugins()
        : action === 'reload'
          ? reloadPlugin(name)
          : setPluginEnabled(name, action === 'enable');
    request
      .then(() => setDiagnosticsKey((key) => key + 1))
      .catch((cause: unknown) => setPluginActionError(toErrorMessage(cause)))
      .finally(() => setPluginBusy(null));
  };

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setErrors({});
    setSaveError(null);
    // Render from the config already in the store immediately; the fetch below
    // refreshes it (and reports server-side problems) without a blank sheet.
    const cached = store.config.value;
    if (cached) {
      setForm(formFromConfig(cached));
      setLocked(cached.settings_locked);
      setLoading(false);
    } else {
      setLoading(true);
    }
    setLoadError(null);
    actions
      .loadConfig()
      .then((result) => {
        if (cancelled) return;
        setForm(formFromConfig(result));
        setLocked(result.settings_locked);
        setLoading(false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setLoadError(toErrorMessage(cause));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, configReloadKey]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setDiagnosticsLoading(true);
    setDiagnosticsError(null);
    Promise.all([listPlugins(), listMcpServers()])
      .then(([pluginList, serverList]) => {
        if (cancelled) return;
        setPlugins(pluginList);
        setMcpServers(serverList);
        setDiagnosticsLoading(false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setDiagnosticsError(toErrorMessage(cause));
        setDiagnosticsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, diagnosticsKey]);

  if (!open) return null;

  const disabled = locked || loading || form === null;

  const update = (patch: Partial<FormState>): void => {
    setForm((current) => (current ? { ...current, ...patch } : current));
    setErrors((current) => (Object.keys(current).length > 0 ? {} : current));
  };

  const onAdvancedChange = (key: keyof AdvancedState) => (value: boolean): void => {
    setAdvanced((current) => ({ ...current, [key]: value }));
  };

  /**
   * Take the user to the first field the form rejected.
   *
   * Validation covers every tab, so a failure is usually off-screen. Switching
   * tab, expanding the disclosure that holds it and focusing the input is the
   * difference between "fix this" and "find this".
   */
  const revealErrors = (validation: Record<string, string>): void => {
    const first = Object.keys(validation)[0];
    if (!first) return;
    const location = FIELD_LOCATIONS[first];
    if (!location) return;
    setTab(location.tab);
    if (location.advanced) {
      const advancedKey = location.advanced;
      setAdvanced((current) => ({ ...current, [advancedKey]: true }));
    }
    // After the render that switches the tab and opens the disclosure, so the
    // element is visible (a closed <details> hides its own subtree).
    window.setTimeout(() => {
      const element = document.getElementById(location.anchor);
      if (!element) return;
      element.scrollIntoView({ block: 'center' });
      element.focus({ preventScroll: true });
    }, 0);
  };

  const submit = async (): Promise<void> => {
    if (!form || disabled) return;
    const validation = validate(form);
    setErrors(validation);
    if (Object.keys(validation).length > 0) {
      revealErrors(validation);
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await actions.saveConfig(buildPatch(form));
      setForm(formFromConfig(updated));
      setLocked(updated.settings_locked);
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 403) {
        setLocked(true);
        setSaveError('Settings are locked by the server (KARUCODE_LOCK_SETTINGS).');
      } else {
        setSaveError(toErrorMessage(cause));
      }
    } finally {
      setSaving(false);
    }
  };

  /**
   * Save one notification switch and re-seed the form from the answer.
   *
   * Notification switches PATCH on their own (they have no Save button of their
   * own), so this reports failure the same way `submit` does: a locked install
   * flips the sheet into its read-only state rather than showing a generic error.
   */
  const saveNotifications = async (patch: NotificationsPatch): Promise<boolean> => {
    try {
      const updated = await actions.saveConfig({ notifications: patch });
      setForm(formFromConfig(updated));
      setLocked(updated.settings_locked);
      return true;
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 403) {
        setLocked(true);
        setSaveError('Settings are locked by the server (KARUCODE_LOCK_SETTINGS).');
      } else {
        setSaveError(toErrorMessage(cause));
      }
      return false;
    }
  };

  const approvalDescription =
    APPROVAL_MODES.find((mode) => mode.value === form?.approvalMode)?.description ?? '';
  const errorCount = Object.keys(errors).length;
  const errorTabs = [...new Set(Object.keys(errors).map(tabOf))];

  return (
    <Dialog
      open
      title="Settings"
      description="Personalize your coding experience."
      onClose={actions.closeSettings}
      class="dialog-sheet dialog-settings"
      labelId="settings-title"
    >
      <div class="settings-body">
        {loading ? (
          <div class="settings-loading" aria-busy="true" aria-label="Loading settings">
            {[0, 1, 2, 3, 4, 5].map((index) => (
              <div class="skeleton skeleton-setting-row" key={index} />
            ))}
          </div>
        ) : null}

        {loadError ? (
          <div class="inline-error" role="alert">
            <p>{loadError}</p>
            <button type="button" class="button button-small" onClick={() => setConfigReloadKey((key) => key + 1)}>
              Retry
            </button>
          </div>
        ) : null}

        {locked ? (
          <div class="banner banner-warn" role="status">
            <AlertIcon size={15} />
            <div class="banner-body">
              <strong>Settings are locked.</strong> The server pins configuration (KARUCODE_LOCK_SETTINGS=1).
              Changes made here will be rejected.
            </div>
          </div>
        ) : null}

        {saveError ? (
          <div class="run-error-banner" role="alert">
            <div class="run-error-head">
              <span class="run-error-kicker">
                <AlertIcon size={14} /> Could not save settings
              </span>
            </div>
            <p class="run-error-message">{saveError}</p>
          </div>
        ) : null}

        {form ? (
          <form
            class="settings-form"
            // The sheet validates every field itself and renders the message
            // inline (with aria-invalid + aria-describedby). Native constraint
            // validation would instead block the submit and show a browser
            // tooltip the app cannot style, translate or announce.
            noValidate
            onSubmit={(event) => {
              event.preventDefault();
              void submit();
            }}
          >
            <Tabs
              items={SETTINGS_TABS}
              active={tab}
              onChange={setTab}
              label="Settings sections"
              idPrefix="settings"
              variant="settings"
            />

            {/*
             * Panels stay mounted and merely hide. A tab switch must not throw
             * away what was typed, and the Models tab owns local state (key
             * drafts, the add-provider form) that would otherwise vanish the
             * moment the user looked at another tab.
             */}
            <div class="settings-panels">
              <TabPanel idPrefix="settings" id="general" active={tab === 'general'} persist>
                <section class="settings-section" aria-labelledby="settings-appearance">
                  <div class="settings-section-head">
                    <h3 id="settings-appearance">Appearance</h3>
                    <span class="settings-section-note">Stored in this browser, not in the server config</span>
                  </div>
                  <div class="setting-row">
                    <div class="setting-copy">
                      <span class="setting-name">Theme studio</span>
                      <span class="setting-hint">
                        Pick a built-in theme or tune every surface, accent, font and density.
                      </span>
                    </div>
                    <button
                      type="button"
                      class="panel-action-btn"
                      onClick={actions.openThemeStudio}
                      aria-haspopup="dialog"
                    >
                      <PaletteIcon size={14} /> Theme studio
                    </button>
                  </div>
                  <div class="field-grid">
                    <Field
                      id="settings-theme"
                      label="Theme"
                      hint="Applies immediately. Saving this sheet is not required."
                    >
                      <select
                        id="settings-theme"
                        class="input"
                        aria-describedby="settings-theme-hint"
                        value={THEME.activeThemeId.value}
                        onChange={(event) => THEME.selectTheme(event.currentTarget.value)}
                      >
                        {THEME.themeLibrary.value.map((entry) => (
                          <option value={entry.id} key={entry.id}>
                            {entry.name}
                          </option>
                        ))}
                        {THEME.draftEdited.value ? (
                          <option value="draft">{THEME.activeThemeName.value}</option>
                        ) : null}
                      </select>
                    </Field>
                    <Field id="settings-density" label="Density" hint="Moves type size and spacing together.">
                      <select
                        id="settings-density"
                        class="input"
                        aria-describedby="settings-density-hint"
                        value={THEME.activeTheme.value.density}
                        onChange={(event) =>
                          THEME.setDensity(event.currentTarget.value as ThemeDensity)
                        }
                      >
                        {THEME.THEME_DENSITIES.map((density) => (
                          <option value={density} key={density}>
                            {DENSITY_LABELS[density]}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <Field id="settings-mode" label="Colour mode" hint="Re-bases the surfaces, keeping your accent.">
                      <select
                        id="settings-mode"
                        class="input"
                        aria-describedby="settings-mode-hint"
                        value={THEME.activeTheme.value.mode}
                        onChange={(event) => THEME.setThemeMode(event.currentTarget.value as ThemeMode)}
                      >
                        {THEME.THEME_MODES.map((mode) => (
                          <option value={mode} key={mode}>
                            {mode === 'dark' ? 'Dark' : 'Light'}
                          </option>
                        ))}
                      </select>
                    </Field>
                  </div>
                </section>

                <section class="settings-section" aria-labelledby="settings-transcript">
                  <div class="settings-section-head">
                    <h3 id="settings-transcript">Transcript</h3>
                    <span class="settings-section-note">
                      How the conversation is drawn; the rest of the theme lives in the Studio
                    </span>
                  </div>
                  <div class="field-grid">
                    <Field id="settings-tool-style" label="Tool calls">
                      <select
                        id="settings-tool-style"
                        class="input"
                        value={THEME.activeTheme.value.toolStyle}
                        onChange={(event) =>
                          THEME.setToolStyle(event.currentTarget.value as ThemeToolStyle)
                        }
                      >
                        {THEME.THEME_TOOL_STYLE_CHOICES.map(({ value, label }) => (
                          <option value={value} key={value}>
                            {label}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <Field id="settings-grouping" label="Grouping">
                      <select
                        id="settings-grouping"
                        class="input"
                        value={THEME.activeTheme.value.grouping}
                        onChange={(event) =>
                          THEME.setGrouping(event.currentTarget.value as ThemeGrouping)
                        }
                      >
                        {THEME.THEME_GROUPING_CHOICES.map(({ value, label }) => (
                          <option value={value} key={value}>
                            {label}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <Field id="settings-thinking" label="Reasoning blocks">
                      <select
                        id="settings-thinking"
                        class="input"
                        value={THEME.activeTheme.value.thinking}
                        onChange={(event) =>
                          THEME.setThinking(event.currentTarget.value as ThemeThinking)
                        }
                      >
                        {THEME.THEME_THINKING_CHOICES.map(({ value, label }) => (
                          <option value={value} key={value}>
                            {label}
                          </option>
                        ))}
                      </select>
                    </Field>
                  </div>
                </section>

                <SettingsAdvanced
                  id="settings-general-advanced"
                  open={advanced.general}
                  onOpenChange={onAdvancedChange('general')}
                  note="Fonts, rounding and the transcript layout"
                >
                  <section class="settings-section" aria-labelledby="settings-type">
                    <div class="settings-section-head">
                      <h3 id="settings-type">Type and shape</h3>
                      <span class="settings-section-note">Every change is applied and saved at once</span>
                    </div>
                    <div class="field-grid">
                      <Field id="settings-font" label="Interface font">
                        <select
                          id="settings-font"
                          class="input"
                          value={THEME.activeTheme.value.font}
                          onChange={(event) => THEME.setFont(event.currentTarget.value as ThemeFont)}
                        >
                          {THEME.THEME_FONTS.map((font) => (
                            <option value={font} key={font}>
                              {THEME.THEME_FONT_LABELS[font]}
                            </option>
                          ))}
                        </select>
                      </Field>
                      <Field id="settings-mono" label="Code font">
                        <select
                          id="settings-mono"
                          class="input"
                          value={THEME.activeTheme.value.mono}
                          onChange={(event) => THEME.setMono(event.currentTarget.value as ThemeMono)}
                        >
                          {THEME.THEME_MONO_FONTS.map((mono) => (
                            <option value={mono} key={mono}>
                              {THEME.THEME_MONO_LABELS[mono]}
                            </option>
                          ))}
                        </select>
                      </Field>
                      <Field
                        id="settings-rounding"
                        label="Corner rounding (px)"
                        hint="0 is square, 22 is fully rounded."
                        wide
                      >
                        <input
                          id="settings-rounding"
                          class="input-range"
                          type="range"
                          min="0"
                          max="22"
                          step="1"
                          value={String(THEME.activeTheme.value.rounding)}
                          onInput={(event) => THEME.setRounding(Number(event.currentTarget.value))}
                        />
                      </Field>
                    </div>
                  </section>
                </SettingsAdvanced>
              </TabPanel>

              <TabPanel idPrefix="settings" id="models" active={tab === 'models'} persist>
                <ProvidersSection disabled={disabled} />

                <SettingsAdvanced
                  id="settings-models-advanced"
                  open={advanced.models}
                  onOpenChange={onAdvancedChange('models')}
                  note="Sampling, request timeout and image input"
                >
                  <div class="field-grid">
                    <Field
                      id="temperature"
                      label="Temperature"
                      error={errors.temperature}
                      hint={
                        form.sendTemperature
                          ? '0–2. Lower is more deterministic.'
                          : 'Not sent: the endpoint uses the model’s own default.'
                      }
                    >
                      <input
                        id="temperature"
                        class="input mono"
                        type="number"
                        step="0.05"
                        min="0"
                        max="2"
                        value={form.temperature}
                        disabled={disabled || !form.sendTemperature}
                        onInput={(event) => update({ temperature: event.currentTarget.value })}
                        aria-invalid={errors.temperature ? 'true' : undefined}
                        aria-describedby={errors.temperature ? 'temperature-error' : 'temperature-hint'}
                      />
                    </Field>
                    <Toggle
                      label="Send a temperature"
                      description="Off by default. An invented sampling value can make a reasoning model repeat itself instead of acting; leave it off unless the endpoint asks for one."
                      checked={form.sendTemperature}
                      disabled={disabled}
                      onChange={(checked) => update({ sendTemperature: checked })}
                    />
                    <Field id="max-tokens" label="Max tokens" error={errors.maxTokens} hint="Empty means the provider default.">
                      <input
                        id="max-tokens"
                        class="input mono"
                        type="number"
                        min="1"
                        step="1"
                        value={form.maxTokens}
                        disabled={disabled}
                        onInput={(event) => update({ maxTokens: event.currentTarget.value })}
                        aria-invalid={errors.maxTokens ? 'true' : undefined}
                        aria-describedby={errors.maxTokens ? 'max-tokens-error' : 'max-tokens-hint'}
                      />
                    </Field>
                    <Field id="top-p" label="Top-p" error={errors.topP} hint="Empty means the provider default.">
                      <input
                        id="top-p"
                        class="input mono"
                        type="number"
                        min="0"
                        max="1"
                        step="0.05"
                        value={form.topP}
                        disabled={disabled}
                        onInput={(event) => update({ topP: event.currentTarget.value })}
                        aria-invalid={errors.topP ? 'true' : undefined}
                        aria-describedby={errors.topP ? 'top-p-error' : 'top-p-hint'}
                      />
                    </Field>
                    <Field
                      id="request-timeout"
                      label="Request timeout (s)"
                      error={errors.requestTimeout}
                      hint="Per provider request."
                    >
                      <input
                        id="request-timeout"
                        class="input mono"
                        type="number"
                        min="1"
                        max="3600"
                        step="1"
                        value={form.requestTimeout}
                        disabled={disabled}
                        onInput={(event) => update({ requestTimeout: event.currentTarget.value })}
                        aria-invalid={errors.requestTimeout ? 'true' : undefined}
                        aria-describedby={
                          errors.requestTimeout ? 'request-timeout-error' : 'request-timeout-hint'
                        }
                      />
                    </Field>
                    <div class="field field-toggle">
                      <Toggle
                        label="Parallel tool calls"
                        description="Ask the endpoint to allow multiple tool calls per turn."
                        checked={form.parallelToolCalls}
                        disabled={disabled}
                        onChange={(checked) => update({ parallelToolCalls: checked })}
                      />
                    </div>
                    <div class="field field-toggle">
                      <Toggle
                        label="Send images to models"
                        description="Install-wide default for read_file on an image. A model's own “Image input (vision)” setting overrides it, so a vision model still works while this is off. models.dev fills that in when you add a model."
                        checked={form.supportsImages}
                        disabled={disabled}
                        onChange={(checked) => update({ supportsImages: checked })}
                      />
                    </div>
                  </div>
                </SettingsAdvanced>
              </TabPanel>

              <TabPanel idPrefix="settings" id="agent" active={tab === 'agent'} persist>
                <section class="settings-section" aria-labelledby="settings-approvals">
                  <div class="settings-section-head">
                    <h3 id="settings-approvals">Approvals</h3>
                    <span class="settings-section-note">The only gate for shell commands</span>
                  </div>
                  <Field id="approval-mode" label="Approval mode" hint={approvalDescription} wide>
                    <select
                      id="approval-mode"
                      class="input"
                      value={form.approvalMode}
                      disabled={disabled}
                      onChange={(event) => update({ approvalMode: event.currentTarget.value as ApprovalMode })}
                      aria-describedby="approval-mode-hint"
                    >
                      {APPROVAL_MODES.map((mode) => (
                        <option value={mode.value} key={mode.value}>
                          {mode.label}
                        </option>
                      ))}
                    </select>
                  </Field>
                  {form.approvalMode === 'auto' ? (
                    <div class="inline-warning" role="status">
                      <AlertIcon size={14} /> Auto mode removes the approval gate. Every command will run without
                      asking.
                    </div>
                  ) : null}
                  <div class="field-grid">
                    <div class="field field-wide">
                      <ListEditor
                        label="Always allow commands"
                        hint="Prefix match. For example, `cargo test` also allows `cargo test -p foo`. Keep this list narrow."
                        items={form.allowCommands}
                        placeholder="cargo test"
                        disabled={disabled}
                        onChange={(items) => update({ allowCommands: items })}
                      />
                    </div>
                    <div class="field field-wide">
                      <ListEditor
                        label="Pre-approved write paths"
                        hint="Only consulted in ask mode. Paths are workspace-relative."
                        items={form.autoApprovePaths}
                        placeholder="docs/"
                        disabled={disabled}
                        onChange={(items) => update({ autoApprovePaths: items })}
                      />
                    </div>
                    <Field
                      id="approval-timeout"
                      label="Approval timeout (s)"
                      error={errors.approvalTimeout}
                      hint="A pending approval is denied when this expires."
                    >
                      <input
                        id="approval-timeout"
                        class="input mono"
                        type="number"
                        min="1"
                        max="86400"
                        step="1"
                        value={form.approvalTimeout}
                        disabled={disabled}
                        onInput={(event) => update({ approvalTimeout: event.currentTarget.value })}
                        aria-invalid={errors.approvalTimeout ? 'true' : undefined}
                        aria-describedby={
                          errors.approvalTimeout ? 'approval-timeout-error' : 'approval-timeout-hint'
                        }
                      />
                    </Field>
                  </div>
                </section>

                <SettingsAdvanced
                  id="settings-agent-advanced"
                  open={advanced.agent}
                  onOpenChange={onAdvancedChange('agent')}
                  note="Subagent budget, timeouts and resource limits"
                >
                  <section class="settings-section" aria-labelledby="settings-subagents">
                    <div class="settings-section-head">
                      <h3 id="settings-subagents">Subagents</h3>
                      <span class="settings-section-note">Same workspace, advisory path ownership</span>
                    </div>
                    <div class="field-grid">
                      <Field
                        id="subagent-concurrent"
                        label="Max concurrent"
                        error={errors.subagentConcurrent}
                        hint="Loops running at once, install-wide. 0 = no ceiling: a fan-out starts every child it was given."
                      >
                        <input
                          id="subagent-concurrent"
                          class="input mono"
                          type="number"
                          min={SUBAGENT_BOUNDS.concurrent.min}
                          max={SUBAGENT_BOUNDS.concurrent.max}
                          step="1"
                          value={form.subagentConcurrent}
                          disabled={disabled}
                          onInput={(event) => update({ subagentConcurrent: event.currentTarget.value })}
                          aria-invalid={errors.subagentConcurrent ? 'true' : undefined}
                          aria-describedby={
                            errors.subagentConcurrent
                              ? 'subagent-concurrent-error'
                              : 'subagent-concurrent-hint'
                          }
                        />
                      </Field>
                      <Field
                        id="subagent-iterations"
                        label="Turns per segment"
                        error={errors.subagentIterations}
                        hint="A checkpoint, not a finish line: the subagent refreshes it below."
                      >
                        <input
                          id="subagent-iterations"
                          class="input mono"
                          type="number"
                          min="1"
                          max="10000"
                          step="1"
                          value={form.subagentIterations}
                          disabled={disabled}
                          onInput={(event) => update({ subagentIterations: event.currentTarget.value })}
                          aria-invalid={errors.subagentIterations ? 'true' : undefined}
                          aria-describedby={
                            errors.subagentIterations
                              ? 'subagent-iterations-error'
                              : 'subagent-iterations-hint'
                          }
                        />
                      </Field>
                      <Field
                        id="subagent-continues"
                        label="Auto-continues"
                        error={errors.subagentContinues}
                        hint="How many times one subagent may refresh its budget. 0 = stop at the segment."
                      >
                        <input
                          id="subagent-continues"
                          class="input mono"
                          type="number"
                          min="0"
                          max="1000"
                          step="1"
                          value={form.subagentContinues}
                          disabled={disabled}
                          onInput={(event) => update({ subagentContinues: event.currentTarget.value })}
                          aria-invalid={errors.subagentContinues ? 'true' : undefined}
                          aria-describedby={
                            errors.subagentContinues
                              ? 'subagent-continues-error'
                              : 'subagent-continues-hint'
                          }
                        />
                      </Field>
                      <Field
                        id="subagent-max-depth"
                        label="Max depth"
                        error={errors.subagentMaxDepth}
                        hint="How many generations may nest. 0 = no delegation at all; 255 is the deepest the field holds."
                      >
                        <input
                          id="subagent-max-depth"
                          class="input mono"
                          type="number"
                          min={SUBAGENT_BOUNDS.maxDepth.min}
                          max={SUBAGENT_BOUNDS.maxDepth.max}
                          step="1"
                          value={form.subagentMaxDepth}
                          disabled={disabled}
                          onInput={(event) => update({ subagentMaxDepth: event.currentTarget.value })}
                          aria-invalid={errors.subagentMaxDepth ? 'true' : undefined}
                          aria-describedby={
                            errors.subagentMaxDepth ? 'subagent-max-depth-error' : 'subagent-max-depth-hint'
                          }
                        />
                      </Field>
                      <Field id="subagent-timeout" label="Timeout (s)" error={errors.subagentTimeout}>
                        <input
                          id="subagent-timeout"
                          class="input mono"
                          type="number"
                          min="1"
                          max="604800"
                          step="1"
                          value={form.subagentTimeout}
                          disabled={disabled}
                          onInput={(event) => update({ subagentTimeout: event.currentTarget.value })}
                          aria-invalid={errors.subagentTimeout ? 'true' : undefined}
                          aria-describedby={errors.subagentTimeout ? 'subagent-timeout-error' : undefined}
                        />
                      </Field>
                      <Field
                        id="subagent-max-background"
                        label="Live background agents"
                        hint="How many detached subagents one session may keep running at once. 0 = no ceiling."
                        error={errors.subagentMaxBackground}
                      >
                        <input
                          id="subagent-max-background"
                          class="input mono"
                          type="number"
                          min={SUBAGENT_BOUNDS.maxBackground.min}
                          max={SUBAGENT_BOUNDS.maxBackground.max}
                          step="1"
                          value={form.subagentMaxBackground}
                          disabled={disabled}
                          onInput={(event) =>
                            update({ subagentMaxBackground: event.currentTarget.value })
                          }
                          aria-invalid={errors.subagentMaxBackground ? 'true' : undefined}
                          aria-describedby={
                            errors.subagentMaxBackground
                              ? 'subagent-max-background-error'
                              : 'subagent-max-background-hint'
                          }
                        />
                      </Field>
                      <Field
                        id="subagent-wait-timeout"
                        label="Wait timeout (s)"
                        hint="The default bound on wait_for_agents; a wait that runs out reports who is still working."
                        error={errors.subagentWaitTimeout}
                      >
                        <input
                          id="subagent-wait-timeout"
                          class="input mono"
                          type="number"
                          min="1"
                          max="3600"
                          step="1"
                          value={form.subagentWaitTimeout}
                          disabled={disabled}
                          onInput={(event) =>
                            update({ subagentWaitTimeout: event.currentTarget.value })
                          }
                          aria-invalid={errors.subagentWaitTimeout ? 'true' : undefined}
                          aria-describedby={
                            errors.subagentWaitTimeout
                              ? 'subagent-wait-timeout-error'
                              : 'subagent-wait-timeout-hint'
                          }
                        />
                      </Field>
                      <div class="field field-toggle field-wide">
                        <Toggle
                          label="Allow background subagents"
                          description="A spawn can return immediately and keep working; the parent collects it with wait_for_agents."
                          checked={form.allowBackground}
                          disabled={disabled}
                          onChange={(checked) => update({ allowBackground: checked })}
                        />
                      </div>
                      <div class="field field-toggle field-wide">
                        <Toggle
                          label="Allow recursive subagents"
                          description="Subagents may spawn their own subagents, bounded by depth."
                          checked={form.allowRecursive}
                          disabled={disabled}
                          onChange={(checked) => update({ allowRecursive: checked })}
                        />
                      </div>
                    </div>
                  </section>

                  <section class="settings-section" aria-labelledby="settings-limits">
                    <div class="settings-section-head">
                      <h3 id="settings-limits">Limits</h3>
                      <span class="settings-section-note">Resource and context bounds</span>
                    </div>
                    <div class="field-grid field-grid-compact">
                      <div class="field field-toggle field-wide">
                        <Toggle
                          label="Auto-continue long runs"
                          description="When a segment's iteration budget runs out, refresh it and keep working in the same session instead of ending the run."
                          checked={form.autoContinue}
                          disabled={disabled}
                          onChange={(checked) => update({ autoContinue: checked })}
                        />
                      </div>
                      {LIMIT_FIELDS.map((field) => (
                        <Field
                          key={field.key}
                          id={`limit-${field.key}`}
                          label={field.label}
                          error={errors[`limits.${field.key}`]}
                        >
                          <input
                            id={`limit-${field.key}`}
                            class="input mono"
                            type="number"
                            min="1"
                            step="1"
                            value={form.limits[field.key]}
                            disabled={disabled}
                            onInput={(event) =>
                              update({
                                limits: { ...form.limits, [field.key]: event.currentTarget.value },
                              })
                            }
                            aria-invalid={errors[`limits.${field.key}`] ? 'true' : undefined}
                            aria-describedby={
                              errors[`limits.${field.key}`] ? `limit-${field.key}-error` : undefined
                            }
                          />
                        </Field>
                      ))}
                    </div>
                  </section>
                </SettingsAdvanced>
              </TabPanel>

              <TabPanel
                idPrefix="settings"
                id="notifications"
                active={tab === 'notifications'}
                persist
              >
                <NotificationsSection
                  config={store.config.value}
                  disabled={disabled}
                  onSave={saveNotifications}
                />

                <NotificationsDeviceSection />

                <SettingsAdvanced
                  id="settings-notifications-advanced"
                  open={advanced.notifications}
                  onOpenChange={onAdvancedChange('notifications')}
                  note="Delivery: minimum run length and body text"
                >
                  <NotificationsDeliverySection
                    config={store.config.value}
                    disabled={disabled}
                    onSave={saveNotifications}
                  />
                </SettingsAdvanced>
              </TabPanel>

              <TabPanel idPrefix="settings" id="plugins" active={tab === 'plugins'} persist>
                <section class="settings-section" aria-labelledby="settings-plugins">
                  <div class="settings-section-head">
                    <h3 id="settings-plugins">Plugins &amp; MCP</h3>
                    <div class="settings-section-actions">
                      <button
                        type="button"
                        class="button button-small"
                        onClick={() => runPluginAction('all', 'rescan')}
                        disabled={pluginBusy !== null}
                        title="Re-read every plugin source: start new plugins, drop removed ones, replace changed ones"
                      >
                        {pluginBusy === 'rescan:all' ? 'Rescanning…' : 'Rescan plugins'}
                      </button>
                      <button
                        type="button"
                        class="button button-small"
                        onClick={() => setDiagnosticsKey((key) => key + 1)}
                        disabled={diagnosticsLoading}
                      >
                        <RefreshIcon size={13} class={diagnosticsLoading ? 'spin' : undefined} /> Refresh status
                      </button>
                    </div>
                  </div>
                  <p class="settings-section-note">
                    Plugins and MCP servers run as separate processes with the server's OS permissions.
                    {/* Say whether a reload happens on its own: an operator who just
                        rebuilt a plugin needs to know whether to click or to wait. */}
                    {store.config.value?.plugins_watch
                      ? ` Plugin sources are watched every ${store.config.value.plugins_watch_secs ?? 2}s; edits apply without a click.`
                      : ' Reload or rescan after changing a plugin. Nothing is applied automatically.'}
                  </p>
                  {diagnosticsError ? (
                    <div class="inline-error" role="alert">
                      <p>{diagnosticsError}</p>
                    </div>
                  ) : null}
                  {pluginActionError ? (
                    <div class="inline-error" role="alert">
                      <p>{pluginActionError}</p>
                    </div>
                  ) : null}
                  <div class="diagnostics-grid">
                    <div class="diagnostics-block">
                      <span class="eyebrow">Plugins</span>
                      {diagnosticsLoading && plugins === null ? <p class="muted small">Loading…</p> : null}
                      {plugins && plugins.length === 0 ? <p class="muted small">No plugins loaded.</p> : null}
                      <ul class="diagnostics-list">
                        {(plugins ?? []).map((plugin) => (
                          <li class="diagnostics-row" key={plugin.name}>
                            <span class={`status-dot status-${statusTone(plugin.status)}`} aria-hidden="true" />
                            <span class="diagnostics-name">{plugin.name}</span>
                            <span class="diagnostics-status">{plugin.status}</span>
                            <span class="diagnostics-meta">
                              {plugin.manifest?.version ? `v${plugin.manifest.version} · ` : ''}
                              {plugin.tools.length} tool{plugin.tools.length === 1 ? '' : 's'}
                            </span>
                            {plugin.error ? <span class="diagnostics-error">{plugin.error}</span> : null}
                            <span class="diagnostics-actions">
                              <button
                                type="button"
                                class="button button-small"
                                onClick={() => runPluginAction(plugin.name, 'reload')}
                                disabled={pluginBusy !== null}
                                title="Restart this plugin's process (pick up a rebuilt binary or an edited manifest)"
                              >
                                {pluginBusy === `reload:${plugin.name}` ? 'Reloading…' : 'Reload'}
                              </button>
                              <button
                                type="button"
                                class="button button-small"
                                onClick={() =>
                                  runPluginAction(
                                    plugin.name,
                                    plugin.status === 'disabled' ? 'enable' : 'disable',
                                  )
                                }
                                disabled={pluginBusy !== null}
                                title="Start or stop this plugin without restarting the server"
                              >
                                {plugin.status === 'disabled' ? 'Enable' : 'Disable'}
                              </button>
                            </span>
                          </li>
                        ))}
                      </ul>
                    </div>
                    <div class="diagnostics-block">
                      <span class="eyebrow">MCP servers</span>
                      {diagnosticsLoading && mcpServers === null ? <p class="muted small">Loading…</p> : null}
                      {mcpServers && mcpServers.length === 0 ? (
                        <p class="muted small">No MCP servers configured.</p>
                      ) : null}
                      <ul class="diagnostics-list">
                        {(mcpServers ?? []).map((server) => (
                          <li class="diagnostics-row" key={server.name}>
                            <span class={`status-dot status-${statusTone(server.status)}`} aria-hidden="true" />
                            <span class="diagnostics-name">{server.name}</span>
                            <span class="diagnostics-status">{server.status}</span>
                            <span class="diagnostics-meta">
                              {server.transport} · {server.tools.length} tool
                              {server.tools.length === 1 ? '' : 's'}
                            </span>
                            {server.error ? <span class="diagnostics-error">{server.error}</span> : null}
                          </li>
                        ))}
                      </ul>
                    </div>
                  </div>
                </section>

                <section class="security-note" aria-labelledby="settings-security-plugins">
                  <h3 id="settings-security-plugins">
                    <AlertIcon size={15} /> Trusted code
                  </h3>
                  <p>
                    Plugins and MCP servers run as separate processes with the server's OS permissions.
                    The status above is informational: their permissions are declarations, not an
                    enforced sandbox, so install only what you would run yourself.
                  </p>
                </section>
              </TabPanel>
            </div>

            <footer class="settings-footer">
              {errorCount > 0 ? (
                <p class="settings-error-summary" role="alert">
                  <AlertIcon size={13} />
                  <span>
                    {errorCount} setting{errorCount === 1 ? '' : 's'} need
                    {errorCount === 1 ? 's' : ''} attention in{' '}
                    {errorTabs.map(tabLabel).join(', ')}.
                  </span>
                </p>
              ) : null}
              <div class="settings-footer-actions">
                <button type="button" class="button" onClick={actions.closeSettings}>
                  Cancel
                </button>
                <button type="submit" class="button button-primary" disabled={disabled || saving}>
                  {saving ? 'Saving…' : 'Save changes'}
                </button>
              </div>
            </footer>
          </form>
        ) : null}
      </div>
    </Dialog>
  );
}
