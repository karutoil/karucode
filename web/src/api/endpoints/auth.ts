import { apiRequest } from '../client';
import type {
  AccessPatch,
  ApiKeyIssued,
  ApiKeyListResponse,
  ApiKeyScope,
  ApiKeySummary,
  AuthPasskey,
  AuthSession,
  AuthStatus,
  OnboardingState,
  PasskeyAssertionResult,
  PasskeyRegistrationOptions,
  PasskeyRegistrationResult,
  PasskeyRequestOptions,
} from '../types/auth';
import { adminHeaders, type CallOptions } from './common';

// ---------------------------------------------------------------------------
// Web-UI sign-in (`/api/auth` and `/api/onboarding`)
// ---------------------------------------------------------------------------

/**
 * What the sign-in screen needs. **Public**: this is the one auth route a
 * signed-out browser may call, so it must not carry a credential.
 */
export function getAuthStatus(options: CallOptions = {}): Promise<AuthStatus> {
  // `skipAuthHandler`: this is the call the 401 handler itself makes, so letting
  // it re-enter would be an endless loop of requests.
  return apiRequest<AuthStatus>('/api/auth/status', { ...options, skipAuthHandler: true });
}

export function login(
  body: { password: string },
  options: CallOptions = {},
): Promise<{ ok: boolean; method: string; label: string; expires_at: number }> {
  // A rejected password is not an expired session: keep the message on screen.
  return apiRequest('/api/auth/login', { ...options, method: 'POST', body, skipAuthHandler: true });
}

export function logout(options: CallOptions = {}): Promise<{ ok: boolean }> {
  return apiRequest('/api/auth/logout', {
    ...options,
    method: 'POST',
    body: {},
    skipAuthHandler: true,
  });
}

export function getAuthMe(options: CallOptions = {}): Promise<{
  signed_in: boolean;
  principal: string;
  label: string | null;
  expires_at: number | null;
  token: boolean;
}> {
  return apiRequest('/api/auth/me', options);
}

export function setPassword(
  body: { password: string; current_password?: string },
  options: CallOptions = {},
): Promise<{ ok: boolean; enabled: boolean; enabled_now: boolean; session_days: number }> {
  return apiRequest('/api/auth/password', { ...options, method: 'POST', body });
}

export function deletePassword(
  body: { current_password?: string } = {},
  options: CallOptions = {},
): Promise<{ ok: boolean; password_set: boolean; passkey_count: number; enabled: boolean }> {
  return apiRequest('/api/auth/password', { ...options, method: 'DELETE', body });
}

export function listPasskeys(
  options: CallOptions = {},
): Promise<{ passkeys: AuthPasskey[]; password_set: boolean }> {
  return apiRequest('/api/auth/passkeys', options);
}

/** Step one of registering a passkey: the challenge and the relying party. */
export function passkeyRegisterOptions(
  options: CallOptions = {},
): Promise<PasskeyRegistrationOptions> {
  return apiRequest('/api/auth/passkeys/options', { ...options, method: 'POST', body: {} });
}

/** Step two: hand the browser's attestation back for verification. */
export function registerPasskey(
  body: PasskeyRegistrationResult & { challenge_id: string; name: string },
  options: CallOptions = {},
): Promise<{ passkey: AuthPasskey; enabled: boolean; enabled_now: boolean }> {
  return apiRequest('/api/auth/passkeys', { ...options, method: 'POST', body });
}

export function renamePasskey(
  id: string,
  name: string,
  options: CallOptions = {},
): Promise<{ passkey: AuthPasskey }> {
  return apiRequest(`/api/auth/passkeys/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { name },
  });
}

export function deletePasskey(
  id: string,
  options: CallOptions = {},
): Promise<{ ok: boolean; passkey_count: number; enabled: boolean }> {
  return apiRequest(`/api/auth/passkeys/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
  });
}

/** Step one of signing in with a passkey: the challenge and the known handles. */
export function passkeyLoginOptions(options: CallOptions = {}): Promise<PasskeyRequestOptions> {
  return apiRequest('/api/auth/passkey/options', { ...options, method: 'POST', body: {} });
}

export function passkeyLogin(
  body: PasskeyAssertionResult & { challenge_id: string },
  options: CallOptions = {},
): Promise<{ ok: boolean; method: string; label: string; expires_at: number }> {
  return apiRequest('/api/auth/passkey/login', {
    ...options,
    method: 'POST',
    body,
    skipAuthHandler: true,
  });
}

export function listAuthSessions(
  options: CallOptions = {},
): Promise<{ sessions: AuthSession[]; current: string | null }> {
  return apiRequest('/api/auth/sessions', options);
}

export function revokeAuthSession(
  id: string,
  options: CallOptions = {},
): Promise<{ ok: boolean; removed: boolean }> {
  return apiRequest(`/api/auth/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
  });
}

export function revokeAllAuthSessions(
  options: CallOptions = {},
): Promise<{ ok: boolean; removed: number }> {
  return apiRequest('/api/auth/sessions/revoke-all', {
    ...options,
    method: 'POST',
    body: {},
  });
}

// ---------------------------------------------------------------------------
// Application API keys
// ---------------------------------------------------------------------------

export function listApiKeys(options: CallOptions = {}): Promise<ApiKeyListResponse> {
  return apiRequest('/api/auth/api-keys', options);
}

export function createApiKey(
  body: {
    name: string;
    scopes?: ApiKeyScope[];
    expires_in_days?: number;
    expires_at?: number;
  },
  adminToken?: string,
  options: CallOptions = {},
): Promise<ApiKeyIssued> {
  return apiRequest('/api/auth/api-keys', {
    ...options,
    method: 'POST',
    body,
    headers: adminHeaders(adminToken, options),
  });
}

export function updateApiKey(
  id: string,
  body: { name?: string; expires_in_days?: number; clear_expiry?: boolean },
  adminToken?: string,
  options: CallOptions = {},
): Promise<{ api_key: ApiKeySummary }> {
  return apiRequest(`/api/auth/api-keys/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body,
    headers: adminHeaders(adminToken, options),
  });
}

export function revokeApiKey(
  id: string,
  purge: boolean,
  adminToken?: string,
  options: CallOptions = {},
): Promise<{ ok: boolean; id: string; purged: boolean }> {
  return apiRequest(
    `/api/auth/api-keys/${encodeURIComponent(id)}${purge ? '?purge=true' : ''}`,
    {
      ...options,
      method: 'DELETE',
      headers: adminHeaders(adminToken, options),
    },
  );
}

export function rotateApiKey(
  id: string,
  adminToken?: string,
  options: CallOptions = {},
): Promise<ApiKeyIssued> {
  return apiRequest(`/api/auth/api-keys/${encodeURIComponent(id)}/rotate`, {
    ...options,
    method: 'POST',
    body: {},
    headers: adminHeaders(adminToken, options),
  });
}

/** Whether the first-run wizard should run, and what it has to show. */
export function getOnboarding(options: CallOptions = {}): Promise<OnboardingState> {
  return apiRequest('/api/onboarding', options);
}

export function completeOnboarding(
  body: { skipped?: boolean } = {},
  options: CallOptions = {},
): Promise<{ ok: boolean; completed: boolean; skipped: boolean }> {
  return apiRequest('/api/onboarding/complete', { ...options, method: 'POST', body });
}

export type { AccessPatch };
