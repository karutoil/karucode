/** Which proxy sits in front of the server; it decides what forwarded headers count. */
export type TunnelKind = 'none' | 'cloudflare' | 'custom';

/**
 * `[access]` as `GET /api/config` reports it.
 *
 * Credentials are deliberately **not** here: a password hash and a passkey's
 * public key are served by `/api/auth/status` and `/api/auth/passkeys`, which
 * report only what the UI has to know (`password_set`, the passkey list).
 */
export interface AccessConfig {
  /** Require a password or passkey before `/api` answers. */
  enabled: boolean;
  /** `https://host[:port]` people type to reach this server, or `null`. */
  public_url: string | null;
  session_days: number;
  tunnel: TunnelKind;
  /** Extra proxy IPs/CIDRs whose forwarded headers are believed. */
  trusted_proxies: string[];
  /** Derived on the server: is any forwarded header read at all? */
  trusts_forwarded_headers: boolean;
  /** Derived: are Cloudflare's published ranges trusted? */
  cloudflare_ranges: boolean;
  /** `public_url` without its scheme, for display. */
  public_authority: string | null;
}

export interface AccessPatch {
  enabled?: boolean;
  /** An empty string clears it (meaning "use the browser's own origin"). */
  public_url?: string;
  session_days?: number;
  tunnel?: TunnelKind;
  trusted_proxies?: string[];
}

/** One registered passkey, as the settings screen lists it. */
export interface AuthPasskey {
  /** base64url credential id, also the handle used by PATCH/DELETE. */
  id: string;
  name: string;
  created_at: number;
  last_used_at: number | null;
}

/** One signed-in browser. `id` is a short, non-reversible handle. */
export interface AuthSession {
  id: string;
  created_at: number;
  expires_at: number;
  method: 'password' | 'passkey';
  label: string;
}

/** What an application API key may do. `write` implies `read`. */
export type ApiKeyScope = 'read' | 'write' | 'admin';

/** `active`, `expired` or `revoked`; the server decides from the timestamps. */
export type ApiKeyStatus = 'active' | 'expired' | 'revoked';

/**
 * One application API key, as `GET /api/auth/api-keys` lists it.
 *
 * The secret is never here: `prefix` is the leading `kck_<id>` and `last4` the
 * tail of the secret, which is enough to match a key by eye.
 */
export interface ApiKeySummary {
  id: string;
  name: string;
  prefix: string;
  last4: string;
  scopes: ApiKeyScope[];
  created_at: number;
  expires_at: number | null;
  revoked_at: number | null;
  last_used_at: number | null;
  last_used_ip: string | null;
  request_count: number;
  status: ApiKeyStatus;
}

/** The create/rotate answer: metadata plus the one and only copy of the token. */
export interface ApiKeyIssued {
  api_key: ApiKeySummary;
  token: string;
  /** Present when the key never expires. */
  warning: string | null;
}

export interface ApiKeyListResponse {
  keys: ApiKeySummary[];
  max_keys: number;
  max_lifetime_days: number;
  /** False on an open server, where creating a key is refused. */
  sign_in_required: boolean;
}

/**
 * `GET /api/auth/status`: everything the sign-in screen and the onboarding
 * wizard need, and the only auth payload available without a credential.
 */
export interface AuthStatus {
  enabled: boolean;
  sign_in_required: boolean;
  password_set: boolean;
  passkey_count: number;
  /** No credential exists yet: the UI has to offer "create", not "sign in". */
  needs_setup: boolean;
  signed_in: boolean;
  signed_in_with: string | null;
  token_required: boolean;
  session_days: number;
  public_url: string | null;
  tunnel: TunnelKind;
  trusted_proxies: string[];
  cloudflare_proxies: boolean;
  /** The origin this request is on, and the relying party a passkey would bind to. */
  origin: string | null;
  rp_id: string | null;
  min_password_len: number;
  max_password_len: number;
  passkey_challenge_ttl_secs: number;
}

/** What a passkey ceremony needs from the server before the browser can run it. */
export interface PasskeyRequestOptions {
  challenge_id: string;
  challenge: string;
  rp_id: string;
  timeout_ms: number;
  allow: { id: string; type: 'public-key' }[];
}

export interface PasskeyRegistrationOptions {
  challenge_id: string;
  challenge: string;
  rp: { id: string; name: string };
  user: { id: string; name: string; displayName: string };
  timeout_ms: number;
  exclude: { id: string; type: 'public-key' }[];
}

/** The browser's answer to `navigator.credentials.create`. */
export interface PasskeyRegistrationResult {
  credential_id: string;
  client_data_json: string;
  attestation_object: string;
}

/** The browser's answer to `navigator.credentials.get`. */
export interface PasskeyAssertionResult {
  credential_id: string;
  client_data_json: string;
  authenticator_data: string;
  signature: string;
}

/** `GET /api/onboarding`: whether the first-run wizard should run. */
export interface OnboardingState {
  needed: boolean;
  completed: boolean;
  provider_configured: boolean;
  has_sessions: boolean;
  project_count: number;
  has_credential: boolean;
  access: AccessSettingsSummary;
  workspace: string | null;
  version: string;
}

/** The Access tab's own payload (part of the onboarding state too). */
export interface AccessSettingsSummary {
  enabled: boolean;
  password_set: boolean;
  passkeys: AuthPasskey[];
  sessions: AuthSession[];
  public_url: string | null;
  session_days: number;
  tunnel: TunnelKind;
  trusted_proxies: string[];
  trusts_forwarded_headers: boolean;
  cloudflare_ranges: boolean;
  token_required: boolean;
  settings_locked: boolean;
}
