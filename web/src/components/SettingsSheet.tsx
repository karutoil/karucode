import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { ApiError, toErrorMessage } from '../api/client';
import { listMcpServers, listPlugins } from '../api/endpoints';
import type {
  ApprovalMode,
  Config,
  ConfigPatch,
  Limits,
  McpServer,
  PluginDiagnostics,
  ProviderTestResult,
} from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';
import { Toggle } from './Toggle';
import { AlertIcon, CheckIcon, RefreshIcon } from './icons';

interface FormState {
  provider: string;
  baseUrl: string;
  model: string;
  temperature: string;
  maxTokens: string;
  topP: string;
  requestTimeout: string;
  parallelToolCalls: boolean;
  apiKey: string;
  approvalMode: ApprovalMode;
  allowCommands: string[];
  autoApprovePaths: string[];
  approvalTimeout: string;
  limits: Record<keyof Limits, string>;
  subagentConcurrent: string;
  subagentIterations: string;
  allowRecursive: boolean;
  subagentMaxDepth: string;
  subagentTimeout: string;
}

const LIMIT_FIELDS: { key: keyof Limits; label: string }[] = [
  { key: 'max_iterations', label: 'Max iterations' },
  { key: 'max_run_secs', label: 'Max run seconds' },
  { key: 'max_tool_output_bytes', label: 'Max tool output (bytes)' },
  { key: 'max_command_output_bytes', label: 'Max command output (bytes)' },
  { key: 'command_timeout_secs', label: 'Command timeout (s)' },
  { key: 'max_command_timeout_secs', label: 'Max command timeout (s)' },
  { key: 'max_file_read_bytes', label: 'Max file read (bytes)' },
  { key: 'search_max_results', label: 'Search results' },
  { key: 'context_max_chars', label: 'Context budget (chars)' },
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
    provider: config.model.provider,
    baseUrl: config.model.base_url,
    model: config.model.model,
    temperature: String(config.model.temperature),
    maxTokens: config.model.max_tokens === null ? '' : String(config.model.max_tokens),
    topP: config.model.top_p === null ? '' : String(config.model.top_p),
    requestTimeout: String(config.model.request_timeout_secs),
    parallelToolCalls: config.model.parallel_tool_calls,
    apiKey: '',
    approvalMode: config.approvals.mode,
    allowCommands: [...config.approvals.allow_commands],
    autoApprovePaths: [...config.approvals.auto_approve_paths],
    approvalTimeout: String(config.approvals.timeout_secs),
    limits: Object.fromEntries(
      LIMIT_FIELDS.map((field) => [field.key, String(config.limits[field.key])]),
    ) as Record<keyof Limits, string>,
    subagentConcurrent: String(config.subagents.max_concurrent),
    subagentIterations: String(config.subagents.max_iterations),
    allowRecursive: config.subagents.allow_recursive,
    subagentMaxDepth: String(config.subagents.max_depth),
    subagentTimeout: String(config.subagents.timeout_secs),
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

function validate(form: FormState): Record<string, string> {
  const errors: Record<string, string> = {};
  if (form.provider.trim() === '') errors.provider = 'Provider id is required.';
  if (form.baseUrl.trim() === '') errors.baseUrl = 'Base URL is required.';
  else if (!/^https?:\/\//i.test(form.baseUrl.trim())) errors.baseUrl = 'Use an http:// or https:// URL.';
  if (form.model.trim() === '') errors.model = 'Model name is required.';

  const temperature = numberValue(form.temperature);
  if (temperature === null || temperature < 0 || temperature > 2) {
    errors.temperature = 'Temperature must be between 0 and 2.';
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
    if (!integerAtLeast(form.limits[field.key], 1, 100_000_000)) {
      errors[`limits.${field.key}`] = `${field.label} must be a positive integer.`;
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
  if (!integerAtLeast(form.subagentConcurrent, 1, 32)) {
    errors.subagentConcurrent = 'Concurrency must be 1–32.';
  }
  if (!integerAtLeast(form.subagentIterations, 1, 1000)) {
    errors.subagentIterations = 'Iterations must be 1–1000.';
  }
  if (!integerAtLeast(form.subagentMaxDepth, 0, 10)) {
    errors.subagentMaxDepth = 'Max depth must be 0–10.';
  }
  if (!integerAtLeast(form.subagentTimeout, 1, 86_400)) {
    errors.subagentTimeout = 'Timeout must be 1–86400 seconds.';
  }

  return errors;
}

function buildPatch(form: FormState): ConfigPatch {
  const limits: Partial<Limits> = {};
  for (const field of LIMIT_FIELDS) {
    limits[field.key] = Number(form.limits[field.key]);
  }
  const patch: ConfigPatch = {
    model: {
      provider: form.provider.trim(),
      base_url: form.baseUrl.trim(),
      model: form.model.trim(),
      temperature: Number(form.temperature),
      max_tokens: form.maxTokens.trim() === '' ? null : Number(form.maxTokens),
      top_p: form.topP.trim() === '' ? null : Number(form.topP),
      request_timeout_secs: Number(form.requestTimeout),
      parallel_tool_calls: form.parallelToolCalls,
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
    },
  };
  if (form.apiKey.trim() !== '') {
    patch.model = { ...patch.model, api_key: form.apiKey.trim() };
  }
  return patch;
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

export function SettingsSheet(): JSX.Element | null {
  const open = store.settingsOpen.value;
  const [config, setConfig] = useState<Config | null>(null);
  const [form, setForm] = useState<FormState | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<ProviderTestResult | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [plugins, setPlugins] = useState<PluginDiagnostics[] | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServer[] | null>(null);
  const [diagnosticsError, setDiagnosticsError] = useState<string | null>(null);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [configReloadKey, setConfigReloadKey] = useState(0);
  const [diagnosticsKey, setDiagnosticsKey] = useState(0);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setForm(null);
    setLoadError(null);
    setSaveError(null);
    setErrors({});
    setTestResult(null);
    setTestError(null);
    setConfirmClear(false);
    actions
      .loadConfig()
      .then((result) => {
        if (cancelled) return;
        setConfig(result);
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

  const submit = async (): Promise<void> => {
    if (!form || disabled) return;
    const validation = validate(form);
    setErrors(validation);
    if (Object.keys(validation).length > 0) return;
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await actions.saveConfig(buildPatch(form));
      setConfig(updated);
      setForm(formFromConfig(updated));
      setLocked(updated.settings_locked);
      setTestResult(null);
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

  const runTest = async (): Promise<void> => {
    setTesting(true);
    setTestResult(null);
    setTestError(null);
    try {
      setTestResult(await actions.testProvider());
    } catch (cause) {
      setTestError(toErrorMessage(cause));
    } finally {
      setTesting(false);
    }
  };

  const clearKey = async (): Promise<void> => {
    try {
      const updated = await actions.clearApiKey();
      setConfig(updated);
      setForm((current) => (current ? { ...current, apiKey: '' } : current));
      setConfirmClear(false);
    } catch (cause) {
      setSaveError(toErrorMessage(cause));
    }
  };

  const approvalDescription =
    APPROVAL_MODES.find((mode) => mode.value === form?.approvalMode)?.description ?? '';

  return (
    <Dialog
      open
      title="Settings"
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
            onSubmit={(event) => {
              event.preventDefault();
              void submit();
            }}
          >
            <section class="settings-section" aria-labelledby="settings-provider">
              <div class="settings-section-head">
                <h3 id="settings-provider">Model provider</h3>
                <span class="settings-section-note">OpenAI-compatible endpoint</span>
              </div>
              <div class="field-grid">
                <Field
                  id="provider"
                  label="Provider"
                  error={errors.provider}
                  hint="The only built-in provider id is openai_compatible."
                >
                  <input
                    id="provider"
                    class="input mono"
                    type="text"
                    value={form.provider}
                    disabled={disabled}
                    onInput={(event) => update({ provider: event.currentTarget.value })}
                    aria-invalid={errors.provider ? 'true' : undefined}
                    aria-describedby={errors.provider ? 'provider-error' : 'provider-hint'}
                  />
                </Field>
                <Field
                  id="model"
                  label="Model"
                  error={errors.model}
                  hint="Model name sent to the endpoint."
                >
                  <input
                    id="model"
                    class="input mono"
                    type="text"
                    value={form.model}
                    disabled={disabled}
                    onInput={(event) => update({ model: event.currentTarget.value })}
                    aria-invalid={errors.model ? 'true' : undefined}
                    aria-describedby={errors.model ? 'model-error' : 'model-hint'}
                  />
                </Field>
                <Field
                  id="base-url"
                  label="Base URL"
                  error={errors.baseUrl}
                  wide
                  hint="Includes the API version, e.g. http://127.0.0.1:4000/v1."
                >
                  <input
                    id="base-url"
                    class="input mono"
                    type="url"
                    value={form.baseUrl}
                    disabled={disabled}
                    data-autofocus
                    onInput={(event) => update({ baseUrl: event.currentTarget.value })}
                    aria-invalid={errors.baseUrl ? 'true' : undefined}
                    aria-describedby={errors.baseUrl ? 'base-url-error' : 'base-url-hint'}
                  />
                </Field>
                <Field
                  id="temperature"
                  label="Temperature"
                  error={errors.temperature}
                  hint="0–2. Lower is more deterministic."
                >
                  <input
                    id="temperature"
                    class="input mono"
                    type="number"
                    step="0.05"
                    min="0"
                    max="2"
                    value={form.temperature}
                    disabled={disabled}
                    onInput={(event) => update({ temperature: event.currentTarget.value })}
                    aria-invalid={errors.temperature ? 'true' : undefined}
                    aria-describedby={errors.temperature ? 'temperature-error' : 'temperature-hint'}
                  />
                </Field>
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
              </div>

              <div class="field-grid">
                <Field
                  id="api-key"
                  label="API key"
                  wide
                  hint="Write-only: the server never returns a stored key, only whether one is configured."
                >
                  <div class="api-key-row">
                    <input
                      id="api-key"
                      class="input mono"
                      type="password"
                      autoComplete="off"
                      placeholder={config?.model.api_key_set ? 'Configured — enter a new key to replace it' : 'sk-…'}
                      value={form.apiKey}
                      disabled={disabled}
                      aria-describedby="api-key-hint"
                      onInput={(event) => update({ apiKey: event.currentTarget.value })}
                    />
                    <span class={`chip ${config?.model.api_key_set ? 'chip-ok' : 'chip-muted'}`}>
                      {config?.model.api_key_set ? (
                        <>
                          <CheckIcon size={12} /> Configured
                        </>
                      ) : (
                        'Not configured'
                      )}
                    </span>
                  </div>
                </Field>
                {config?.model.api_key_set ? (
                  <div class="field field-wide clear-key-row">
                    {confirmClear ? (
                      <>
                        <span class="field-hint">Remove the stored API key from the server?</span>
                        <button
                          type="button"
                          class="button button-small button-danger"
                          disabled={disabled}
                          onClick={() => void clearKey()}
                        >
                          Confirm clear
                        </button>
                        <button type="button" class="button button-small" onClick={() => setConfirmClear(false)}>
                          Cancel
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        class="button button-small button-danger-ghost"
                        disabled={disabled}
                        onClick={() => setConfirmClear(true)}
                      >
                        Clear stored key
                      </button>
                    )}
                  </div>
                ) : null}
              </div>

              <div class="test-connection">
                <button
                  type="button"
                  class="button button-small"
                  onClick={() => void runTest()}
                  disabled={testing}
                >
                  <RefreshIcon size={13} class={testing ? 'spin' : undefined} />
                  {testing ? 'Testing…' : 'Test connection'}
                </button>
                <span class="field-hint">Uses the saved configuration — save first if you changed the fields.</span>
              </div>
              {testResult ? (
                <div class={`test-result ${testResult.ok ? 'is-ok' : 'is-error'}`} role="status">
                  <strong>{testResult.ok ? 'Connection succeeded' : 'Connection failed'}</strong>
                  <span>{testResult.latency_ms} ms</span>
                  {testResult.models && testResult.models.length > 0 ? (
                    <span class="test-models">
                      {testResult.models.length} model{testResult.models.length === 1 ? '' : 's'}:{' '}
                      {testResult.models.slice(0, 8).join(', ')}
                      {testResult.models.length > 8 ? '…' : ''}
                    </span>
                  ) : null}
                  {testResult.error ? <span class="test-error">{testResult.error}</span> : null}
                </div>
              ) : null}
              {testError ? (
                <div class="run-error-banner" role="alert">
                  <p class="run-error-message">{testError}</p>
                </div>
              ) : null}
            </section>

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

            <section class="settings-section" aria-labelledby="settings-limits">
              <div class="settings-section-head">
                <h3 id="settings-limits">Limits</h3>
                <span class="settings-section-note">Resource and context bounds</span>
              </div>
              <div class="field-grid field-grid-compact">
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
                >
                  <input
                    id="subagent-concurrent"
                    class="input mono"
                    type="number"
                    min="1"
                    max="32"
                    step="1"
                    value={form.subagentConcurrent}
                    disabled={disabled}
                    onInput={(event) => update({ subagentConcurrent: event.currentTarget.value })}
                    aria-invalid={errors.subagentConcurrent ? 'true' : undefined}
                    aria-describedby={
                      errors.subagentConcurrent ? 'subagent-concurrent-error' : undefined
                    }
                  />
                </Field>
                <Field
                  id="subagent-iterations"
                  label="Max iterations"
                  error={errors.subagentIterations}
                >
                  <input
                    id="subagent-iterations"
                    class="input mono"
                    type="number"
                    min="1"
                    max="1000"
                    step="1"
                    value={form.subagentIterations}
                    disabled={disabled}
                    onInput={(event) => update({ subagentIterations: event.currentTarget.value })}
                    aria-invalid={errors.subagentIterations ? 'true' : undefined}
                    aria-describedby={
                      errors.subagentIterations ? 'subagent-iterations-error' : undefined
                    }
                  />
                </Field>
                <Field
                  id="subagent-max-depth"
                  label="Max depth"
                  error={errors.subagentMaxDepth}
                  hint="Only meaningful when recursion is allowed."
                >
                  <input
                    id="subagent-max-depth"
                    class="input mono"
                    type="number"
                    min="0"
                    max="10"
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
                    max="86400"
                    step="1"
                    value={form.subagentTimeout}
                    disabled={disabled}
                    onInput={(event) => update({ subagentTimeout: event.currentTarget.value })}
                    aria-invalid={errors.subagentTimeout ? 'true' : undefined}
                    aria-describedby={errors.subagentTimeout ? 'subagent-timeout-error' : undefined}
                  />
                </Field>
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

            <section class="settings-section" aria-labelledby="settings-plugins">
              <div class="settings-section-head">
                <h3 id="settings-plugins">Plugins &amp; MCP</h3>
                <button
                  type="button"
                  class="button button-small"
                  onClick={() => setDiagnosticsKey((key) => key + 1)}
                  disabled={diagnosticsLoading}
                >
                  <RefreshIcon size={13} class={diagnosticsLoading ? 'spin' : undefined} /> Refresh status
                </button>
              </div>
              <p class="settings-section-note">
                Read-only diagnostics. Plugins and MCP servers run as separate processes with the server's OS
                permissions.
              </p>
              {diagnosticsError ? (
                <div class="inline-error" role="alert">
                  <p>{diagnosticsError}</p>
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

            <section class="security-note" aria-labelledby="settings-security">
              <h3 id="settings-security">
                <AlertIcon size={15} /> Security note
              </h3>
              <p>
                Shell commands are <strong>not sandboxed</strong>. <code>run_command</code> executes{' '}
                <code>/bin/sh -c &lt;command&gt;</code> as the server user, with that user's filesystem and network
                access. The approval policy and command allowlist are convenience gates, not isolation. Run
                karucode in a container or VM, or as a dedicated unprivileged user, if you need real isolation.
              </p>
              <p class="muted small">
                Plugins and MCP servers are trusted code. Their diagnostics above are informational; permissions
                are declarations, not an enforced sandbox.
              </p>
            </section>

            <footer class="settings-footer">
              <button type="button" class="button" onClick={actions.closeSettings}>
                Cancel
              </button>
              <button type="submit" class="button button-primary" disabled={disabled || saving}>
                {saving ? 'Saving…' : 'Save changes'}
              </button>
            </footer>
          </form>
        ) : null}
      </div>
    </Dialog>
  );
}