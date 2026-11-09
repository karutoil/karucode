import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { AuthScreen } from './AuthScreen';
import type { AuthStatus } from '../api/types';
import { jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

function authStatus(overrides: Partial<AuthStatus> = {}): AuthStatus {
  return {
    enabled: true,
    sign_in_required: true,
    password_set: false,
    passkey_count: 0,
    needs_setup: false,
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
    ...overrides,
  };
}

/** Serve `GET /api/auth/status` and record every other request. */
function stubApi(status: AuthStatus): { calls: { url: string; body: unknown }[] } {
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined;
      calls.push({ url, body });
      if (url.endsWith('/api/auth/status')) return jsonResponse(status);
      if (url.endsWith('/api/auth/login')) {
        return jsonResponse({ ok: true, method: 'password', label: 'password', expires_at: 0 });
      }
      return jsonResponse({ error: { code: 'not_found', message: 'not stubbed' } }, 404);
    }),
  );
  return { calls };
}

/** jsdom exposes no WebAuthn container; `passkeySupported` reads this one. */
function stubCredentials(value: unknown): void {
  Object.defineProperty(navigator, 'credentials', {
    value,
    configurable: true,
    writable: true,
  });
}

beforeEach(() => {
  resetAppState();
});

afterEach(() => {
  vi.unstubAllGlobals();
  stubCredentials(undefined);
});

describe('AuthScreen', () => {
  it('asks for the password when a password is set', async () => {
    stubApi(authStatus({ password_set: true }));
    render(<AuthScreen />);
    expect(await screen.findByLabelText('Password')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeTruthy();
    expect(screen.queryByLabelText('Access token')).toBeNull();
  });

  it('shows the passkey button only when a passkey exists and the browser supports one', async () => {
    // jsdom has no WebAuthn; the screen must not offer a button that cannot work.
    stubCredentials(undefined);
    vi.stubGlobal('PublicKeyCredential', undefined);
    stubApi(authStatus({ passkey_count: 2, password_set: true }));
    const { unmount } = render(<AuthScreen />);
    await screen.findByLabelText('Password');
    expect(screen.queryByText(/Sign in with a passkey/)).toBeNull();
    unmount();

    vi.stubGlobal('PublicKeyCredential', class FakePublicKeyCredential {});
    stubCredentials({ get: async () => null, create: async () => null });
    stubApi(authStatus({ passkey_count: 2, password_set: true }));
    render(<AuthScreen />);
    expect(await screen.findByText(/Sign in with a passkey/)).toBeTruthy();
  });

  it('falls back to the bearer token when that is the only credential', async () => {
    stubApi(
      authStatus({ password_set: false, passkey_count: 0, token_required: true, enabled: false }),
    );
    render(<AuthScreen />);
    expect(await screen.findByLabelText('Access token')).toBeTruthy();
    expect(screen.queryByLabelText('Password')).toBeNull();
  });

  it('says so when the server demands sign-in but has no credential at all', async () => {
    stubApi(authStatus({ password_set: false, passkey_count: 0, token_required: false }));
    render(<AuthScreen />);
    expect(await screen.findByText(/Sign-in is required but no password or passkey/)).toBeTruthy();
  });

  it('posts the password to /api/auth/login', async () => {
    const { calls } = stubApi(authStatus({ password_set: true }));
    render(<AuthScreen />);
    const field = await screen.findByLabelText('Password');
    fireEvent.input(field, { target: { value: 'correct horse battery' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    await waitFor(() => {
      expect(calls.some((call) => call.url.endsWith('/api/auth/login'))).toBe(true);
    });
    const login = calls.find((call) => call.url.endsWith('/api/auth/login'));
    expect(login?.body).toEqual({ password: 'correct horse battery' });
  });

  it('surfaces the server rejection without clearing the form', async () => {
    stubbedRejection();
    render(<AuthScreen />);
    const field = await screen.findByLabelText('Password');
    fireEvent.input(field, { target: { value: 'wrong password!!' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByRole('alert')).toBeTruthy();
    expect((field as HTMLInputElement).value).toBe('wrong password!!');
  });

  it('shows the public address the server says it is protecting', async () => {
    stubApi(authStatus({ password_set: true, public_url: 'https://karucode.example.com' }));
    render(<AuthScreen />);
    expect(await screen.findByText('https://karucode.example.com')).toBeTruthy();
  });

  it('lets the password be revealed and hidden again', async () => {
    stubApi(authStatus({ password_set: true }));
    render(<AuthScreen />);
    const field = (await screen.findByLabelText('Password')) as HTMLInputElement;
    expect(field.type).toBe('password');
    const reveal = screen.getByRole('button', { name: 'Show password' });
    expect(reveal.getAttribute('aria-pressed')).toBe('false');
    fireEvent.click(reveal);
    expect((screen.getByLabelText('Password') as HTMLInputElement).type).toBe('text');
    const hide = screen.getByRole('button', { name: 'Hide password' });
    expect(hide.getAttribute('aria-pressed')).toBe('true');
    fireEvent.click(hide);
    expect((screen.getByLabelText('Password') as HTMLInputElement).type).toBe('password');
  });

  it('separates the passkey from the password instead of stacking two equal actions', async () => {
    stubCredentials({ get: async () => null, create: async () => null });
    vi.stubGlobal('PublicKeyCredential', class {});
    stubApi(authStatus({ password_set: true, passkey_count: 1 }));
    const { container } = render(<AuthScreen />);
    await screen.findByLabelText('Password');
    expect(screen.getByText(/Sign in with a passkey/)).toBeTruthy();
    expect(container.querySelector('.auth-or')).not.toBeNull();
  });
});

function stubbedRejection(): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input.toString();
      if (url.endsWith('/api/auth/status')) return jsonResponse(authStatus({ password_set: true }));
      return jsonResponse({ error: { code: 'unauthorized', message: 'that password is not correct' } }, 401);
    }),
  );
}

describe('AuthScreen store wiring', () => {
  it('reads the status the app already fetched instead of guessing', () => {
    store.authStatus.value = authStatus({ password_set: true });
    stubApi(authStatus({ password_set: true }));
    render(<AuthScreen />);
    expect(screen.getByLabelText('Password')).toBeTruthy();
  });
});
