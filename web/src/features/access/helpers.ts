import type { AccessSettingsSummary, ApiKeyScope, TunnelKind } from '../../api/types';

/**
 * The three tunnel kinds, worded exactly as the onboarding wizard words them
 * (`TUNNELS` in `Onboarding.tsx`).
 *
 * Duplicated rather than imported: the wizard does not export its copy, and the
 * rule for this screen is that the two must read the same. If one changes, both
 * change.
 */
export const TUNNELS: { value: TunnelKind; label: string; description: string }[] = [
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

/** What the server's `MIN_PASSWORD_LEN` is, as a fallback before status lands. */
export const MIN_PASSWORD_FALLBACK = 10;

/** Bounds for `access.session_days`, mirroring `MIN/MAX_SESSION_DAYS` in kc-core. */
export const MIN_SESSION_DAYS = 1;
export const MAX_SESSION_DAYS = 365;

/** Longest key lifetime the server accepts, mirroring `MAX_LIFETIME_DAYS`. */
export const MAX_KEY_LIFETIME_DAYS = 3650;

/**
 * The three API-key scopes, worded the way `crates/kc-server/src/api_keys.rs`
 * defines them: `write` already includes `read`, and `admin` includes both.
 */
export const API_KEY_SCOPES: { value: ApiKeyScope; label: string; description: string }[] = [
  {
    value: 'read',
    label: 'Read',
    description: 'GET and HEAD on the API and /metrics.',
  },
  {
    value: 'write',
    label: 'Write',
    description: 'Reads plus ordinary mutations: sessions, messages, projects, files, git.',
  },
  {
    value: 'admin',
    label: 'Admin',
    description:
      'Everything, including key management, terminals, approvals, config and plugin reloads.',
  },
];

/** What a new key gets unless the user chooses otherwise. */
export const DEFAULT_KEY_SCOPES: ApiKeyScope[] = ['read', 'write'];

/** One inline result: `ok` decides whether it is a status or an alert. */
export interface Message {
  ok: boolean;
  text: string;
}

export interface PasswordErrors {
  current?: string;
  next?: string;
  confirm?: string;
}

export interface AddressErrors {
  url?: string;
  days?: string;
}

export interface IssuedKey {
  id: string;
  name: string;
  token: string;
  warning: string | null;
}

/**
 * The `[access]` fields this screen edits.
 *
 * Numbers stay strings while they are being typed - a `session_days` of `""` is a
 * half-typed value, not a zero - and are parsed on save.
 */
export interface PolicyDraft {
  enabled: boolean;
  publicUrl: string;
  sessionDays: string;
  tunnel: TunnelKind;
  proxies: string;
}

/** The editable copy of the server's policy; `null` re-derives it. */
export function draftFrom(summary: AccessSettingsSummary | null): PolicyDraft {
  return {
    enabled: summary?.enabled ?? false,
    publicUrl: summary?.public_url ?? '',
    sessionDays: String(summary?.session_days ?? 30),
    tunnel: summary?.tunnel ?? 'none',
    proxies: (summary?.trusted_proxies ?? []).join('\n'),
  };
}

/**
 * A readable default label for a passkey, from what the browser will admit to.
 *
 * Mirrors `defaultPasskeyName` in `Onboarding.tsx`: a passkey added from Settings
 * should be as recognisable in the list as one added during setup.
 */
export function newPasskeyName(): string {
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

/** One "IP or CIDR per line" field, split the way the server expects it. */
export function parseProxies(text: string): string[] {
  return text
    .split(/[\n,]+/)
    .map((entry) => entry.trim())
    .filter((entry) => entry !== '');
}
