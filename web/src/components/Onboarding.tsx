import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import * as api from '../api/endpoints';
import type { ApprovalMode } from '../api/types';
import { AccessStep } from '../features/onboarding/AccessStep';
import { AppearanceStep } from '../features/onboarding/AppearanceStep';
import { ApprovalsStep } from '../features/onboarding/ApprovalsStep';
import { ProviderStep } from '../features/onboarding/ProviderStep';
import { StepRail } from '../features/onboarding/StepRail';
import { SummaryStep } from '../features/onboarding/SummaryStep';
import { WelcomeStep } from '../features/onboarding/WelcomeStep';
import {
  DEFAULT_BASE_URL,
  STEPS,
  defaultPasskeyName,
  type AccessDraft,
  type ProviderTestResult,
} from '../features/onboarding/steps';
import { createPasskey, PasskeyError } from '../lib/passkey';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { AlertIcon, CheckIcon } from './icons';
import { Button, Callout, Text } from '../ui';

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
 *
 * This component owns the flow (which step, what has been typed, what a save
 * writes); each step body lives in `features/onboarding/` and is presentational.
 */
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
  const [testResult, setTestResult] = useState<ProviderTestResult | null>(null);

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
  const patchAccess = (patch: Partial<AccessDraft>): void =>
    setAccess((draft) => ({ ...draft, ...patch }));

  return (
    <div class="full-screen-state onboarding-screen">
      <div class="state-card onboarding-card">
        <div class="onboarding-head">
          <div class="brand">
            <span class="brand-dot" aria-hidden="true" />
            <span class="brand-name">karucode</span>
          </div>
          <Text as="span" size="tiny" tone="muted" class="onboarding-progress" aria-live="polite">
            Step {step + 1} of {STEPS.length}
          </Text>
        </div>

        <StepRail step={step} />

        {/* A bare `<h1>` rather than `Text`: the step change moves focus here so
            the new step is announced, and `Text` takes neither a ref nor a
            `tabIndex`. */}
        <h1 class="state-title onboarding-title" tabIndex={-1} ref={headingRef}>
          {current.title}
        </h1>
        <Text as="p" size="small" tone="muted" class="state-body">
          {current.blurb}
        </Text>

        {error ? (
          <Callout
            tone="danger"
            role="alert"
            icon={<AlertIcon size={15} />}
            class="onboarding-alert"
          >
            {error}
          </Callout>
        ) : null}
        {notice && !error ? (
          <Callout
            tone="success"
            role="status"
            icon={<CheckIcon size={14} />}
            class="onboarding-alert"
          >
            {notice}
          </Callout>
        ) : null}

        <div class="onboarding-body">
          {current.id === 'welcome' ? <WelcomeStep workspace={state?.workspace} /> : null}

          {current.id === 'provider' ? (
            <ProviderStep
              provider={provider}
              baseUrl={baseUrl}
              apiKey={apiKey}
              model={model}
              keyStored={keyStored}
              availableProviders={config?.available_providers ?? []}
              testing={testing}
              testResult={testResult}
              onProviderChange={setProvider}
              onBaseUrlChange={setBaseUrl}
              onApiKeyChange={setApiKey}
              onModelChange={setModel}
              onTest={() => void runTest()}
            />
          ) : null}

          {current.id === 'appearance' ? <AppearanceStep /> : null}

          {current.id === 'approvals' ? (
            <ApprovalsStep mode={approvalMode} onChange={setApprovalMode} />
          ) : null}

          {current.id === 'access' ? (
            <AccessStep
              access={access}
              passwordSet={passwordSet}
              passkeyCount={passkeyCount}
              passkeyBusy={passkeyBusy}
              minPasswordLength={store.authStatus.value?.min_password_len ?? 10}
              onPatch={patchAccess}
              onAddPasskey={() => void addPasskey()}
            />
          ) : null}

          {current.id === 'done' ? (
            <SummaryStep
              model={config?.model.model || model}
              baseUrl={config?.model.base_url ?? ''}
              approvalMode={approvalMode}
              access={access}
              hasCredential={hasCredential}
            />
          ) : null}
        </div>

        <div class="onboarding-footer">
          <div class="onboarding-footer-left">
            {step > 0 && !finishing ? (
              <Button
                variant="ghost"
                disabled={busy}
                onClick={() => {
                  setError(null);
                  setStep((index) => Math.max(0, index - 1));
                }}
              >
                Back
              </Button>
            ) : null}
            {!finishing ? (
              <Button variant="ghost" disabled={busy} onClick={() => void skip()}>
                Skip setup
              </Button>
            ) : null}
          </div>
          <Button
            variant="primary"
            disabled={busy || !canContinue}
            onClick={() => void next()}
          >
            {busy ? 'Saving…' : finishing ? 'Start working' : 'Continue'}
          </Button>
        </div>
      </div>
    </div>
  );
}
