import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import * as api from '../api/endpoints';
import type { ApprovalMode, TunnelKind } from '../api/types';
import { createPasskey, PasskeyError, passkeySupported } from '../lib/passkey';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as THEME from '../theme';
import type { ThemeDensity, ThemeToolStyle, ThemeThinking } from '../theme';
import { AlertIcon, CheckIcon, KeyIcon } from './icons';
import { Toggle } from './Toggle';

/**
 * The first-run wizard.
 *
 * Six steps, in the order a new install actually needs them: what this is, the
 * model endpoint, how the transcript should look, what the agent may do without
 * asking, who may reach the server, and a summary. Each step writes through the
 * API that already owns that setting, so there is no second source of truth:
 * providers and approvals are a `PATCH /api/config`, the password and passkeys go
 * to `/api/auth`, and appearance is the theme engine the settings sheet uses.
 *
 * It runs once. `GET /api/onboarding` decides, the completion marker lives in the
 * server's settings table, and "Skip" records the same marker - a wizard that
 * reappears on every reload is worse than no wizard.
 */

type StepId = 'welcome' | 'provider' | 'appearance' | 'approvals' | 'access' | 'done';

const STEPS: { id: StepId; title: string; blurb: string }[] = [
  {
    id: 'welcome',
    title: 'Welcome to karucode',
    blurb: 'Four decisions in six short steps: the model, the look, the approvals and how this server is reached.',
  },
  {
    id: 'provider',
    title: 'Connect a model provider',
    blurb: 'karucode talks to any OpenAI-compatible endpoint. Nothing works until this does.',
  },
  {
    id: 'appearance',
    title: 'Make it yours',
    blurb: 'Theme, density and how tool calls are drawn. Stored in this browser.',
  },
  {
    id: 'approvals',
    title: 'Decide what the agent may do alone',
    blurb: 'This is the human gate. It applies to every project and every session.',
  },
  {
    id: 'access',
    title: 'Who can reach this server',
    blurb:
      'A password or passkey, and — if you reach karucode through a tunnel — the name it answers to.',
  },
  {
    id: 'done',
    title: 'Ready',
    blurb: 'Everything you chose is saved. You can change any of it later in Settings.',
  },
];

const APPROVAL_CHOICES: { value: ApprovalMode; label: string; description: string }[] = [
  {
    value: 'ask',
    label: 'Ask me (recommended)',
    description: 'Shell commands and file changes wait for your approval.',
  },
  {
    value: 'ask_commands',
    label: 'Ask for shell commands only',
    description: 'File writes inside the project are allowed; commands still ask.',
  },
  {
    value: 'read_only',
    label: 'Read only',
    description: 'The agent may look, never change. Useful for a first cautious session.',
  },
  {
    value: 'auto',
    label: 'Never ask',
    description: 'Everything is allowed and recorded. Only for a disposable machine.',
  },
];

const TUNNELS: { value: TunnelKind; label: string; description: string }[] = [
  {
    value: 'none',
    label: 'No tunnel',
    description: 'Reached directly on this machine or your own network.',
  },
  {
    value: 'cloudflare',
    label: 'Cloudflare Tunnel',
    description:
      'cloudflared forwards from this machine. Cloudflare’s published proxy ranges are trusted automatically.',
  },
  {
    value: 'custom',
    label: 'Another reverse proxy',
    description: 'nginx, Caddy, Traefik, Tailscale — name the addresses it forwards from.',
  },
];

const DEFAULT_BASE_URL = 'https://api.openai.com/v1';

interface AccessDraft {
  enabled: boolean;
  domain: string;
  tunnel: TunnelKind;
  proxies: string;
  password: string;
  confirm: string;
}

export function Onboarding(): JSX.Element {
  const [step, setStep] = useState(0);
  const state = store.onboarding.value;
  const config = store.config.value;
  const headingRef = useRef<HTMLHeadingElement>(null);

  // ---- provider step state ---------------------------------------------
  const [provider, setProvider] = useState('openai_compatible');
  const [baseUrl, setBaseUrl] = useState(config?.model.base_url ?? DEFAULT_BASE_URL);
  const [apiKey, setApiKey] = useState('');
  const [model, setModel] = useState(config?.model.model ?? '');
  const [keyStored, setKeyStored] = useState(Boolean(config?.model.api_key_set));
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<
    { ok: boolean; message: string; models: string[] } | null
  >(null);

  // ---- appearance step state -------------------------------------------
  const [toolStyle, setToolStyleLocal] = useState<ThemeToolStyle>(THEME.activeTheme.value.toolStyle);
  const [density, setDensityLocal] = useState<ThemeDensity>(THEME.activeTheme.value.density);
  const [thinking, setThinkingLocal] = useState<ThemeThinking>(THEME.activeTheme.value.thinking);

  // ---- approvals step state --------------------------------------------
  const [approvalMode, setApprovalMode] = useState<ApprovalMode>(
    config?.approvals.mode ?? 'ask',
  );

  // ---- access step state -----------------------------------------------
  const [access, setAccess] = useState<AccessDraft>(() => ({
    enabled: state?.access.enabled ?? false,
    domain: state?.access.public_url ?? '',
    tunnel: state?.access.tunnel ?? 'none',
    proxies: (state?.access.trusted_proxies ?? []).join('\n'),
    password: '',
    confirm: '',
  }));
  const [passkeysAdded, setPasskeysAdded] = useState(0);
  /** Set when this wizard created the password: the stored state predates it. */
  const [passwordJustSet, setPasswordJustSet] = useState(false);
  const [passkeyBusy, setPasskeyBusy] = useState(false);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    void actions.refreshOnboarding();
  }, []);

  useEffect(() => {
    headingRef.current?.focus();
  }, [step]);

  // `step` is clamped by every transition; the fallback keeps the type honest.
  const current = STEPS[step] ?? STEPS[0]!;
  const passwordSet =
    passwordJustSet ||
    Boolean(store.authStatus.value?.password_set) ||
    Boolean(state?.access.password_set);
  const passkeyCount = (state?.access.passkeys.length ?? 0) + passkeysAdded;
  const hasCredential = passwordSet || passkeyCount > 0 || access.password !== '';

  /** Whether the current step has everything it needs to move on. */
  const canContinue = useMemo(() => {
    if (current.id === 'provider') {
      // Only the model id is required. A local gateway (Ollama, vLLM, a
      // llama.cpp server) takes no key, and refusing to continue until one is
      // typed would make the wizard impossible to finish on exactly the install
      // that has no account anywhere.
      return model.trim() !== '';
    }
    if (current.id === 'access') {
      if (access.password !== '' && access.password !== access.confirm) return false;
      if (access.enabled && !hasCredential) return false;
      if (access.domain.trim() !== '' && !/^https?:\/\/[^\s]+$/i.test(access.domain.trim())) {
        return false;
      }
    }
    return true;
  }, [current.id, model, keyStored, apiKey, access, hasCredential]);

  async function runTest(): Promise<void> {
    setTesting(true);
    setTestResult(null);
    try {
      const result = await actions.testProvider({
        base_url: baseUrl.trim(),
        // The stored key is only used when the endpoint has not been changed;
        // sending the draft key here is what makes "test before save" honest.
        api_key: apiKey.trim() === '' ? undefined : apiKey.trim(),
        model: model.trim(),
      });
      setTestResult({
        ok: result.ok,
        message: result.ok
          ? result.models && result.models.length > 0
            ? `Connected in ${result.latency_ms} ms — ${result.models.length} models available`
            : `Connected in ${result.latency_ms} ms`
          : (result.error ?? 'The endpoint refused the request.'),
        models: result.models ?? [],
      });
    } catch (failure) {
      setTestResult({ ok: false, message: toErrorMessage(failure), models: [] });
    } finally {
      setTesting(false);
    }
  }

  /** Persist whatever the current step owns. Throws on a rejected save. */
  async function saveStep(): Promise<void> {
    switch (current.id) {
      case 'provider': {
        const patch: Parameters<typeof actions.applyConfig>[0] = {
          model: {
            provider,
            base_url: baseUrl.trim(),
            model: model.trim(),
          },
        };
        if (apiKey.trim() !== '') patch.model!.api_key = apiKey.trim();
        await actions.applyConfig(patch);
        setKeyStored(true);
        setApiKey('');
        break;
      }
      case 'approvals':
        await actions.applyConfig({ approvals: { mode: approvalMode } });
        break;
      case 'access':
        await saveAccess();
        break;
      default:
        break;
    }
  }

  async function saveAccess(): Promise<void> {
    // 1. A new password first: creating the first credential is what turns
    //    sign-in on server-side, so it has to happen before the policy patch.
    if (access.password !== '') {
      const result = await api.setPassword({ password: access.password });
      // The credential now exists but the payload this screen was built from
      // predates it, so without these two lines the summary would still claim
      // there is no way to sign in.
      setPasswordJustSet(true);
      void actions.refreshAuthStatus();
      await actions.refreshOnboarding();
      setNotice(
        result.enabled_now
          ? 'Sign-in is now required. This browser is already signed in.'
          : 'Password saved. This browser is already signed in.',
      );
      setAccess((draft) => ({ ...draft, password: '', confirm: '' }));
    }
    // 2. Then the policy: the public URL, the tunnel and the proxy list.
    const proxies = access.proxies
      .split(/[\n,]+/)
      .map((entry) => entry.trim())
      .filter((entry) => entry !== '');
    await actions.applyConfig({
      access: {
        enabled: access.enabled,
        public_url: access.domain.trim(),
        tunnel: access.tunnel,
        trusted_proxies: proxies,
      },
    });
  }

  async function addPasskey(): Promise<void> {
    setPasskeyBusy(true);
    setError(null);
    try {
      const options = await api.passkeyRegisterOptions();
      const registration = await createPasskey(options);
      const result = await api.registerPasskey({
        ...registration,
        challenge_id: options.challenge_id,
        name: defaultPasskeyName(),
      });
      setPasskeysAdded((count) => count + 1);
      setAccess((draft) => ({ ...draft, enabled: true }));
      setNotice(
        result.enabled_now
          ? `Passkey “${result.passkey.name}” added. Sign-in is now required.`
          : `Passkey “${result.passkey.name}” added.`,
      );
    } catch (failure) {
      setError(failure instanceof PasskeyError ? failure.message : toErrorMessage(failure));
    } finally {
      setPasskeyBusy(false);
    }
  }

  async function next(): Promise<void> {
    if (busy) return;
    setError(null);
    setBusy(true);
    try {
      await saveStep();
      if (step === STEPS.length - 1) {
        await actions.finishOnboarding(false);
        return;
      }
      setStep((index) => Math.min(index + 1, STEPS.length - 1));
    } catch (failure) {
      setError(toErrorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  async function skip(): Promise<void> {
    setBusy(true);
    try {
      await actions.finishOnboarding(true);
    } finally {
      setBusy(false);
    }
  }

  const finishing = current.id === 'done';

  return (
    <div class="full-screen-state onboarding-screen">
      <div class="state-card onboarding-card">
        <div class="onboarding-head">
          <div class="brand">
            <span class="brand-dot" aria-hidden="true" />
            <span class="brand-name">karucode</span>
          </div>
          <span class="onboarding-progress" aria-live="polite">
            Step {step + 1} of {STEPS.length}
          </span>
        </div>

        <ol class="onboarding-rail" aria-label="Setup steps">
          {STEPS.map((entry, index) => (
            <li
              key={entry.id}
              class={
                index < step
                  ? 'onboarding-rail-item is-done'
                  : index === step
                    ? 'onboarding-rail-item is-current'
                    : 'onboarding-rail-item'
              }
              aria-current={index === step ? 'step' : undefined}
            >
              <span class="onboarding-rail-dot" aria-hidden="true">
                {index < step ? <CheckIcon size={11} /> : index + 1}
              </span>
              <span class="onboarding-rail-label">{entry.title}</span>
            </li>
          ))}
        </ol>

        <h1 class="state-title onboarding-title" tabIndex={-1} ref={headingRef}>
          {current.title}
        </h1>
        <p class="state-body">{current.blurb}</p>

        {error ? (
          <div class="state-error" role="alert">
            <AlertIcon size={15} />
            <span>{error}</span>
          </div>
        ) : null}
        {notice && !error ? (
          <div class="onboarding-notice" role="status">
            <CheckIcon size={14} />
            <span>{notice}</span>
          </div>
        ) : null}

        <div class="onboarding-body">
          {current.id === 'welcome' ? (
            <ul class="onboarding-points">
              <li>
                <strong>Bring an endpoint.</strong> Any OpenAI-compatible API works: OpenAI,
                OpenRouter, a local gateway, a self-hosted vLLM.
              </li>
              <li>
                <strong>Point it at a project.</strong> A project is a directory; every session in
                it is confined to that directory.
              </li>
              <li>
                <strong>You stay in charge.</strong> Approvals decide what runs without asking.
              </li>
              {state?.workspace ? (
                <li>
                  Working directory: <code>{state.workspace}</code>
                </li>
              ) : null}
            </ul>
          ) : null}

          {current.id === 'provider' ? (
            <div class="onboarding-fields">
              <label class="field-label" for="onboarding-provider">
                Provider
              </label>
              <select
                id="onboarding-provider"
                class="input"
                value={provider}
                onInput={(event) => setProvider(event.currentTarget.value)}
              >
                {(config?.available_providers?.length
                  ? config.available_providers
                  : ['openai_compatible']
                ).map((id) => (
                  <option value={id} key={id}>
                    {id}
                  </option>
                ))}
              </select>

              <label class="field-label" for="onboarding-base-url">
                Base URL
              </label>
              <input
                id="onboarding-base-url"
                class="input mono"
                type="url"
                spellcheck={false}
                value={baseUrl}
                onInput={(event) => setBaseUrl(event.currentTarget.value)}
                placeholder="https://api.openai.com/v1"
              />
              <p class="field-hint">
                The endpoint root, ending in <code>/v1</code> for most providers.
              </p>

              <label class="field-label" for="onboarding-api-key">
                API key
              </label>
              <input
                id="onboarding-api-key"
                class="input mono"
                type="password"
                autoComplete="off"
                spellcheck={false}
                value={apiKey}
                onInput={(event) => setApiKey(event.currentTarget.value)}
                placeholder={keyStored ? 'A key is already stored — leave blank to keep it' : 'sk-…'}
              />
              <p class="field-hint">
                Stored on the server, never rendered back. Leave blank for a local endpoint that
                takes no key.
              </p>

              <label class="field-label" for="onboarding-model">
                Model
              </label>
              <input
                id="onboarding-model"
                class="input mono"
                spellcheck={false}
                value={model}
                onInput={(event) => setModel(event.currentTarget.value)}
                placeholder="gpt-4o-mini"
                list={testResult?.models.length ? 'onboarding-model-options' : undefined}
              />
              {testResult?.models.length ? (
                <datalist id="onboarding-model-options">
                  {testResult.models.map((name) => (
                    <option value={name} key={name} />
                  ))}
                </datalist>
              ) : null}

              <div class="onboarding-test-row">
                <button
                  type="button"
                  class="button button-small"
                  disabled={testing || model.trim() === ''}
                  onClick={() => void runTest()}
                >
                  {testing ? 'Testing…' : 'Test connection'}
                </button>
                {testResult ? (
                  <span
                    class={testResult.ok ? 'onboarding-test is-ok' : 'onboarding-test is-bad'}
                    role="status"
                  >
                    {testResult.ok ? <CheckIcon size={13} /> : <AlertIcon size={13} />}
                    {testResult.message}
                  </span>
                ) : null}
              </div>
            </div>
          ) : null}

          {current.id === 'appearance' ? (
            <div class="onboarding-fields">
              <span class="field-label">Theme</span>
              <div class="onboarding-choice-grid">
                {THEME.themeLibrary.value.slice(0, 6).map((entry) => (
                  <button
                    type="button"
                    key={entry.id}
                    class={
                      // `baseThemeId`, not `activeThemeId`: picking a density or
                      // tool style promotes the theme to a draft, and a draft's
                      // id is `draft` - which made the highlight vanish the
                      // moment any option beside it was touched.
                      THEME.baseThemeId.value === entry.id
                        ? 'onboarding-choice is-selected'
                        : 'onboarding-choice'
                    }
                    aria-pressed={THEME.baseThemeId.value === entry.id}
                    onClick={() => THEME.selectTheme(entry.id)}
                  >
                    <span class="onboarding-choice-title">{entry.name}</span>
                    {entry.description ? (
                      <span class="onboarding-choice-hint">{entry.description}</span>
                    ) : null}
                  </button>
                ))}
              </div>

              <span class="field-label">Tool calls</span>
              <div class="onboarding-segment" role="group" aria-label="Tool call appearance">
                <button
                  type="button"
                  class={toolStyle === 'rows' ? 'is-selected' : ''}
                  aria-pressed={toolStyle === 'rows'}
                  onClick={() => {
                    setToolStyleLocal('rows');
                    THEME.setToolStyle('rows');
                  }}
                >
                  Rows
                </button>
                <button
                  type="button"
                  class={toolStyle === 'cards' ? 'is-selected' : ''}
                  aria-pressed={toolStyle === 'cards'}
                  onClick={() => {
                    setToolStyleLocal('cards');
                    THEME.setToolStyle('cards');
                  }}
                >
                  Cards
                </button>
              </div>
              <p class="field-hint">
                Rows keep a turn with many calls compact; cards give each call its own block.
              </p>

              <span class="field-label">Density</span>
              <div class="onboarding-segment" role="group" aria-label="Density">
                {(['compact', 'comfortable', 'spacious'] as ThemeDensity[]).map((value) => (
                  <button
                    type="button"
                    key={value}
                    class={density === value ? 'is-selected' : ''}
                    aria-pressed={density === value}
                    onClick={() => {
                      setDensityLocal(value);
                      THEME.setDensity(value);
                    }}
                  >
                    {value.charAt(0).toUpperCase() + value.slice(1)}
                  </button>
                ))}
              </div>

              <span class="field-label">Model reasoning</span>
              <div class="onboarding-segment" role="group" aria-label="Thinking visibility">
                {(['collapsed', 'preview', 'expanded'] as ThemeThinking[]).map((value) => (
                  <button
                    type="button"
                    key={value}
                    class={thinking === value ? 'is-selected' : ''}
                    aria-pressed={thinking === value}
                    onClick={() => {
                      setThinkingLocal(value);
                      THEME.setThinking(value);
                    }}
                  >
                    {value.charAt(0).toUpperCase() + value.slice(1)}
                  </button>
                ))}
              </div>
              <p class="field-hint">
                Changes apply immediately and live in this browser, not in the server config.
              </p>
            </div>
          ) : null}

          {current.id === 'approvals' ? (
            <div class="onboarding-choice-list" role="radiogroup" aria-label="Approval mode">
              {APPROVAL_CHOICES.map((choice) => (
                <button
                  type="button"
                  key={choice.value}
                  role="radio"
                  aria-checked={approvalMode === choice.value}
                  class={
                    approvalMode === choice.value
                      ? 'onboarding-choice is-selected'
                      : 'onboarding-choice'
                  }
                  onClick={() => setApprovalMode(choice.value)}
                >
                  <span class="onboarding-choice-title">{choice.label}</span>
                  <span class="onboarding-choice-hint">{choice.description}</span>
                </button>
              ))}
            </div>
          ) : null}

          {current.id === 'access' ? (
            <div class="onboarding-fields">
              <div class="setting-row onboarding-toggle-row">
                <div class="setting-copy">
                  <span class="setting-name">Require sign-in</span>
                  <span class="setting-hint">
                    A password or passkey before the web UI answers. Leave off for a loopback-only
                    install.
                  </span>
                </div>
                <Toggle
                  checked={access.enabled}
                  label="Require sign-in"
                  onChange={(next) => setAccess((draft) => ({ ...draft, enabled: next }))}
                />
              </div>

              {passwordSet ? (
                <p class="field-hint">
                  A password is already set. Change it later in Settings → Access.
                </p>
              ) : (
                <>
                  <label class="field-label" for="onboarding-password">
                    Password
                  </label>
                  <input
                    id="onboarding-password"
                    class="input"
                    type="password"
                    autoComplete="new-password"
                    value={access.password}
                    onInput={(event) =>
                      setAccess((draft) => ({ ...draft, password: event.currentTarget.value }))
                    }
                  />
                  <label class="field-label" for="onboarding-password-confirm">
                    Confirm password
                  </label>
                  <input
                    id="onboarding-password-confirm"
                    class="input"
                    type="password"
                    autoComplete="new-password"
                    value={access.confirm}
                    onInput={(event) =>
                      setAccess((draft) => ({ ...draft, confirm: event.currentTarget.value }))
                    }
                  />
                  <p class="field-hint">
                    At least {store.authStatus.value?.min_password_len ?? 10} characters. Setting a
                    password turns sign-in on for this server.
                  </p>
                  {access.confirm !== '' && access.password !== access.confirm ? (
                    <p class="field-error" role="alert">
                      The two passwords do not match.
                    </p>
                  ) : null}
                </>
              )}

              <div class="onboarding-passkey-row">
                <button
                  type="button"
                  class="button button-small"
                  disabled={passkeyBusy || !passkeySupported()}
                  onClick={() => void addPasskey()}
                >
                  <KeyIcon size={14} />
                  {passkeyBusy ? ' Waiting for your device…' : ' Add a passkey instead'}
                </button>
                <span class="field-hint">
                  {passkeyCount > 0
                    ? `${passkeyCount} passkey${passkeyCount === 1 ? '' : 's'} registered.`
                    : passkeySupported()
                      ? 'Fingerprint, face, screen lock or a security key.'
                      : 'This browser cannot create a passkey; use a password.'}
                </span>
              </div>

              <label class="field-label" for="onboarding-domain">
                Public address (optional)
              </label>
              <input
                id="onboarding-domain"
                class="input mono"
                type="url"
                spellcheck={false}
                placeholder="https://karucode.example.com"
                value={access.domain}
                onInput={(event) =>
                  setAccess((draft) => ({ ...draft, domain: event.currentTarget.value }))
                }
              />
              <p class="field-hint">
                The address people type to reach this server. It is added to the allowed hosts, and
                passkeys are bound to it.
              </p>

              <label class="field-label" for="onboarding-tunnel">
                How is it reached?
              </label>
              <select
                id="onboarding-tunnel"
                class="input"
                value={access.tunnel}
                onInput={(event) =>
                  setAccess((draft) => ({
                    ...draft,
                    tunnel: event.currentTarget.value as TunnelKind,
                  }))
                }
              >
                {TUNNELS.map((tunnel) => (
                  <option value={tunnel.value} key={tunnel.value}>
                    {tunnel.label}
                  </option>
                ))}
              </select>
              <p class="field-hint">
                {TUNNELS.find((tunnel) => tunnel.value === access.tunnel)?.description}
              </p>

              {access.tunnel !== 'none' ? (
                <>
                  <label class="field-label" for="onboarding-proxies">
                    Trusted proxy addresses (optional)
                  </label>
                  <textarea
                    id="onboarding-proxies"
                    class="input mono onboarding-textarea"
                    rows={3}
                    spellcheck={false}
                    placeholder={'10.0.0.0/8\n172.17.0.1'}
                    value={access.proxies}
                    onInput={(event) =>
                      setAccess((draft) => ({ ...draft, proxies: event.currentTarget.value }))
                    }
                  />
                  <p class="field-hint">
                    One IP or CIDR per line. Forwarded headers are only believed from these
                    addresses
                    {access.tunnel === 'cloudflare'
                      ? '; Cloudflare’s own ranges are always included for a Cloudflare tunnel.'
                      : '.'}{' '}
                    Loopback is always trusted.
                  </p>
                </>
              ) : null}
            </div>
          ) : null}

          {current.id === 'done' ? (
            <dl class="onboarding-summary">
              <div>
                <dt>Model</dt>
                <dd>
                  {config?.model.model || model || '—'}
                  {config?.model.base_url ? ` via ${config.model.base_url}` : ''}
                </dd>
              </div>
              <div>
                <dt>Approvals</dt>
                <dd>
                  {APPROVAL_CHOICES.find((choice) => choice.value === approvalMode)?.label ??
                    approvalMode}
                </dd>
              </div>
              <div>
                <dt>Sign-in</dt>
                <dd>
                  {access.enabled
                    ? hasCredential
                      ? 'Required — a password or passkey is set'
                      : 'Required — but no credential is set yet'
                    : 'Off (anyone who can reach the port)'}
                </dd>
              </div>
              <div>
                <dt>Address</dt>
                <dd>{access.domain.trim() || 'this browser’s own origin'}</dd>
              </div>
              <div>
                <dt>Tunnel</dt>
                <dd>{TUNNELS.find((tunnel) => tunnel.value === access.tunnel)?.label}</dd>
              </div>
            </dl>
          ) : null}
        </div>

        <div class="onboarding-footer">
          <div class="onboarding-footer-left">
            {step > 0 && !finishing ? (
              <button
                type="button"
                class="button button-ghost"
                disabled={busy}
                onClick={() => {
                  setError(null);
                  setStep((index) => Math.max(0, index - 1));
                }}
              >
                Back
              </button>
            ) : null}
            {!finishing ? (
              <button type="button" class="button button-ghost" disabled={busy} onClick={() => void skip()}>
                Skip setup
              </button>
            ) : null}
          </div>
          <button
            type="button"
            class="button button-primary"
            disabled={busy || !canContinue}
            onClick={() => void next()}
          >
            {busy
              ? 'Saving…'
              : finishing
                ? 'Start working'
                : current.id === 'done'
                  ? 'Finish'
                  : 'Continue'}
          </button>
        </div>
      </div>
    </div>
  );
}

/** A readable default label for a passkey, from what the browser will admit to. */
function defaultPasskeyName(): string {
  try {
    const agent = navigator.userAgent;
    if (/iPhone|iPad/.test(agent)) return 'iPhone or iPad';
    if (/Macintosh/.test(agent)) return 'Mac (Touch ID)';
    if (/Windows/.test(agent)) return 'Windows Hello';
    if (/Android/.test(agent)) return 'Android passkey';
    if (/Linux/.test(agent)) return 'This device';
  } catch {
    // No navigator (tests, or a hardened browser): the server has a default too.
  }
  return 'Passkey';
}
