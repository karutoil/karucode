/**
 * The Access tab of the settings sheet: who may reach this server, and how.
 *
 * Five things live here and each one owns its own save, because they are five
 * different mechanisms rather than five fields of one form:
 *
 * * **sign-in on/off** (`PATCH /api/config` `access.enabled`) - a policy switch,
 *   persisted the moment it is flipped. The server refuses to turn it on when no
 *   credential exists, so the refusal is what the hint beside it warns about.
 * * **the password** (`/api/auth/password`) - set, change or remove. Creating the
 *   first credential turns sign-in on server-side; the answer says whether it did.
 * * **passkeys** (`/api/auth/passkeys`) - a WebAuthn ceremony per registration,
 *   then a rename or a remove.
 * * **signed-in browsers** (`/api/auth/sessions`) - the one list that can be
 *   revoked from here, including "sign out everywhere".
 * * **address, tunnel and trusted proxies** (`PATCH /api/config` `access`) - the
 *   policy that decides whether a forwarded header is believed at all.
 *
 * This is the same ground the first-run wizard's access step covers, and the copy
 * deliberately matches `Onboarding.tsx` so the two screens cannot disagree about
 * what a tunnel means. Unlike the rest of the sheet it is **not** part of the
 * parent form's save/dirty machinery: the sheet's Save button patches the model,
 * limits and approvals sections, and this component never calls it.
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import * as api from '../api/endpoints';
import type {
  AccessSettingsSummary,
  ApiKeyScope,
  ApiKeySummary,
  AuthSession,
  TunnelKind,
} from '../api/types';
import { copyText } from '../lib/clipboard';
import { absoluteTime, fromUnixSeconds, relativeTime } from '../lib/format';
import { createPasskey, PasskeyError, passkeySupported } from '../lib/passkey';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { AlertIcon, CheckIcon, KeyIcon } from './icons';
import { Toggle } from './Toggle';

/**
 * The three tunnel kinds, worded exactly as the onboarding wizard words them
 * (`TUNNELS` in `Onboarding.tsx`).
 *
 * Duplicated rather than imported: the wizard does not export its copy, and the
 * rule for this screen is that the two must read the same. If one changes, both
 * change.
 */
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

/** What the server's `MIN_PASSWORD_LEN` is, as a fallback before status lands. */
const MIN_PASSWORD_FALLBACK = 10;

/** Bounds for `access.session_days`, mirroring `MIN/MAX_SESSION_DAYS` in kc-core. */
const MIN_SESSION_DAYS = 1;
const MAX_SESSION_DAYS = 365;

/** Longest key lifetime the server accepts, mirroring `MAX_LIFETIME_DAYS`. */
const MAX_KEY_LIFETIME_DAYS = 3650;

/**
 * The three API-key scopes, worded the way `crates/kc-server/src/api_keys.rs`
 * defines them: `write` already includes `read`, and `admin` includes both.
 */
const API_KEY_SCOPES: { value: ApiKeyScope; label: string; description: string }[] = [
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
const DEFAULT_KEY_SCOPES: ApiKeyScope[] = ['read', 'write'];

/** One inline result: `ok` decides whether it is a status or an alert. */
interface Message {
  ok: boolean;
  text: string;
}

interface PasswordErrors {
  current?: string;
  next?: string;
  confirm?: string;
}

interface AddressErrors {
  url?: string;
  days?: string;
}

/**
 * The `[access]` fields this screen edits.
 *
 * Numbers stay strings while they are being typed - a `session_days` of `""` is a
 * half-typed value, not a zero - and are parsed on save.
 */
interface PolicyDraft {
  enabled: boolean;
  publicUrl: string;
  sessionDays: string;
  tunnel: TunnelKind;
  proxies: string;
}

/** The editable copy of the server's policy; `null` re-derives it. */
function draftFrom(summary: AccessSettingsSummary | null): PolicyDraft {
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
function newPasskeyName(): string {
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
function parseProxies(text: string): string[] {
  return text
    .split(/[\n,]+/)
    .map((entry) => entry.trim())
    .filter((entry) => entry !== '');
}

export function AccessSettings(): JSX.Element {
  const [settings, setSettings] = useState<AccessSettingsSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);

  /**
   * The policy on screen. `null` means "whatever the server last said"; an edit
   * writes a draft and keeps it, and a successful save writes back the normalised
   * values rather than re-deriving (a re-read that failed must not silently put
   * the old address back in the box).
   */
  const [draft, setDraft] = useState<PolicyDraft | null>(null);

  const [sessions, setSessions] = useState<AuthSession[]>([]);
  const [currentSession, setCurrentSession] = useState<string | null>(null);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const [sessionBusy, setSessionBusy] = useState<string | null>(null);

  // API keys. `issuedKey` is the only place a secret ever lives in this
  // component: the server shows it once, and dismissing the panel is permanent.
  const [apiKeys, setApiKeys] = useState<ApiKeySummary[]>([]);
  const [apiKeysError, setApiKeysError] = useState<string | null>(null);
  const [keySignInRequired, setKeySignInRequired] = useState(true);
  const [keyMax, setKeyMax] = useState(100);
  const [keyBusy, setKeyBusy] = useState<string | null>(null);
  const [keyMessage, setKeyMessage] = useState<Message | null>(null);
  const [issuedKey, setIssuedKey] = useState<{
    id: string;
    name: string;
    token: string;
    warning: string | null;
  } | null>(null);
  const [newKeyName, setNewKeyName] = useState('');
  const [newKeyScopes, setNewKeyScopes] = useState<ApiKeyScope[]>([...DEFAULT_KEY_SCOPES]);
  const [newKeyExpiry, setNewKeyExpiry] = useState('30');
  const [adminToken, setAdminToken] = useState('');
  const [confirmRevokeKey, setConfirmRevokeKey] = useState<string | null>(null);
  const [renamingKey, setRenamingKey] = useState<{ id: string; name: string } | null>(null);
  const [copiedKey, setCopiedKey] = useState(false);

  const [enabledBusy, setEnabledBusy] = useState(false);
  const [signInMessage, setSignInMessage] = useState<Message | null>(null);

  const [passwordForm, setPasswordForm] = useState({ current: '', next: '', confirm: '' });
  const [passwordErrors, setPasswordErrors] = useState<PasswordErrors>({});
  const [passwordMessage, setPasswordMessage] = useState<Message | null>(null);
  const [passwordBusy, setPasswordBusy] = useState(false);
  const [removingPassword, setRemovingPassword] = useState(false);
  const [removePasswordValue, setRemovePasswordValue] = useState('');
  const [removeBusy, setRemoveBusy] = useState(false);

  const [passkeyBusy, setPasskeyBusy] = useState(false);
  const [passkeyMessage, setPasskeyMessage] = useState<Message | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [confirmRemovePasskey, setConfirmRemovePasskey] = useState<string | null>(null);

  const [addressErrors, setAddressErrors] = useState<AddressErrors>({});
  const [addressMessage, setAddressMessage] = useState<Message | null>(null);
  const [savingAddress, setSavingAddress] = useState(false);

  /**
   * Read everything this tab shows.
   *
   * `reloadToken` re-runs it, which is what the retry button bumps: an inline
   * closure in the effect body would have to be repeated for the button, and the
   * two copies would drift.
   */
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setLoadError(null);
    void (async () => {
      const summary = await actions.refreshAccessSettings();
      if (cancelled) return;
      if (summary === null) {
        setLoadError('Could not read this server’s sign-in settings.');
        setLoading(false);
        return;
      }
      setSettings(summary);
      setLoading(false);
      // The minimum password length and the passkey story come from
      // `/api/auth/status`, which the settings sheet does not otherwise read.
      void actions.refreshAuthStatus();
      try {
        const list = await api.listAuthSessions();
        if (cancelled) return;
        setSessions(list.sessions);
        setCurrentSession(list.current);
      } catch (error) {
        if (!cancelled) setSessionsError(toErrorMessage(error));
      }
      try {
        const list = await api.listApiKeys();
        if (cancelled) return;
        setApiKeys(list.keys);
        setKeySignInRequired(list.sign_in_required);
        setKeyMax(list.max_keys);
        setApiKeysError(null);
      } catch (error) {
        if (!cancelled) setApiKeysError(toErrorMessage(error));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [reloadToken]);

  const policy = draft ?? draftFrom(settings);
  const passwordSet = settings?.password_set ?? false;
  const passkeys = settings?.passkeys ?? [];
  const hasCredential = passwordSet || passkeys.length > 0;
  const locked = settings?.settings_locked ?? false;
  // `null` before the status lands (or on a server too old to have the route):
  // 10 is the server's own floor, so the message is never a stricter guess.
  const minPasswordLen = store.authStatus.value?.min_password_len ?? MIN_PASSWORD_FALLBACK;
  const supported = passkeySupported();

  const update = (patch: Partial<PolicyDraft>): void => {
    setDraft({ ...policy, ...patch });
  };

  /** Re-read the policy half of the summary without disturbing the draft. */
  const reloadPolicy = async (): Promise<void> => {
    const summary = await actions.refreshAccessSettings();
    if (summary !== null) setSettings(summary);
  };

  const refreshSessions = async (): Promise<void> => {
    try {
      const list = await api.listAuthSessions();
      setSessions(list.sessions);
      setCurrentSession(list.current);
      setSessionsError(null);
    } catch (error) {
      setSessionsError(toErrorMessage(error));
    }
  };

  /** Re-read the key list (and whether this server may mint one at all). */
  const refreshApiKeys = async (): Promise<void> => {
    try {
      const list = await api.listApiKeys();
      setApiKeys(list.keys);
      setKeySignInRequired(list.sign_in_required);
      setKeyMax(list.max_keys);
      setApiKeysError(null);
    } catch (error) {
      setApiKeysError(toErrorMessage(error));
    }
  };

  // ---- sign-in ----------------------------------------------------------

  /**
   * Flip the sign-in requirement.
   *
   * This one saves on the click rather than waiting for the Save button: it is a
   * security decision the user just made explicitly, and a switch that only takes
   * effect when they later remember to press something else is a trap.
   */
  const setEnabled = async (next: boolean): Promise<void> => {
    if (enabledBusy) return;
    const previous = policy.enabled;
    setEnabledBusy(true);
    setSignInMessage(null);
    setDraft({ ...policy, enabled: next });
    try {
      await actions.applyConfig({ access: { enabled: next } });
      setSettings((current) => (current ? { ...current, enabled: next } : current));
      setSignInMessage({
        ok: true,
        text: next
          ? 'Sign-in is now required. Every browser must present a password or passkey.'
          : 'Sign-in is off. Anyone who can reach this port can use this server.',
      });
    } catch (error) {
      // The server refused, so it is still what it was: put the switch back
      // instead of leaving the screen claiming a policy that is not in force.
      setDraft({ ...policy, enabled: previous });
      setSignInMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setEnabledBusy(false);
    }
  };

  // ---- password ---------------------------------------------------------

  const submitPassword = async (): Promise<void> => {
    if (passwordBusy) return;
    const errors: PasswordErrors = {};
    if (passwordSet && passwordForm.current === '') {
      errors.current = 'Enter the current password.';
    }
    if (passwordForm.next.length < minPasswordLen) {
      errors.next = `Use at least ${minPasswordLen} characters.`;
    } else if (passwordForm.next !== passwordForm.confirm) {
      errors.confirm = 'The two passwords do not match.';
    }
    setPasswordErrors(errors);
    if (Object.keys(errors).length > 0) {
      setPasswordMessage(null);
      return;
    }
    setPasswordBusy(true);
    setPasswordMessage(null);
    try {
      const result = await api.setPassword(
        passwordSet
          ? { password: passwordForm.next, current_password: passwordForm.current }
          : { password: passwordForm.next },
      );
      setPasswordForm({ current: '', next: '', confirm: '' });
      setPasswordErrors({});
      await reloadPolicy();
      await refreshSessions();
      await refreshApiKeys();
      setPasswordMessage({
        ok: true,
        text: result.enabled_now
          ? 'Password saved. Sign-in is now required, and this browser is already signed in.'
          : 'Password saved. This browser is already signed in.',
      });
    } catch (error) {
      setPasswordMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setPasswordBusy(false);
    }
  };

  const removePassword = async (): Promise<void> => {
    if (removeBusy) return;
    if (removePasswordValue === '') {
      setPasswordMessage({ ok: false, text: 'Enter the current password to remove it.' });
      return;
    }
    setRemoveBusy(true);
    setPasswordMessage(null);
    try {
      const result = await api.deletePassword({ current_password: removePasswordValue });
      setRemovePasswordValue('');
      setRemovingPassword(false);
      await reloadPolicy();
      await refreshSessions();
      await refreshApiKeys();
      setPasswordMessage({
        ok: true,
        text:
          result.passkey_count > 0
            ? 'Password removed. Your passkeys still protect this server.'
            : 'Password removed. Sign-in is off — nothing is required to reach this server.',
      });
    } catch (error) {
      setPasswordMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setRemoveBusy(false);
    }
  };

  // ---- passkeys ---------------------------------------------------------

  const addPasskey = async (): Promise<void> => {
    if (passkeyBusy) return;
    setPasskeyBusy(true);
    setPasskeyMessage(null);
    try {
      const options = await api.passkeyRegisterOptions();
      const registration = await createPasskey(options);
      const result = await api.registerPasskey({
        ...registration,
        challenge_id: options.challenge_id,
        name: newPasskeyName(),
      });
      await reloadPolicy();
      await refreshApiKeys();
      setPasskeyMessage({
        ok: true,
        text: result.enabled_now
          ? `Passkey “${result.passkey.name}” added. Sign-in is now required.`
          : `Passkey “${result.passkey.name}” added.`,
      });
    } catch (failure) {
      // A `PasskeyError` already carries a sentence a person can act on (the
      // prompt was dismissed, the page is not a secure context, …).
      setPasskeyMessage({
        ok: false,
        text: failure instanceof PasskeyError ? failure.message : toErrorMessage(failure),
      });
    } finally {
      setPasskeyBusy(false);
    }
  };

  const saveRename = async (id: string, name: string): Promise<void> => {
    const trimmed = name.trim();
    if (trimmed === '') {
      setPasskeyMessage({ ok: false, text: 'A passkey needs a name.' });
      return;
    }
    setPasskeyBusy(true);
    setPasskeyMessage(null);
    try {
      await api.renamePasskey(id, trimmed);
      setRenaming(null);
      await reloadPolicy();
      setPasskeyMessage({ ok: true, text: `Passkey renamed to “${trimmed}”.` });
    } catch (error) {
      setPasskeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setPasskeyBusy(false);
    }
  };

  const removePasskey = async (id: string): Promise<void> => {
    setPasskeyBusy(true);
    setPasskeyMessage(null);
    try {
      const result = await api.deletePasskey(id);
      setConfirmRemovePasskey(null);
      await reloadPolicy();
      await refreshApiKeys();
      setPasskeyMessage({
        ok: true,
        text: result.enabled
          ? 'Passkey removed.'
          : 'Passkey removed. With no credential left, sign-in is now off.',
      });
    } catch (error) {
      setPasskeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setPasskeyBusy(false);
    }
  };

  // ---- sessions ---------------------------------------------------------

  const signOutSession = async (session: AuthSession): Promise<void> => {
    if (session.id === currentSession) {
      // This browser's own row is the ordinary logout: revoking it first would
      // 401 every request in between.
      await actions.signOut();
      return;
    }
    setSessionBusy(session.id);
    setSessionsError(null);
    try {
      await api.revokeAuthSession(session.id);
      await refreshSessions();
    } catch (error) {
      setSessionsError(toErrorMessage(error));
    } finally {
      setSessionBusy(null);
    }
  };

  const signOutEverywhere = async (): Promise<void> => {
    if (sessionBusy !== null) return;
    setSessionBusy('all');
    setSessionsError(null);
    try {
      await api.revokeAllAuthSessions();
    } catch (error) {
      setSessionsError(toErrorMessage(error));
      setSessionBusy(null);
      return;
    }
    // Every session is gone, this one included, so the sign-in screen is the only
    // honest next thing to show.
    await actions.signOut();
  };

  // ---- API keys ---------------------------------------------------------

  /** Add or remove one scope from the draft key. */
  const toggleKeyScope = (scope: ApiKeyScope): void => {
    setNewKeyScopes((current) =>
      current.includes(scope)
        ? current.filter((entry) => entry !== scope)
        : [...current, scope],
    );
  };

  /**
   * Create a key and surface the one copy of its token.
   *
   * The expiry is a whole number of days or empty (never expires). The server is
   * the authority on both bounds; the checks here exist so a typo is answered
   * before a round trip, not so the form can disagree with it.
   */
  const createKey = async (): Promise<void> => {
    if (keyBusy !== null) return;
    const name = newKeyName.trim();
    if (name === '') {
      setKeyMessage({ ok: false, text: 'Give the key a name so you can recognise it later.' });
      return;
    }
    const rawDays = newKeyExpiry.trim();
    const days = rawDays === '' ? undefined : Number(rawDays);
    if (
      days !== undefined &&
      (!Number.isInteger(days) || days < 1 || days > MAX_KEY_LIFETIME_DAYS)
    ) {
      setKeyMessage({
        ok: false,
        text: `Use a whole number of days from 1 to ${MAX_KEY_LIFETIME_DAYS}, or leave it empty for no expiry.`,
      });
      return;
    }
    if (newKeyScopes.length === 0) {
      setKeyMessage({ ok: false, text: 'Choose at least one scope for the key.' });
      return;
    }
    setKeyBusy('create');
    setKeyMessage(null);
    setCopiedKey(false);
    try {
      const issued = await api.createApiKey(
        {
          name,
          scopes: newKeyScopes,
          ...(days !== undefined ? { expires_in_days: days } : {}),
        },
        adminToken,
      );
      setIssuedKey({
        id: issued.api_key.id,
        name: issued.api_key.name,
        token: issued.token,
        warning: issued.warning,
      });
      setNewKeyName('');
      await refreshApiKeys();
      setKeyMessage({
        ok: true,
        text: `Key “${issued.api_key.name}” created. Copy the token now — it is not shown again.`,
      });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  const copyIssuedKey = async (): Promise<void> => {
    if (issuedKey === null) return;
    const copied = await copyText(issuedKey.token);
    setCopiedKey(copied);
    setKeyMessage(
      copied
        ? { ok: true, text: 'Token copied to the clipboard.' }
        : { ok: false, text: 'The browser refused to copy it; select the token and copy it by hand.' },
    );
  };

  const rotateKey = async (key: ApiKeySummary): Promise<void> => {
    if (keyBusy !== null) return;
    setKeyBusy(key.id);
    setKeyMessage(null);
    setCopiedKey(false);
    try {
      const issued = await api.rotateApiKey(key.id, adminToken);
      setIssuedKey({
        id: issued.api_key.id,
        name: issued.api_key.name,
        token: issued.token,
        warning: issued.warning,
      });
      await refreshApiKeys();
      setKeyMessage({
        ok: true,
        text: `Key “${issued.api_key.name}” rotated. The old secret stopped working; copy the new one now.`,
      });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  const revokeKey = async (key: ApiKeySummary): Promise<void> => {
    if (keyBusy !== null) return;
    setKeyBusy(key.id);
    setKeyMessage(null);
    try {
      await api.revokeApiKey(key.id, false, adminToken);
      setConfirmRevokeKey(null);
      // A revoked key's token is dead; do not leave it on screen as if it were
      // still usable.
      if (issuedKey !== null && issuedKey.id === key.id) {
        setIssuedKey(null);
        setCopiedKey(false);
      }
      await refreshApiKeys();
      setKeyMessage({
        ok: true,
        text: `Key “${key.name}” revoked. It cannot authenticate again.`,
      });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  /** Remove a revoked or expired key's row for good. */
  const purgeKey = async (key: ApiKeySummary): Promise<void> => {
    if (keyBusy !== null) return;
    setKeyBusy(key.id);
    setKeyMessage(null);
    try {
      await api.revokeApiKey(key.id, true, adminToken);
      if (issuedKey !== null && issuedKey.id === key.id) {
        setIssuedKey(null);
        setCopiedKey(false);
      }
      await refreshApiKeys();
      setKeyMessage({ ok: true, text: `Key “${key.name}” removed from the list.` });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  const saveKeyRename = async (id: string, name: string): Promise<void> => {
    const trimmed = name.trim();
    if (trimmed === '') {
      setKeyMessage({ ok: false, text: 'A key needs a name.' });
      return;
    }
    if (keyBusy !== null) return;
    setKeyBusy(id);
    setKeyMessage(null);
    try {
      await api.updateApiKey(id, { name: trimmed }, adminToken);
      setRenamingKey(null);
      await refreshApiKeys();
      setKeyMessage({ ok: true, text: `Key renamed to “${trimmed}”.` });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  // ---- address and tunnel -----------------------------------------------

  const savePolicy = async (): Promise<void> => {
    if (savingAddress) return;
    const url = policy.publicUrl.trim();
    const days = Number(policy.sessionDays.trim());
    const errors: AddressErrors = {};
    if (url !== '' && !/^https?:\/\/[^\s]+$/i.test(url)) {
      errors.url = 'Use a full address such as https://karucode.example.com, or leave it empty.';
    }
    if (
      policy.sessionDays.trim() === '' ||
      !Number.isInteger(days) ||
      days < MIN_SESSION_DAYS ||
      days > MAX_SESSION_DAYS
    ) {
      errors.days = `Use a whole number of days between ${MIN_SESSION_DAYS} and ${MAX_SESSION_DAYS}.`;
    }
    setAddressErrors(errors);
    if (Object.keys(errors).length > 0) {
      setAddressMessage(null);
      return;
    }
    const proxies = parseProxies(policy.proxies);
    setSavingAddress(true);
    setAddressMessage(null);
    try {
      await actions.applyConfig({
        access: {
          enabled: policy.enabled,
          public_url: url,
          session_days: days,
          tunnel: policy.tunnel,
          trusted_proxies: proxies,
        },
      });
      // Show exactly what was sent: the server trims the URL and drops empty
      // proxy lines, and a re-read that failed must not put the old values back.
      setDraft({ ...policy, publicUrl: url, sessionDays: String(days), proxies: proxies.join('\n') });
      await reloadPolicy();
      setAddressMessage({ ok: true, text: 'Access settings saved.' });
    } catch (error) {
      setAddressMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setSavingAddress(false);
    }
  };

  // ---- render -----------------------------------------------------------

  if (loading && settings === null) {
    return (
      <section class="settings-section" aria-labelledby="settings-access-loading">
        <div class="settings-section-head">
          <h3 id="settings-access-loading">Access</h3>
          <span class="settings-section-note">Reading the sign-in policy…</span>
        </div>
        <p class="field-hint" role="status">
          Fetching the password, passkey and signed-in-browser state from this server.
        </p>
      </section>
    );
  }

  if (loadError !== null && settings === null) {
    return (
      <section class="settings-section" aria-labelledby="settings-access-loading">
        <div class="settings-section-head">
          <h3 id="settings-access-loading">Access</h3>
          <span class="settings-section-note">Nothing could be read</span>
        </div>
        <p class="field-error" role="alert">
          {loadError}
        </p>
        <div class="access-actions">
          <button
            type="button"
            class="button button-small"
            onClick={() => setReloadToken((token) => token + 1)}
          >
            Try again
          </button>
        </div>
      </section>
    );
  }

  return (
    <>
      <section class="settings-section" aria-labelledby="settings-access-signin">
        <div class="settings-section-head">
          <h3 id="settings-access-signin">Sign-in</h3>
          <span class="settings-section-note">Who may use this web UI at all</span>
        </div>

        <Toggle
          id="access-enabled"
          checked={policy.enabled}
          disabled={locked || enabledBusy}
          label="Require a password or passkey"
          description="A credential before the API answers. Off means anything that can reach this port can use the agent, its files and its shell."
          onChange={(next) => void setEnabled(next)}
        />

        {!policy.enabled && hasCredential ? (
          <div class="access-warning">
            <AlertIcon size={15} />
            <span>
              Sign-in is off while a credential exists: this server is open to anyone who can reach
              the port. Turn the switch on to require it.
            </span>
          </div>
        ) : null}

        {!hasCredential ? (
          <p class="field-hint">
            Turning this on needs a credential first — the server refuses it otherwise, so that
            nobody can lock themselves out. Set a password or add a passkey below, then come back.
          </p>
        ) : null}

        {signInMessage ? (
          <p
            class={signInMessage.ok ? 'access-saved' : 'field-error'}
            role={signInMessage.ok ? 'status' : 'alert'}
          >
            {signInMessage.ok ? <CheckIcon size={13} /> : null}
            {signInMessage.text}
          </p>
        ) : null}
      </section>

      <section class="settings-section" aria-labelledby="settings-access-password">
        <div class="settings-section-head">
          <h3 id="settings-access-password">Password</h3>
          <span class="settings-section-note">
            {passwordSet ? 'Changing or removing it needs the current one' : 'Nothing to sign in with yet'}
          </span>
        </div>

        <div class="settings-subhead">
          <h4 id="access-password-heading">{passwordSet ? 'Change password' : 'Set a password'}</h4>
          <span class="settings-section-note">
            {passwordSet
              ? 'Knowing the current password is what stops a hijacked browser from locking you out.'
              : `At least ${minPasswordLen} characters. Setting one turns sign-in on.`}
          </span>
        </div>

        {/*
         * Not a `<form>`: this whole tab is already inside the sheet's form, and
         * a form inside a form is not something the sheet does anywhere else. The
         * Enter key is handled here instead, so pressing it in a password box
         * saves the password and never submits the parent's settings form.
         */}
        <div
          class="access-form"
          role="group"
          aria-labelledby="access-password-heading"
          onKeyDown={(event) => {
            if (event.key !== 'Enter') return;
            const target = event.target;
            if (!(target instanceof HTMLInputElement)) return;
            event.preventDefault();
            void submitPassword();
          }}
        >
          {passwordSet ? (
            <div class="field">
              <label class="field-label" for="access-password-current">
                Current password
              </label>
              <input
                id="access-password-current"
                class="input"
                type="password"
                autoComplete="current-password"
                value={passwordForm.current}
                disabled={passwordBusy}
                aria-invalid={passwordErrors.current ? 'true' : undefined}
                aria-describedby={
                  passwordErrors.current ? 'access-password-current-error' : undefined
                }
                onInput={(event) => {
                  // `currentTarget` is only valid during dispatch; a functional
                  // state update runs later, so the value is read now.
                  const value = event.currentTarget.value;
                  setPasswordForm((form) => ({ ...form, current: value }));
                }}
              />
              {passwordErrors.current ? (
                <p class="field-error" id="access-password-current-error" role="alert">
                  {passwordErrors.current}
                </p>
              ) : null}
            </div>
          ) : null}

          <div class="field">
            <label class="field-label" for="access-password-new">
              {passwordSet ? 'New password' : 'Password'}
            </label>
            <input
              id="access-password-new"
              class="input"
              type="password"
              autoComplete="new-password"
              value={passwordForm.next}
              disabled={passwordBusy}
              aria-invalid={passwordErrors.next ? 'true' : undefined}
              aria-describedby={
                passwordErrors.next ? 'access-password-new-error' : 'access-password-new-hint'
              }
              onInput={(event) => {
                const value = event.currentTarget.value;
                setPasswordForm((form) => ({ ...form, next: value }));
              }}
            />
            <p class="field-hint" id="access-password-new-hint">
              At least {minPasswordLen} characters.
            </p>
            {passwordErrors.next ? (
              <p class="field-error" id="access-password-new-error" role="alert">
                {passwordErrors.next}
              </p>
            ) : null}
          </div>

          <div class="field">
            <label class="field-label" for="access-password-confirm">
              Confirm password
            </label>
            <input
              id="access-password-confirm"
              class="input"
              type="password"
              autoComplete="new-password"
              value={passwordForm.confirm}
              disabled={passwordBusy}
              aria-invalid={passwordErrors.confirm ? 'true' : undefined}
              aria-describedby={
                passwordErrors.confirm ? 'access-password-confirm-error' : undefined
              }
              onInput={(event) => {
                const value = event.currentTarget.value;
                setPasswordForm((form) => ({ ...form, confirm: value }));
              }}
            />
            {passwordErrors.confirm ? (
              <p class="field-error" id="access-password-confirm-error" role="alert">
                {passwordErrors.confirm}
              </p>
            ) : null}
          </div>

          <div class="access-actions">
            <button
              type="button"
              class="button button-small"
              disabled={passwordBusy}
              onClick={() => void submitPassword()}
            >
              {passwordBusy
                ? 'Saving…'
                : passwordSet
                  ? 'Save new password'
                  : 'Set password'}
            </button>
          </div>
        </div>

        {passwordSet ? (
          <div class="access-danger">
            <div class="setting-copy">
              <span class="setting-name">Remove the password</span>
              <span class="setting-hint">
                The current password is required. Your passkeys keep working; the last credential
                removed also turns sign-in off.
              </span>
            </div>
            {removingPassword ? (
              <div class="access-inline-form">
                <label class="field-label" for="access-password-remove">
                  Current password to remove it
                </label>
                <div class="access-inline-row">
                  <input
                    id="access-password-remove"
                    class="input"
                    type="password"
                    autoComplete="current-password"
                    value={removePasswordValue}
                    disabled={removeBusy}
                    onInput={(event) => setRemovePasswordValue(event.currentTarget.value)}
                  />
                  <button
                    type="button"
                    class="button button-small button-danger-ghost"
                    disabled={removeBusy}
                    onClick={() => void removePassword()}
                  >
                    {removeBusy ? 'Removing…' : 'Yes, remove it'}
                  </button>
                  <button
                    type="button"
                    class="button button-small"
                    disabled={removeBusy}
                    onClick={() => {
                      setRemovingPassword(false);
                      setRemovePasswordValue('');
                    }}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            ) : (
              <button
                type="button"
                class="button button-small button-danger-ghost"
                onClick={() => {
                  setRemovingPassword(true);
                  setPasswordMessage(null);
                }}
              >
                Remove password
              </button>
            )}
          </div>
        ) : null}

        {passwordMessage ? (
          <p
            class={passwordMessage.ok ? 'access-saved' : 'field-error'}
            role={passwordMessage.ok ? 'status' : 'alert'}
          >
            {passwordMessage.ok ? <CheckIcon size={13} /> : null}
            {passwordMessage.text}
          </p>
        ) : null}
      </section>

      <section class="settings-section" aria-labelledby="settings-access-passkeys">
        <div class="settings-section-head">
          <h3 id="settings-access-passkeys">Passkeys</h3>
          <span class="settings-section-note">
            {passkeys.length === 0
              ? 'None registered'
              : `${passkeys.length} registered`}
          </span>
        </div>

        <p class="field-hint">
          A passkey signs in with the fingerprint, face, screen lock or security key on a device.
          Nothing secret leaves the device, and a passkey cannot be phished the way a password can.
        </p>

        {passkeys.length > 0 ? (
          <ul class="access-list" aria-label="Registered passkeys">
            {passkeys.map((passkey, index) => {
              const nameId = `access-passkey-name-${index}`;
              const isRenaming = renaming !== null && renaming.id === passkey.id;
              const isRemoving = confirmRemovePasskey === passkey.id;
              return (
                <li class="access-row" key={passkey.id}>
                  <div class="access-row-main">
                    <span class="access-row-title">{passkey.name}</span>
                    <span class="access-row-meta">
                      Added {absoluteTime(fromUnixSeconds(passkey.created_at))}
                      {passkey.last_used_at !== null && passkey.last_used_at > 0
                        ? ` · last used ${relativeTime(fromUnixSeconds(passkey.last_used_at))}`
                        : ' · never used'}
                    </span>
                  </div>

                  {isRenaming && renaming !== null ? (
                    <div class="access-inline-form">
                      <label class="field-label" for={nameId}>
                        Name for this passkey
                      </label>
                      <div class="access-inline-row">
                        <input
                          id={nameId}
                          class="input"
                          type="text"
                          value={renaming.name}
                          disabled={passkeyBusy}
                          onInput={(event) =>
                            setRenaming({ id: passkey.id, name: event.currentTarget.value })
                          }
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') {
                              event.preventDefault();
                              void saveRename(passkey.id, renaming.name);
                            }
                            if (event.key === 'Escape') setRenaming(null);
                          }}
                        />
                        <button
                          type="button"
                          class="button button-small"
                          disabled={passkeyBusy}
                          onClick={() => void saveRename(passkey.id, renaming.name)}
                        >
                          Save
                        </button>
                        <button
                          type="button"
                          class="button button-small"
                          disabled={passkeyBusy}
                          onClick={() => setRenaming(null)}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  ) : isRemoving ? (
                    <div class="access-inline-form">
                      <span class="field-hint">
                        Remove “{passkey.name}”? That device will no longer be able to sign in with
                        it.
                      </span>
                      <div class="access-inline-row">
                        <button
                          type="button"
                          class="button button-small button-danger-ghost"
                          disabled={passkeyBusy}
                          onClick={() => void removePasskey(passkey.id)}
                        >
                          {passkeyBusy ? 'Removing…' : `Remove “${passkey.name}”`}
                        </button>
                        <button
                          type="button"
                          class="button button-small"
                          disabled={passkeyBusy}
                          onClick={() => setConfirmRemovePasskey(null)}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div class="access-row-actions">
                      <button
                        type="button"
                        class="button button-small"
                        disabled={passkeyBusy}
                        aria-label={`Rename ${passkey.name}`}
                        onClick={() => {
                          setConfirmRemovePasskey(null);
                          setRenaming({ id: passkey.id, name: passkey.name });
                        }}
                      >
                        Rename
                      </button>
                      <button
                        type="button"
                        class="button button-small button-danger-ghost"
                        disabled={passkeyBusy}
                        aria-label={`Remove ${passkey.name}`}
                        onClick={() => {
                          setRenaming(null);
                          setConfirmRemovePasskey(passkey.id);
                        }}
                      >
                        Remove
                      </button>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        ) : (
          <p class="field-hint">No passkeys yet. Add one to sign in without typing a password.</p>
        )}

        <div class="access-actions">
          <button
            type="button"
            class="button button-small"
            disabled={passkeyBusy || !supported}
            onClick={() => void addPasskey()}
          >
            <KeyIcon size={14} />
            {passkeyBusy ? 'Waiting for your device…' : 'Add a passkey'}
          </button>
          <span class="field-hint">
            {supported
              ? 'Fingerprint, face, screen lock or a security key.'
              : 'This browser cannot create a passkey (no WebAuthn, or the page is not on HTTPS or localhost). Use a password instead.'}
          </span>
        </div>

        {passkeyMessage ? (
          <p
            class={passkeyMessage.ok ? 'access-saved' : 'field-error'}
            role={passkeyMessage.ok ? 'status' : 'alert'}
          >
            {passkeyMessage.ok ? <CheckIcon size={13} /> : null}
            {passkeyMessage.text}
          </p>
        ) : null}
      </section>

      <section class="settings-section" aria-labelledby="settings-access-sessions">
        <div class="settings-section-head">
          <h3 id="settings-access-sessions">Sessions</h3>
          <span class="settings-section-note">Browsers signed in right now</span>
        </div>

        <p class="field-hint">
          Each row is one browser holding a session cookie. Revoking one signs that browser out on
          its next request.
        </p>

        {sessions.length === 0 ? (
          <p class="field-hint">No browser is signed in.</p>
        ) : (
          <ul class="access-list" aria-label="Signed-in browsers">
            {sessions.map((session) => (
              <li class="access-row" key={session.id}>
                <div class="access-row-main">
                  <span class="access-row-title">
                    {session.label}
                    {session.id === currentSession ? (
                      <span class="access-tag">This browser</span>
                    ) : null}
                  </span>
                  <span class="access-row-meta">
                    Signed in with {session.method} · since {absoluteTime(fromUnixSeconds(session.created_at))} ·
                    expires {absoluteTime(fromUnixSeconds(session.expires_at))}
                  </span>
                </div>
                <div class="access-row-actions">
                  <button
                    type="button"
                    class="button button-small"
                    disabled={sessionBusy !== null}
                    aria-label={`Sign out ${session.label}`}
                    onClick={() => void signOutSession(session)}
                  >
                    Sign out
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}

        <div class="access-actions">
          <button
            type="button"
            class="button button-small button-danger-ghost"
            disabled={sessionBusy !== null}
            onClick={() => void signOutEverywhere()}
          >
            {sessionBusy === 'all' ? 'Signing out…' : 'Sign out everywhere'}
          </button>
          <span class="field-hint">
            Revokes every session, including this one — you will be asked to sign in again.
          </span>
        </div>

        {sessionsError ? (
          <p class="field-error" role="alert">
            {sessionsError}
          </p>
        ) : null}
      </section>

      <section class="settings-section" aria-labelledby="settings-access-api-keys">
        <div class="settings-section-head">
          <h3 id="settings-access-api-keys">API keys</h3>
          <span class="settings-section-note">
            {apiKeys.length === 0
              ? 'None created'
              : `${apiKeys.filter((key) => key.status === 'active').length} active`}
          </span>
        </div>

        <p class="field-hint">
          A key is a bearer credential for one application — a script, a CI job, a bot. It works
          like the auth token, but it is scoped, can expire, and can be rotated or revoked without
          changing your password. The full token is shown once, when it is created.
        </p>

        {!keySignInRequired ? (
          <div class="access-warning">
            <AlertIcon size={15} />
            <span>
              This server requires no credential yet, so a key would protect nothing and creation is
              refused. Set a password above, or <code>KARUCODE_AUTH_TOKEN</code>, first.
            </span>
          </div>
        ) : null}

        {issuedKey !== null ? (
          <div class="access-issued" role="group" aria-label={`Token for ${issuedKey.name}`}>
            <span class="setting-name">Copy this token now</span>
            <span class="field-hint">
              It is not stored in the browser and will not be shown again. If it is lost, rotate the
              key to get a new one.
            </span>
            <code class="access-key-token">{issuedKey.token}</code>
            {issuedKey.warning ? <span class="field-hint">{issuedKey.warning}</span> : null}
            <div class="access-inline-row">
              <button
                type="button"
                class="button button-small"
                onClick={() => void copyIssuedKey()}
              >
                {copiedKey ? 'Copied' : 'Copy token'}
              </button>
              <button
                type="button"
                class="button button-small"
                onClick={() => {
                  setIssuedKey(null);
                  setCopiedKey(false);
                }}
              >
                I have saved it
              </button>
            </div>
          </div>
        ) : null}

        {apiKeys.length === 0 ? (
          <p class="field-hint">No API keys yet.</p>
        ) : (
          <ul class="access-list" aria-label="API keys">
            {apiKeys.map((key) => {
              const nameId = `access-api-key-name-${key.id}`;
              const isRenaming = renamingKey !== null && renamingKey.id === key.id;
              const isRemoving = confirmRevokeKey === key.id;
              const dead = key.status !== 'active';
              return (
                <li class="access-row" key={key.id}>
                  <div class="access-row-main">
                    <span class="access-row-title">
                      {key.name}
                      {dead ? <span class="access-tag">{key.status}</span> : null}
                    </span>
                    <span class="access-row-meta">
                      {key.prefix}…{key.last4} · {key.scopes.join(' + ')} · created{' '}
                      {absoluteTime(fromUnixSeconds(key.created_at))} ·{' '}
                      {key.expires_at === null
                        ? 'never expires'
                        : key.revoked_at !== null
                          ? `revoked ${absoluteTime(fromUnixSeconds(key.revoked_at))}`
                          : `expires ${absoluteTime(fromUnixSeconds(key.expires_at))}`}{' '}
                      ·{' '}
                      {key.last_used_at === null
                        ? 'never used'
                        : `last used ${relativeTime(fromUnixSeconds(key.last_used_at))}`}
                      {key.request_count > 0
                        ? ` · ${key.request_count} request${key.request_count === 1 ? '' : 's'}`
                        : ''}
                    </span>
                  </div>

                  {isRenaming && renamingKey !== null ? (
                    <div class="access-inline-form">
                      <label class="field-label" for={nameId}>
                        Name for this key
                      </label>
                      <div class="access-inline-row">
                        <input
                          id={nameId}
                          class="input"
                          type="text"
                          value={renamingKey.name}
                          disabled={keyBusy !== null}
                          onInput={(event) =>
                            setRenamingKey({ id: key.id, name: event.currentTarget.value })
                          }
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') {
                              event.preventDefault();
                              void saveKeyRename(key.id, renamingKey.name);
                            }
                            if (event.key === 'Escape') setRenamingKey(null);
                          }}
                        />
                        <button
                          type="button"
                          class="button button-small"
                          disabled={keyBusy !== null}
                          onClick={() => void saveKeyRename(key.id, renamingKey.name)}
                        >
                          Save
                        </button>
                        <button
                          type="button"
                          class="button button-small"
                          disabled={keyBusy !== null}
                          onClick={() => setRenamingKey(null)}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  ) : isRemoving ? (
                    <div class="access-inline-form">
                      <span class="field-hint">
                        Revoke “{key.name}”? Anything using it stops working immediately; this
                        cannot be undone.
                      </span>
                      <div class="access-inline-row">
                        <button
                          type="button"
                          class="button button-small button-danger-ghost"
                          disabled={keyBusy !== null}
                          onClick={() => void revokeKey(key)}
                        >
                          {keyBusy === key.id ? 'Revoking…' : `Revoke “${key.name}”`}
                        </button>
                        <button
                          type="button"
                          class="button button-small"
                          disabled={keyBusy !== null}
                          onClick={() => setConfirmRevokeKey(null)}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div class="access-row-actions">
                      {dead ? (
                        <button
                          type="button"
                          class="button button-small"
                          disabled={keyBusy !== null}
                          aria-label={`Remove ${key.name} from the list`}
                          onClick={() => void purgeKey(key)}
                        >
                          Remove
                        </button>
                      ) : (
                        <>
                          <button
                            type="button"
                            class="button button-small"
                            disabled={keyBusy !== null}
                            aria-label={`Rename ${key.name}`}
                            onClick={() => {
                              setConfirmRevokeKey(null);
                              setRenamingKey({ id: key.id, name: key.name });
                            }}
                          >
                            Rename
                          </button>
                          <button
                            type="button"
                            class="button button-small"
                            disabled={keyBusy !== null}
                            aria-label={`Rotate ${key.name}`}
                            onClick={() => void rotateKey(key)}
                          >
                            {keyBusy === key.id ? 'Rotating…' : 'Rotate'}
                          </button>
                          <button
                            type="button"
                            class="button button-small button-danger-ghost"
                            disabled={keyBusy !== null}
                            aria-label={`Revoke ${key.name}`}
                            onClick={() => {
                              setRenamingKey(null);
                              setConfirmRevokeKey(key.id);
                            }}
                          >
                            Revoke
                          </button>
                        </>
                      )}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        <div class="settings-subhead">
          <h4 id="access-api-key-new">Create a key</h4>
          <span class="settings-section-note">
            {apiKeys.filter((key) => key.status === 'active').length} of {keyMax} live
          </span>
        </div>

        <div class="access-form" role="group" aria-labelledby="access-api-key-new">
          <div class="field">
            <label class="field-label" for="access-api-key-name-new">
              Name
            </label>
            <input
              id="access-api-key-name-new"
              class="input"
              type="text"
              placeholder="ci-runner"
              value={newKeyName}
              disabled={keyBusy !== null || !keySignInRequired}
              onInput={(event) => setNewKeyName(event.currentTarget.value)}
            />
            <p class="field-hint">So the list says who holds the key. Up to 64 characters.</p>
          </div>

          <div class="field">
            <label class="field-label" for="access-api-key-days">
              Expires in (days)
            </label>
            <input
              id="access-api-key-days"
              class="input mono"
              type="number"
              min={1}
              max={MAX_KEY_LIFETIME_DAYS}
              step="1"
              placeholder="30"
              value={newKeyExpiry}
              disabled={keyBusy !== null || !keySignInRequired}
              onInput={(event) => setNewKeyExpiry(event.currentTarget.value)}
            />
            <p class="field-hint">
              Leave it empty for a key that never expires (not recommended). Up to{' '}
              {MAX_KEY_LIFETIME_DAYS} days.
            </p>
          </div>

          <div class="field">
            <span class="field-label">Scopes</span>
            <div class="access-scope-row">
              {API_KEY_SCOPES.map((scope) => (
                <label class="access-scope" key={scope.value}>
                  <input
                    type="checkbox"
                    checked={newKeyScopes.includes(scope.value)}
                    disabled={keyBusy !== null || !keySignInRequired}
                    onChange={() => toggleKeyScope(scope.value)}
                  />
                  <span class="access-scope-text">
                    <span class="setting-name">{scope.label}</span>
                    <span class="setting-hint">{scope.description}</span>
                  </span>
                </label>
              ))}
            </div>
            <p class="field-hint">
              <code>Write</code> includes <code>Read</code>; <code>Admin</code> includes both. Admin
              can open terminals and answer approvals — grant it only to an integration you trust
              with the whole machine.
            </p>
          </div>

          <div class="field">
            <label class="field-label" for="access-api-key-admin">
              Admin token (only when configured)
            </label>
            <input
              id="access-api-key-admin"
              class="input"
              type="password"
              autoComplete="off"
              value={adminToken}
              disabled={keyBusy !== null || !keySignInRequired}
              onInput={(event) => setAdminToken(event.currentTarget.value)}
            />
            <p class="field-hint">
              With <code>KARUCODE_ADMIN_TOKEN</code> set, creating and revoking keys needs that
              credential. It stays in this tab; the browser does not store it.
            </p>
          </div>

          <div class="access-actions">
            <button
              type="button"
              class="button button-small button-primary"
              disabled={keyBusy !== null || !keySignInRequired}
              onClick={() => void createKey()}
            >
              {keyBusy === 'create' ? 'Creating…' : 'Create key'}
            </button>
            <span class="field-hint">
              The token is returned once. Send it as <code>Authorization: Bearer …</code>, and keep
              sending <code>X-Karucode-Client: web</code> on mutations.
            </span>
          </div>
        </div>

        {keyMessage ? (
          <p
            class={keyMessage.ok ? 'access-saved' : 'field-error'}
            role={keyMessage.ok ? 'status' : 'alert'}
          >
            {keyMessage.ok ? <CheckIcon size={13} /> : null}
            {keyMessage.text}
          </p>
        ) : null}

        {apiKeysError ? (
          <p class="field-error" role="alert">
            {apiKeysError}
          </p>
        ) : null}
      </section>

      <section class="settings-section" aria-labelledby="settings-access-address">
        <div class="settings-section-head">
          <h3 id="settings-access-address">Address and tunnel</h3>
          <span class="settings-section-note">What this server is called, and what it believes</span>
        </div>

        <div class="field">
          <label class="field-label" for="access-public-url">
            Public address
          </label>
          <input
            id="access-public-url"
            class="input mono"
            type="url"
            spellcheck={false}
            placeholder="https://karucode.example.com"
            value={policy.publicUrl}
            disabled={locked || savingAddress}
            aria-invalid={addressErrors.url ? 'true' : undefined}
            aria-describedby={
              addressErrors.url ? 'access-public-url-error' : 'access-public-url-hint'
            }
            onInput={(event) => update({ publicUrl: event.currentTarget.value })}
          />
          <p class="field-hint" id="access-public-url-hint">
            The address people type to reach this server. It is added to the allowed hosts, and a
            passkey is bound to it. Leave it empty to use this browser’s own origin.
          </p>
          {addressErrors.url ? (
            <p class="field-error" id="access-public-url-error" role="alert">
              {addressErrors.url}
            </p>
          ) : null}
        </div>

        <div class="field">
          <label class="field-label" for="access-tunnel">
            How is it reached?
          </label>
          <select
            id="access-tunnel"
            class="input"
            value={policy.tunnel}
            disabled={locked || savingAddress}
            onInput={(event) => update({ tunnel: event.currentTarget.value as TunnelKind })}
          >
            {TUNNELS.map((tunnel) => (
              <option value={tunnel.value} key={tunnel.value}>
                {tunnel.label}
              </option>
            ))}
          </select>
          <p class="field-hint">
            {TUNNELS.find((tunnel) => tunnel.value === policy.tunnel)?.description}
          </p>
        </div>

        {policy.tunnel !== 'none' ? (
          <div class="field">
            <label class="field-label" for="access-proxies">
              Trusted proxy addresses
            </label>
            <textarea
              id="access-proxies"
              class="input mono access-textarea"
              rows={3}
              spellcheck={false}
              placeholder={'10.0.0.0/8\n172.17.0.1'}
              value={policy.proxies}
              disabled={locked || savingAddress}
              aria-describedby="access-proxies-hint"
              onInput={(event) => update({ proxies: event.currentTarget.value })}
            />
            <p class="field-hint" id="access-proxies-hint">
              One IP or CIDR per line. Forwarded headers are believed only from these addresses
              {policy.tunnel === 'cloudflare'
                ? '; Cloudflare’s published ranges are trusted automatically for a Cloudflare tunnel'
                : ''}
              . Loopback is always trusted, so a proxy on this machine needs no entry.
            </p>
          </div>
        ) : null}

        <div class="field">
          <label class="field-label" for="access-session-days">
            Stay signed in for (days)
          </label>
          <input
            id="access-session-days"
            class="input mono"
            type="number"
            min={MIN_SESSION_DAYS}
            max={MAX_SESSION_DAYS}
            step="1"
            value={policy.sessionDays}
            disabled={locked || savingAddress}
            aria-invalid={addressErrors.days ? 'true' : undefined}
            aria-describedby={
              addressErrors.days ? 'access-session-days-error' : 'access-session-days-hint'
            }
            onInput={(event) => update({ sessionDays: event.currentTarget.value })}
          />
          <p class="field-hint" id="access-session-days-hint">
            How long a browser stays signed in before it has to present the credential again. 1–365
            days.
          </p>
          {addressErrors.days ? (
            <p class="field-error" id="access-session-days-error" role="alert">
              {addressErrors.days}
            </p>
          ) : null}
        </div>

        <div class="access-actions">
          <button
            type="button"
            class="button button-small button-primary"
            disabled={locked || savingAddress}
            onClick={() => void savePolicy()}
          >
            {savingAddress ? 'Saving…' : 'Save'}
          </button>
          <span class="field-hint">
            {locked
              ? 'Settings are locked on this server (KARUCODE_LOCK_SETTINGS); change this in the config file or environment.'
              : 'Saving changes where the server believes it is reached. Sign-in itself is saved the moment its switch moves.'}
          </span>
        </div>

        {addressMessage ? (
          <p
            class={addressMessage.ok ? 'access-saved' : 'field-error'}
            role={addressMessage.ok ? 'status' : 'alert'}
          >
            {addressMessage.ok ? <CheckIcon size={13} /> : null}
            {addressMessage.text}
          </p>
        ) : null}
      </section>
    </>
  );
}
