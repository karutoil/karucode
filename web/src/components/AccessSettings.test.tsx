/**
 * The Access tab of the settings sheet.
 *
 * The tab is self-contained by design: it reads `/api/auth/*` and `/api/onboarding`
 * on mount and writes each thing it owns, so the properties worth pinning are
 * that the two password states are told apart from the server's own answer, that
 * the policy really reaches `PATCH /api/config`, and that a password the server
 * would refuse is refused *here*, before a request is spent on it.
 *
 * The fetch stub follows `SettingsSheet.test.tsx`: one `vi.fn` that answers by
 * path and method, so an assertion can name the request that was made.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { AccessSettings } from './AccessSettings';
import type {
  AccessSettingsSummary,
  AuthSession,
  AuthStatus,
  ConfigPatch,
  OnboardingState,
} from '../api/types';
import { NOW, configFixture, jsonResponse, resetAppState } from '../test/fixtures';

function accessSummary(overrides: Partial<AccessSettingsSummary> = {}): AccessSettingsSummary {
  return {
    enabled: true,
    password_set: true,
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
    ...overrides,
  };
}

function authStatus(overrides: Partial<AuthStatus> = {}): AuthStatus {
  return {
    enabled: true,
    sign_in_required: true,
    password_set: true,
    passkey_count: 0,
    needs_setup: false,
    signed_in: true,
    signed_in_with: 'password',
    token_required: false,
    session_days: 30,
    public_url: null,
    tunnel: 'none',
    trusted_proxies: [],
    cloudflare_proxies: false,
    origin: 'http://localhost:8787',
    rp_id: 'localhost',
    min_password_len: 10,
    max_password_len: 1024,
    passkey_challenge_ttl_secs: 300,
    ...overrides,
  };
}

const CURRENT_SESSION: AuthSession = {
  id: 'sess-current',
  created_at: NOW - 60_000,
  expires_at: NOW + 86_400_000,
  method: 'password',
  label: 'Chrome on macOS',
};

interface Harness {
  /** Every `PATCH /api/config` body, in order. */
  patches: ConfigPatch[];
  /** Every `/api/auth/password` write, so "no request" is checkable. */
  passwordWrites: { method: string; body: unknown }[];
}

/**
 * Stub the API this component reads.
 *
 * `access` is what `GET /api/onboarding` reports, because that is where the
 * Access summary comes from (`refreshAccessSettings`), and it is the only place
 * the password/passkey/session state is read.
 */
function installFetch(access: AccessSettingsSummary): Harness {
  const patches: ConfigPatch[] = [];
  const passwordWrites: { method: string; body: unknown }[] = [];

  const onboarding: OnboardingState = {
    needed: false,
    completed: true,
    provider_configured: true,
    has_sessions: true,
    project_count: 1,
    has_credential: access.password_set || access.passkeys.length > 0,
    access,
    workspace: '/home/me/project',
    version: '0.1.0',
  };

  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const method = (init?.method ?? 'GET').toUpperCase();
    if (url === '/api/onboarding') return jsonResponse(onboarding);
    if (url === '/api/auth/status') return jsonResponse(authStatus({ password_set: access.password_set }));
    if (url === '/api/auth/sessions') {
      return jsonResponse({
        sessions: access.sessions.length > 0 ? access.sessions : [CURRENT_SESSION],
        current: CURRENT_SESSION.id,
      });
    }
    if (url === '/api/auth/password') {
      passwordWrites.push({
        method,
        body: typeof init?.body === 'string' ? JSON.parse(init.body) : null,
      });
      return jsonResponse({ ok: true, enabled: true, enabled_now: false, session_days: 30 });
    }
    if (url === '/api/config' && method === 'PATCH') {
      const body: ConfigPatch =
        typeof init?.body === 'string' ? (JSON.parse(init.body) as ConfigPatch) : {};
      patches.push(body);
      return jsonResponse(configFixture);
    }
    if (url === '/api/config') return jsonResponse(configFixture);
    if (url === '/api/health') {
      return jsonResponse({
        status: 'ok',
        version: '0.1.0',
        uptime_secs: 1,
        workspace: '/home/me/project',
        projects: [],
        auth_required: access.enabled,
        provider_configured: true,
        db: 'ok',
      });
    }
    return jsonResponse({ error: { code: 'not_found', message: `unmocked ${method} ${url}` } }, 404);
  });

  vi.stubGlobal('fetch', fetchMock);
  return { patches, passwordWrites };
}

beforeEach(() => {
  resetAppState();
});

afterEach(() => {
  resetAppState();
  vi.unstubAllGlobals();
});

describe('AccessSettings: which password action is offered', () => {
  it('offers “Set a password” when the server has none', async () => {
    installFetch(accessSummary({ password_set: false, enabled: false }));
    render(<AccessSettings />);

    // The form is a "create" form: no current-password box, because there is
    // nothing to prove knowledge of.
    expect(await screen.findByText('Set a password')).not.toBeNull();
    expect(screen.queryByText('Change password')).toBeNull();
    expect(screen.queryByLabelText('Current password')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove password' })).toBeNull();
  });

  it('offers “Change password” when one is already set', async () => {
    installFetch(accessSummary({ password_set: true, enabled: true }));
    render(<AccessSettings />);

    expect(await screen.findByText('Change password')).not.toBeNull();
    expect(screen.queryByText('Set a password')).toBeNull();
    // Changing it needs the old one, and removing it is now an option.
    expect(screen.getByLabelText('Current password')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Remove password' })).not.toBeNull();
  });
});

describe('AccessSettings: the policy is saved by this tab', () => {
  it('PATCHes {access:{tunnel}} when the address form is saved', async () => {
    const harness = installFetch(accessSummary({ enabled: true }));

    render(<AccessSettings />);

    const tunnel = await screen.findByLabelText('How is it reached?');
    fireEvent.input(tunnel, { target: { value: 'cloudflare' } });
    const address = screen.getByLabelText('Public address');
    fireEvent.input(address, { target: { value: 'https://karucode.example.com' } });
    // A proxy list is only offered for a tunnel that forwards anything, which is
    // also what the PATCH has to carry.
    fireEvent.input(screen.getByLabelText('Trusted proxy addresses'), {
      target: { value: '10.0.0.0/8\n172.17.0.1' },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(harness.patches).toHaveLength(1));
    expect(harness.patches[0]?.access).toEqual({
      enabled: true,
      public_url: 'https://karucode.example.com',
      session_days: 30,
      tunnel: 'cloudflare',
      trusted_proxies: ['10.0.0.0/8', '172.17.0.1'],
    });
    expect(await screen.findByText('Access settings saved.')).not.toBeNull();
  });

  it('sends nothing when the public address is not a URL', async () => {
    const harness = installFetch(accessSummary());

    render(<AccessSettings />);

    const address = await screen.findByLabelText('Public address');
    fireEvent.input(address, { target: { value: 'karucode.example.com' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(
      await screen.findByText(/Use a full address such as https:\/\/karucode\.example\.com/),
    ).not.toBeNull();
    expect(harness.patches).toHaveLength(0);
  });
});

describe('AccessSettings: the password is validated before it is sent', () => {
  it('refuses a password shorter than the minimum without a request', async () => {
    const harness = installFetch(accessSummary({ password_set: false, enabled: false }));

    render(<AccessSettings />);

    fireEvent.input(await screen.findByLabelText('Password', { selector: 'input' }), {
      target: { value: 'short' },
    });
    fireEvent.input(screen.getByLabelText('Confirm password', { selector: 'input' }), {
      target: { value: 'short' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Set password' }));

    expect(await screen.findByText('Use at least 10 characters.')).not.toBeNull();
    // The point of the rule: a password the server would reject never costs a
    // round trip, and nothing was created in the meantime.
    expect(harness.passwordWrites).toHaveLength(0);
    expect(harness.patches).toHaveLength(0);
  });

  it('refuses a mismatched confirmation without a request', async () => {
    const harness = installFetch(accessSummary({ password_set: false, enabled: false }));

    render(<AccessSettings />);

    fireEvent.input(
      await screen.findByLabelText('Password', { selector: 'input' }),
      { target: { value: 'correct horse battery' } },
    );
    fireEvent.input(screen.getByLabelText('Confirm password', { selector: 'input' }), {
      target: { value: 'correct horse batteru' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Set password' }));

    expect(await screen.findByText('The two passwords do not match.')).not.toBeNull();
    expect(harness.passwordWrites).toHaveLength(0);
  });

  it('posts a password that passes the checks', async () => {
    const harness = installFetch(accessSummary({ password_set: false, enabled: false }));

    render(<AccessSettings />);

    fireEvent.input(
      await screen.findByLabelText('Password', { selector: 'input' }),
      { target: { value: 'correct horse battery' } },
    );
    fireEvent.input(screen.getByLabelText('Confirm password', { selector: 'input' }), {
      target: { value: 'correct horse battery' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Set password' }));

    await waitFor(() => expect(harness.passwordWrites).toHaveLength(1));
    expect(harness.passwordWrites[0]).toEqual({
      method: 'POST',
      body: { password: 'correct horse battery' },
    });
    expect(await screen.findByText(/Password saved/)).not.toBeNull();
  });

  it('saves on Enter from a password field, without a nested form', async () => {
    const harness = installFetch(accessSummary({ password_set: false, enabled: false }));

    const { container } = render(<AccessSettings />);

    fireEvent.input(await screen.findByLabelText('Password', { selector: 'input' }), {
      target: { value: 'correct horse battery' },
    });
    const confirm = screen.getByLabelText('Confirm password', { selector: 'input' });
    fireEvent.input(confirm, { target: { value: 'correct horse battery' } });
    fireEvent.keyDown(confirm, { key: 'Enter' });

    await waitFor(() => expect(harness.passwordWrites).toHaveLength(1));
    // The sheet already renders a `<form>`; this section must not nest another
    // one inside it.
    expect(container.querySelector('form')).toBeNull();
  });
});

describe('AccessSettings: the credential list is visible', () => {
  it('marks this browser in the session list', async () => {
    installFetch(accessSummary({ sessions: [CURRENT_SESSION] }));

    render(<AccessSettings />);

    expect(await screen.findByText('This browser')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Sign out Chrome on macOS' })).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Sign out everywhere' })).not.toBeNull();
  });

  it('warns when sign-in is off while a credential exists', async () => {
    installFetch(accessSummary({ enabled: false, password_set: true }));

    render(<AccessSettings />);

    expect(await screen.findByText(/this server is open to anyone who can reach the port/)).not.toBeNull();
    expect(screen.getByRole('switch', { name: 'Require a password or passkey' })).not.toBeNull();
  });
});
