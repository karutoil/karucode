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
  ApiKeySummary,
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
  /** Every `/api/auth/api-keys…` write: mint, rotate, revoke. */
  keyWrites: { method: string; url: string; body: unknown; admin: string | null }[];
  /**
   * How the next `POST /api/auth/api-keys` fails, if it must. Set after
   * `installFetch` and before the click. `network` is the ambiguous case: the
   * stub mints the key server-side and then drops the connection, so the list
   * re-read finds it — the same shape as the real client's 30s deadline firing
   * after the server persisted the key.
   */
  failCreate: { kind: 'network'; minted: ApiKeySummary } | { kind: 'refused'; status: number; code: string; message: string } | null;
}

/** One key as the API lists it: Unix seconds, no secret. */
function keyFixture(overrides: Partial<ApiKeySummary> = {}): ApiKeySummary {
  return {
    id: 'abcdef1234567890',
    name: 'ci-runner',
    prefix: 'kck_abcdef1234567890',
    last4: 'abcd',
    scopes: ['read', 'write'],
    created_at: 1_731_420_000,
    expires_at: 1_734_012_000,
    revoked_at: null,
    last_used_at: null,
    last_used_ip: null,
    request_count: 0,
    status: 'active',
    ...overrides,
  };
}

/**
 * Stub the API this component reads.
 *
 * `access` is what `GET /api/onboarding` reports, because that is where the
 * Access summary comes from (`refreshAccessSettings`), and it is the only place
 * the password/passkey/session state is read.
 */
function installFetch(
  access: AccessSettingsSummary,
  apiKeys: ApiKeySummary[] = [],
  signInRequired = true,
): Harness {
  const patches: ConfigPatch[] = [];
  const passwordWrites: { method: string; body: unknown }[] = [];
  const keyWrites: { method: string; url: string; body: unknown; admin: string | null }[] = [];
  let keys = [...apiKeys];
  // Declared before the fetch mock below so the mock can read and clear it.
  const harness: Harness = {
    patches,
    passwordWrites,
    keyWrites,
    failCreate: null,
  };

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
    if (url === '/api/auth/api-keys' && method === 'GET') {
      return jsonResponse({
        keys,
        max_keys: 100,
        max_lifetime_days: 3650,
        sign_in_required: signInRequired,
      });
    }
    if (url === '/api/auth/api-keys' && method === 'POST') {
      const body: Record<string, unknown> =
        typeof init?.body === 'string' ? (JSON.parse(init.body) as Record<string, unknown>) : {};
      keyWrites.push({
        method,
        url,
        body,
        admin: new Headers(init?.headers).get('x-karucode-admin-token'),
      });
      if (harness.failCreate !== null) {
        const failure = harness.failCreate;
        harness.failCreate = null;
        if (failure.kind === 'network') {
          // The mint landed; only the answer was lost. A later list read must
          // see the key, which is what makes this failure ambiguous.
          keys = [failure.minted, ...keys];
          throw new TypeError('fetch failed');
        }
        return jsonResponse(
          { error: { code: failure.code, message: failure.message } },
          failure.status,
        );
      }
      const issued = {
        api_key: keyFixture({
          name: String(body.name ?? 'new-key'),
          expires_at: body.expires_in_days === undefined ? null : 1_740_000_000,
        }),
        token: 'kck_abcdef1234567890_secretsecretsecretsecretsecretabcd',
        warning: body.expires_in_days === undefined ? 'this key never expires' : null,
      };
      keys = [issued.api_key, ...keys];
      return jsonResponse(issued);
    }
    if (url.startsWith('/api/auth/api-keys/') && method === 'DELETE') {
      keyWrites.push({
        method,
        url,
        body: null,
        admin: new Headers(init?.headers).get('x-karucode-admin-token'),
      });
      const id = url.split('/')[4] ?? '';
      keys = keys.filter((key) => key.id !== id);
      return jsonResponse({ ok: true, id, purged: url.includes('purge=true') });
    }
    if (url.startsWith('/api/auth/api-keys/') && method === 'POST') {
      keyWrites.push({
        method,
        url,
        body: null,
        admin: new Headers(init?.headers).get('x-karucode-admin-token'),
      });
      return jsonResponse({
        api_key: keyFixture(),
        token: 'kck_abcdef1234567890_rotatedrotatedrotatedrotatedrotabcd',
        warning: null,
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
  return harness;
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

describe('AccessSettings: API keys', () => {
  it('lists a key with its scope, recognisable prefix and expiry — never a secret', async () => {
    installFetch(accessSummary(), [keyFixture()]);

    render(<AccessSettings />);

    expect(await screen.findByText('ci-runner')).not.toBeNull();
    // Only the leading `kck_<id>` and the tail, so a key is recognisable without
    // the credential being recoverable from the list.
    expect(screen.getByText(/kck_abcdef1234567890…abcd/)).not.toBeNull();
    const meta = screen.getByText(/kck_abcdef1234567890…abcd/).closest('.access-row-meta');
    expect(meta?.textContent).toMatch(/read \+ write/);
    expect(meta?.textContent).toMatch(/expires /);
    expect(screen.queryByText(/secretsecret/)).toBeNull();
  });

  it('creates a key, sends the chosen expiry and shows the token once', async () => {
    const harness = installFetch(accessSummary());

    render(<AccessSettings />);

    fireEvent.input(await screen.findByLabelText('Name', { selector: 'input' }), {
      target: { value: 'ci-runner' },
    });
    fireEvent.input(screen.getByLabelText('Expires in (days)'), { target: { value: '14' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create key' }));

    const token = 'kck_abcdef1234567890_secretsecretsecretsecretsecretabcd';
    expect(await screen.findByText(token)).not.toBeNull();
    await waitFor(() => expect(harness.keyWrites).toHaveLength(1));
    expect(harness.keyWrites[0]?.body).toEqual({
      name: 'ci-runner',
      scopes: ['read', 'write'],
      expires_in_days: 14,
    });

    // Dismissing the panel is permanent: the secret is not stored anywhere in
    // this component, so it cannot come back without a rotation.
    fireEvent.click(screen.getByRole('button', { name: 'I have saved it' }));
    expect(screen.queryByText(token)).toBeNull();
  });

  it('revokes only after the confirmation, and the request reaches the server', async () => {
    const harness = installFetch(accessSummary(), [keyFixture()]);

    render(<AccessSettings />);

    fireEvent.click(await screen.findByRole('button', { name: 'Revoke ci-runner' }));
    expect(harness.keyWrites).toHaveLength(0);

    fireEvent.click(screen.getByRole('button', { name: 'Revoke “ci-runner”' }));

    await waitFor(() => expect(harness.keyWrites).toHaveLength(1));
    expect(harness.keyWrites[0]?.method).toBe('DELETE');
    expect(harness.keyWrites[0]?.url).toBe('/api/auth/api-keys/abcdef1234567890');
    expect(await screen.findByText(/revoked\. It cannot authenticate again\./)).not.toBeNull();
  });

  it('sends the admin credential when one was typed', async () => {
    const harness = installFetch(accessSummary());

    render(<AccessSettings />);

    fireEvent.input(await screen.findByLabelText('Name', { selector: 'input' }), {
      target: { value: 'ci-runner' },
    });
    fireEvent.input(screen.getByLabelText('Admin token (only when configured)'), {
      target: { value: 'admin-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create key' }));

    await waitFor(() => expect(harness.keyWrites).toHaveLength(1));
    expect(harness.keyWrites[0]?.admin).toBe('admin-secret');
  });

  it('explains why creation is refused on a server that requires no credential', async () => {
    installFetch(accessSummary({ enabled: false, password_set: false }), [], false);

    render(<AccessSettings />);

    expect(await screen.findByText(/requires no credential yet/)).not.toBeNull();
    const create = screen.getByRole('button', { name: 'Create key' }) as HTMLButtonElement;
    expect(create.disabled).toBe(true);
  });

  it('treats a create that failed ambiguously as "the key may exist": no second mint, the list is re-read', async () => {
    // The POST dies on the wire (the client's own deadline or a dropped
    // connection), but the server side of the stub did mint the key — exactly
    // the ambiguity the finding is about: the sole copy of the token was in
    // the answer that never arrived.
    const harness = installFetch(accessSummary());
    const name = 'ci-runner';
    harness.failCreate = { kind: 'network', minted: keyFixture({ name }) };

    render(<AccessSettings />);

    fireEvent.input(await screen.findByLabelText('Name', { selector: 'input' }), {
      target: { value: name },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create key' }));

    // The message a person reads says the key may already exist and what to do
    // about it — not a bare network error next to an armed "Create key" button.
    expect(await screen.findByText(/may already have been created/)).not.toBeNull();
    expect(screen.getByText(/rotate it to get a new token/)).not.toBeNull();

    // The re-read happened, so the key the server minted is visible in the list
    // instead of hiding behind the failure.
    expect(await screen.findByText(/kck_abcdef1234567890…abcd/)).not.toBeNull();

    // The blind retry is not taken for them: exactly one create was attempted.
    expect(harness.keyWrites).toHaveLength(1);
  });

  it('keeps the server’s own message for a refusal that minted nothing', async () => {
    // A 400 is a definitive answer: nothing was minted, so the message is the
    // server’s own and the form stays armed for a corrected retry.
    const harness = installFetch(accessSummary());
    harness.failCreate = {
      kind: 'refused',
      status: 400,
      code: 'bad_request',
      message: 'this server already has 100 live API keys; revoke one before creating another',
    };

    render(<AccessSettings />);

    fireEvent.input(await screen.findByLabelText('Name', { selector: 'input' }), {
      target: { value: 'ci-runner' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create key' }));

    expect(await screen.findByText(/revoke one before creating another/)).not.toBeNull();
    expect(screen.queryByText(/may already have been created/)).toBeNull();
    expect(harness.keyWrites).toHaveLength(1);
  });
});
