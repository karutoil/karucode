import type { Config, ConfigPatch, Limits } from '../../api/types';
import { settingsSection, type SettingsScope, type SettingsTab } from '../../components/SettingsNav';
import type { ThemeDensity } from '../../theme';
import type { AdvancedState, FieldLocation, FormState, NumericLimitKey } from './types';

/** Density labels, matching the Theme studio so one setting reads the same in both. */
export const DENSITY_LABELS: Record<ThemeDensity, string> = {
  compact: 'Compact',
  comfortable: 'Comfortable',
  spacious: 'Spacious',
};

/**
 * Bounds for the `[limits]` numeric fields, mirroring `LIMITS_BOUNDS` in
 * `crates/kc-server/src/settings.rs`.
 *
 * The bounds live beside the label rather than in one rule for the whole grid,
 * because the fields disagree about `0` on purpose: `max_iterations = 0` and
 * `context_max_chars = 0` are the documented "no ceiling" and "derive it from
 * the model" spellings, and everything else needs a positive value. A single
 * `min = 1` for the grid is what made a saved `max_iterations = 0` - the default
 * the server hands back - fail this form's own validation with "must be a
 * positive integer", on a field whose label says `0 = no ceiling`.
 *
 * Keep these in step with the server: a value this form accepts and the server
 * refuses comes back as a 400 the user cannot act on.
 */
export const LIMIT_FIELDS: { key: NumericLimitKey; label: string; min: number; max: number }[] = [
  { key: 'max_iterations', label: 'Max iterations per run (0 = no ceiling)', min: 0, max: 10_000 },
  { key: 'max_run_secs', label: 'Max run seconds (whole run)', min: 10, max: 604_800 },
  { key: 'max_tool_output_bytes', label: 'Max tool output (bytes)', min: 256, max: 1_000_000 },
  {
    key: 'max_command_output_bytes',
    label: 'Max command output (bytes)',
    min: 256,
    max: 1_000_000,
  },
  { key: 'command_timeout_secs', label: 'Command timeout (s)', min: 1, max: 3_600 },
  { key: 'max_command_timeout_secs', label: 'Max command timeout (s)', min: 1, max: 3_600 },
  { key: 'max_file_read_bytes', label: 'Max file read (bytes)', min: 1, max: 5_000_000 },
  {
    key: 'search_max_results',
    label: 'Search results',
    min: 1,
    max: 2_000,
  },
  {
    key: 'context_max_chars',
    label: 'Context budget (chars, 0 = from the model)',
    min: 0,
    max: 4_000_000,
  },
  { key: 'max_tool_calls_per_turn', label: 'Tool calls per turn', min: 1, max: 256 },
  {
    key: 'max_turn_retries',
    label: 'Turn retries after a provider gives up (0 = off)',
    min: 0,
    max: 10,
  },
  {
    key: 'turn_retry_delay_ms',
    label: 'First turn-retry wait (ms, doubles)',
    min: 0,
    max: 600_000,
  },
];

export const APPROVAL_MODES: { value: FormState['approvalMode']; label: string; description: string }[] = [
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
    // The stored value stays `auto` (config key and wire value); only the label a
    // person reads names what the mode does.
    label: 'Approve All (no approval prompts)',
    description: 'Everything is allowed and recorded in the event log. Only use this in disposable environments.',
  },
  {
    value: 'read_only',
    label: 'Read only',
    description: 'Only read-only tools are allowed. Shell commands and mutations are denied.',
  },
];

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
export const SUBAGENT_BOUNDS = {
  concurrent: { min: 0, max: 1024 },
  maxDepth: { min: 0, max: 255 },
  maxBackground: { min: 0, max: 1024 },
} as const;

export const ADVANCED_CLOSED: AdvancedState = {
  general: false,
  models: false,
  agent: false,
  notifications: false,
};

/**
 * What the footer says about the section on screen when nothing is wrong.
 *
 * It is the counterweight to a shared Save button: Appearance has already been
 * applied by the time the button is reachable, and a user should not have to
 * press it — or wonder whether they must — to keep a theme.
 */
export const SCOPE_NOTES: Record<SettingsScope, string> = {
  browser: 'Applied immediately and stored in this browser — nothing to save here.',
  install: 'Saved to this install when you press Save changes.',
  mixed: 'What lives in this browser applies immediately; the rest is saved to this install.',
};

/**
 * Tabs whose controls commit themselves, so the footer must not imply that its
 * Save button is what keeps them.
 *
 * Skills save as they change (a switch has no Save of its own near it, and a
 * skill's row is a per-skill PATCH); the sheet's own form never sees them. Saying
 * "press Save changes" over such a pane is the small lie that teaches someone to
 * press a button that does nothing.
 */
export const SELF_SAVING: ReadonlySet<SettingsTab> = new Set<SettingsTab>([
  'skills',
  // A plugin's settings are saved by its own "Save and restart", which also
  // restarts that plugin. The footer's "Save changes" cannot apply them, so
  // leaving this tab out would make a saved edit look discarded.
  'plugins',
  // Same rule, same reasoning: Access commits each change the moment it is
  // made (a password, a passkey, a tunnel), because a half-typed credential in
  // a form would be a security state nobody asked for.
  'access',
  // Updates runs a check, installs a build and reports the result; there is
  // nothing for this sheet's form to carry, and "Save changes" over it was a
  // button that did nothing.
  'updates',
]);

export const SELF_SAVING_NOTE = 'This section saves every change as you make it.';

export const FIELD_LOCATIONS: Record<string, FieldLocation> = {
  temperature: { tab: 'models', advanced: 'models', anchor: 'temperature' },
  maxTokens: { tab: 'models', advanced: 'models', anchor: 'max-tokens' },
  topP: { tab: 'models', advanced: 'models', anchor: 'top-p' },
  requestTimeout: { tab: 'models', advanced: 'models', anchor: 'request-timeout' },
  approvalTimeout: { tab: 'agent', anchor: 'approval-timeout' },
  // Approvals are open on the Agent pane; the subagent budget and the raw limits
  // are both inside its Advanced fold.
  subagentConcurrent: { tab: 'agent', advanced: 'agent', anchor: 'subagent-concurrent' },
  subagentIterations: { tab: 'agent', advanced: 'agent', anchor: 'subagent-iterations' },
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

/** How long a jumped-to section stays highlighted (matches `settings.css`). */
export const FOUND_FLASH_MS = 1600;

export function formFromConfig(config: Config): FormState {
  return {
    sendTemperature: config.model.send_temperature,
    temperature: String(config.model.temperature),
    maxTokens: config.model.max_tokens === null ? '' : String(config.model.max_tokens),
    topP: config.model.top_p === null ? '' : String(config.model.top_p),
    requestTimeout: String(config.model.request_timeout_secs),
    parallelToolCalls: config.model.parallel_tool_calls,
    parallelExecution: config.limits.parallel_tool_calls,
    supportsImages: config.model.supports_images,
    approvalMode: config.approvals.mode,
    allowCommands: [...config.approvals.allow_commands],
    autoApprovePaths: [...config.approvals.auto_approve_paths],
    approvalTimeout: String(config.approvals.timeout_secs),
    limits: Object.fromEntries(
      LIMIT_FIELDS.map((field) => [field.key, String(config.limits[field.key])]),
    ) as Record<NumericLimitKey, string>,
    subagentConcurrent: String(config.subagents.max_concurrent),
    subagentIterations: String(config.subagents.max_iterations),
    allowRecursive: config.subagents.allow_recursive,
    subagentMaxDepth: String(config.subagents.max_depth),
    subagentTimeout: String(config.subagents.timeout_secs),
    allowBackground: config.subagents.allow_background,
    subagentMaxBackground: String(config.subagents.max_background),
    subagentWaitTimeout: String(config.subagents.wait_timeout_secs),
    subagentWorktrees: config.subagents.worktrees,
  };
}

export function numberValue(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === '') return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

export function integerAtLeast(value: string, min: number, max: number): boolean {
  const parsed = numberValue(value);
  return parsed !== null && Number.isInteger(parsed) && parsed >= min && parsed <= max;
}

export function validate(form: FormState): Record<string, string> {
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
    if (!integerAtLeast(form.limits[field.key], field.min, field.max)) {
      // Name the zero rule only for the fields that have one, so the message
      // matches the label the user is looking at.
      errors[`limits.${field.key}`] =
        field.min === 0
          ? `${field.label} must be 0 or between ${field.min + 1} and ${field.max}.`
          : `${field.label} must be between ${field.min} and ${field.max}.`;
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
  if (!integerAtLeast(form.subagentIterations, 0, 10_000)) {
    errors.subagentIterations = 'Iterations must be 0 (no ceiling) or 1–10000.';
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

export function buildPatch(form: FormState): ConfigPatch {
  const limits: Partial<Limits> = {};
  for (const field of LIMIT_FIELDS) {
    limits[field.key] = Number(form.limits[field.key]);
  }
  // A switch, not a bound: no validity rule, so it is set directly.
  limits.parallel_tool_calls = form.parallelExecution;
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
      allow_recursive: form.allowRecursive,
      max_depth: Number(form.subagentMaxDepth),
      timeout_secs: Number(form.subagentTimeout),
      allow_background: form.allowBackground,
      max_background: Number(form.subagentMaxBackground),
      wait_timeout_secs: Number(form.subagentWaitTimeout),
      worktrees: form.subagentWorktrees,
    },
  };
}

/** The tab a validated field belongs to, for the footer's error summary. */
export function tabOf(errorKey: string): SettingsTab {
  return FIELD_LOCATIONS[errorKey]?.tab ?? 'general';
}

/** The human label of a tab, for the footer's error summary. */
export function tabLabel(tab: SettingsTab): string {
  return settingsSection(tab).label;
}

/** Which Advanced disclosure, if any, holds a section element. */
export function advancedKeyOf(details: Element | null): keyof AdvancedState | null {
  if (!details) return null;
  // The disclosure's `id` prop is on its body (`SettingsAdvanced` renders
  // `<details>` + `.settings-advanced-body#id`), not on the `<details>`.
  const id = details.querySelector('.settings-advanced-body')?.id ?? '';
  if (id === 'settings-general-advanced') return 'general';
  if (id === 'settings-models-advanced') return 'models';
  if (id === 'settings-agent-advanced') return 'agent';
  if (id === 'settings-notifications-advanced') return 'notifications';
  return null;
}
