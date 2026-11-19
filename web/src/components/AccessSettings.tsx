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
import type { AccessSettingsSummary, ApiKeySummary, AuthSession } from '../api/types';
import { ApiKeysSection } from '../features/access/ApiKeysSection';
import { draftFrom, MIN_PASSWORD_FALLBACK, type PolicyDraft } from '../features/access/helpers';
import { NetworkTunnelSection } from '../features/access/NetworkTunnelSection';
import { PasskeysSection } from '../features/access/PasskeysSection';
import { PasswordSection } from '../features/access/PasswordSection';
import { SessionsSection } from '../features/access/SessionsSection';
import { SignInSection } from '../features/access/SignInSection';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Button, Callout, Inline, Spinner, Text } from '../ui';

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

  const [apiKeys, setApiKeys] = useState<ApiKeySummary[]>([]);
  const [apiKeysError, setApiKeysError] = useState<string | null>(null);
  const [keySignInRequired, setKeySignInRequired] = useState(true);
  const [keyMax, setKeyMax] = useState(100);

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

  const updatePolicy = (patch: Partial<PolicyDraft>): void => {
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

  const handlePasswordChanged = async (): Promise<void> => {
    await reloadPolicy();
    await refreshSessions();
    await refreshApiKeys();
  };

  const handlePasskeysChanged = async (): Promise<void> => {
    await reloadPolicy();
    await refreshApiKeys();
  };

  if (loading && settings === null) {
    return (
      <section class="settings-section" aria-labelledby="settings-access-loading">
        <div class="settings-section-head">
          <h3 id="settings-access-loading">Access</h3>
          <Text size="tiny" tone="muted">
            Reading the sign-in policy…
          </Text>
        </div>
        <Callout tone="info" role="status" icon={<Spinner size={14} />}>
          Fetching the password, passkey and signed-in-browser state from this server.
        </Callout>
      </section>
    );
  }

  if (loadError !== null && settings === null) {
    return (
      <section class="settings-section" aria-labelledby="settings-access-loading">
        <div class="settings-section-head">
          <h3 id="settings-access-loading">Access</h3>
          <Text size="tiny" tone="muted">
            Nothing could be read
          </Text>
        </div>
        <Callout tone="danger" role="alert">
          {loadError}
        </Callout>
        <Inline gap="4">
          <Button
            variant="secondary"
            size="sm"
            onClick={() => setReloadToken((token) => token + 1)}
          >
            Try again
          </Button>
        </Inline>
      </section>
    );
  }

  return (
    <>
      <SignInSection
        enabled={policy.enabled}
        hasCredential={hasCredential}
        locked={locked}
        onUpdateDraftEnabled={(enabled) =>
          setDraft((current) => ({ ...(current ?? draftFrom(settings)), enabled }))
        }
        onUpdateSettingsEnabled={(enabled) =>
          setSettings((current) => (current ? { ...current, enabled } : current))
        }
      />

      <PasswordSection
        passwordSet={passwordSet}
        minPasswordLen={minPasswordLen}
        onCredentialsChanged={handlePasswordChanged}
      />

      <PasskeysSection
        passkeys={passkeys}
        onReloadPolicy={reloadPolicy}
        onCredentialsChanged={handlePasskeysChanged}
      />

      <SessionsSection
        sessions={sessions}
        currentSession={currentSession}
        error={sessionsError}
        onError={setSessionsError}
        onRefresh={refreshSessions}
      />

      <ApiKeysSection
        apiKeys={apiKeys}
        apiKeysError={apiKeysError}
        keySignInRequired={keySignInRequired}
        keyMax={keyMax}
        onRefresh={refreshApiKeys}
      />

      <NetworkTunnelSection
        policy={policy}
        locked={locked}
        onUpdatePolicy={updatePolicy}
        onCommitPolicy={setDraft}
        onReloadPolicy={reloadPolicy}
      />
    </>
  );
}
