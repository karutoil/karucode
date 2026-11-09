import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { Onboarding } from './Onboarding';
import type { OnboardingState } from '../api/types';
import { configFixture, jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

function onboardingState(overrides: Partial<OnboardingState> = {}): OnboardingState {
  return {
    needed: true,
    completed: false,
    provider_configured: false,
    has_sessions: false,
    project_count: 1,
    has_credential: false,
    access: {
      enabled: false,
      password_set: false,
      passkeys: [],
      sessions: [],
      public_url: null,
      session_days: 30,
      tunnel: 'none',
      trusted_proxies: [],
      trusts_forwarded_headers: false,
      cloudflare_ranges: false,
      token_required: false,
      settings_locked: false,
    },
    workspace: '/home/me/project',
    version: '0.1.0',
    ...overrides,
  };
}

interface Call {
  url: string;
  method: string;
  body: unknown;
}

/** Stub the API: everything succeeds, and every call is recorded. */
function stubApi(state: OnboardingState = onboardingState()): { calls: Call[] } {
  const calls: Call[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      const method = init?.method ?? 'GET';
      const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined;
      calls.push({ url, method, body });
      if (url.endsWith('/api/onboarding')) return jsonResponse(state);
      if (url.endsWith('/api/config') && method === 'PATCH') {
        return jsonResponse({ ...configFixture, ...(body as object) });
      }
      if (url.endsWith('/api/config')) return jsonResponse(configFixture);
      if (url.endsWith('/api/auth/status')) {
        return jsonResponse({
          enabled: false,
          sign_in_required: false,
          password_set: false,
          passkey_count: 0,
          needs_setup: true,
          signed_in: false,
          signed_in_with: null,
          token_required: false,
          session_days: 30,
          public_url: null,
          tunnel: 'none',
          trusted_proxies: [],
          cloudflare_proxies: false,
          origin: 'http://127.0.0.1:8787',
          rp_id: '127.0.0.1',
          min_password_len: 10,
          max_password_len: 1024,
          passkey_challenge_ttl_secs: 300,
        });
      }
      if (url.endsWith('/api/onboarding/complete')) {
        return jsonResponse({ ok: true, completed: true, skipped: true });
      }
      if (url.endsWith('/api/auth/password')) {
        return jsonResponse({ ok: true, enabled: true, enabled_now: true, session_days: 30 });
      }
      if (url.endsWith('/api/health')) return jsonResponse({ status: 'ok', version: '0.1.0' });
      return jsonResponse({ error: { code: 'not_found', message: `not stubbed: ${url}` } }, 404);
    }),
  );
  return { calls };
}

function renderWizard(
  state: OnboardingState = onboardingState(),
  options: { keyStored?: boolean } = {},
): { calls: Call[] } {
  const { calls } = stubApi(state);
  store.config.value = {
    ...configFixture,
    model: { ...configFixture.model, api_key_set: options.keyStored ?? false },
  };
  store.onboarding.value = state;
  render(<Onboarding />);
  return { calls };
}

/** The step heading, which is unique (the rail repeats the title). */
function heading(name: string | RegExp): Promise<HTMLElement> {
  return screen.findByRole('heading', { name });
}

/** Fill the provider step with something valid. */
async function fillProvider(): Promise<void> {
  fireEvent.input(await screen.findByLabelText('API key'), { target: { value: 'sk-test' } });
  fireEvent.input(screen.getByLabelText('Model'), { target: { value: 'gpt-4o-mini' } });
}

beforeEach(() => {
  resetAppState();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('Onboarding', () => {
  it('starts on the welcome step and walks forward', async () => {
    renderWizard();
    expect(await heading('Welcome to karucode')).toBeTruthy();
    expect(screen.getByText('Step 1 of 6')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(await heading('Connect a model provider')).toBeTruthy();
    expect(screen.getByText('Step 2 of 6')).toBeTruthy();
  });

  it('cannot leave the provider step without a model', async () => {
    renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    const model = await screen.findByLabelText('Model');
    fireEvent.input(model, { target: { value: '' } });
    const continueButton = screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement;
    expect(continueButton.disabled).toBe(true);
    // A key is optional on purpose: a local gateway takes none.
    fireEvent.input(model, { target: { value: 'gpt-4o-mini' } });
    await waitFor(() => expect(continueButton.disabled).toBe(false));
  });

  it('lets an install that already has a stored key continue without retyping it', async () => {
    renderWizard(onboardingState(), { keyStored: true });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    fireEvent.input(await screen.findByLabelText('Model'), { target: { value: 'gpt-4o-mini' } });
    const continueButton = screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement;
    expect(continueButton.disabled).toBe(false);
  });

  it('saves the provider through PATCH /api/config', async () => {
    const { calls } = renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    fireEvent.input(await screen.findByLabelText('Base URL'), {
      target: { value: 'https://api.example.test/v1' },
    });
    await fillProvider();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Make it yours');
    const patch = calls.find((call) => call.url.endsWith('/api/config') && call.method === 'PATCH');
    expect(patch?.body).toEqual({
      model: {
        provider: 'openai_compatible',
        base_url: 'https://api.example.test/v1',
        model: 'gpt-4o-mini',
        api_key: 'sk-test',
      },
    });
  });

  it('saves the approval mode it was given', async () => {
    const { calls } = renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await fillProvider();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Make it yours');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Decide what the agent may do alone');
    fireEvent.click(await screen.findByRole('radio', { name: /Read only/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Who can reach this server');
    const patch = calls
      .filter((call) => call.url.endsWith('/api/config') && call.method === 'PATCH')
      .at(-1);
    expect(patch?.body).toEqual({ approvals: { mode: 'read_only' } });
  });

  it('refuses a mismatched password before it talks to the server', async () => {
    const { calls } = renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await fillProvider();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Make it yours');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Decide what the agent may do alone');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Who can reach this server');

    fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'a-long-password' } });
    fireEvent.input(screen.getByLabelText('Confirm password'), {
      target: { value: 'a-different-password' },
    });
    expect(await screen.findByText('The two passwords do not match.')).toBeTruthy();
    const continueButton = screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement;
    expect(continueButton.disabled).toBe(true);
    expect(calls.some((call) => call.url.endsWith('/api/auth/password'))).toBe(false);
  });

  it('refuses sign-in with no credential at all', async () => {
    renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await fillProvider();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Make it yours');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Decide what the agent may do alone');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Who can reach this server');
    fireEvent.click(screen.getByRole('switch', { name: 'Require sign-in' }));
    const continueButton = screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement;
    expect(continueButton.disabled).toBe(true);
  });

  it('saves the password and then the access policy', async () => {
    const { calls } = renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await fillProvider();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Make it yours');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Decide what the agent may do alone');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Who can reach this server');

    fireEvent.click(screen.getByRole('switch', { name: 'Require sign-in' }));
    fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'a-long-password' } });
    fireEvent.input(screen.getByLabelText('Confirm password'), {
      target: { value: 'a-long-password' },
    });
    fireEvent.input(screen.getByLabelText('Public address (optional)'), {
      target: { value: 'https://karucode.example.com' },
    });
    fireEvent.input(screen.getByLabelText('How is it reached?'), {
      target: { value: 'cloudflare' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Ready');
    const password = calls.find((call) => call.url.endsWith('/api/auth/password'));
    expect(password?.body).toEqual({ password: 'a-long-password' });
    // The summary must know the credential it just created exists: the payload
    // this screen was built from predates it.
    expect(screen.getByText(/Required — a password or passkey is set/)).toBeTruthy();
    const patch = calls
      .filter((call) => call.url.endsWith('/api/config') && call.method === 'PATCH')
      .at(-1);
    expect(patch?.body).toEqual({
      access: {
        enabled: true,
        public_url: 'https://karucode.example.com',
        tunnel: 'cloudflare',
        trusted_proxies: [],
      },
    });
  });

  it('records a skip so the wizard does not come back', async () => {
    const { calls } = renderWizard();
    fireEvent.click(screen.getByRole('button', { name: 'Skip setup' }));
    await waitFor(() => {
      expect(calls.some((call) => call.url.endsWith('/api/onboarding/complete'))).toBe(true);
    });
    const complete = calls.find((call) => call.url.endsWith('/api/onboarding/complete'));
    expect(complete?.body).toEqual({ skipped: true });
  });

  it('shows the summary it is about to finish with', async () => {
    renderWizard(
      onboardingState({
        access: { ...onboardingState().access, enabled: true, password_set: true },
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await fillProvider();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Make it yours');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Decide what the agent may do alone');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Who can reach this server');
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    await heading('Ready');
    expect(screen.getByRole('button', { name: 'Start working' })).toBeTruthy();
    expect(screen.getByText(/Required — a password or passkey is set/)).toBeTruthy();
  });
});
