import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { AuthSession } from '../../api/types';
import { absoluteTime, fromUnixSeconds } from '../../lib/format';
import * as actions from '../../state/actions';
import { Badge, Button, Callout, Inline, Stack, Text } from '../../ui';

export interface SessionsSectionProps {
  sessions: AuthSession[];
  currentSession: string | null;
  error: string | null;
  onError: (error: string | null) => void;
  onRefresh: () => Promise<void>;
}

export function SessionsSection({
  sessions,
  currentSession,
  error,
  onError,
  onRefresh,
}: SessionsSectionProps): JSX.Element {
  const [sessionBusy, setSessionBusy] = useState<string | null>(null);

  const signOutSession = async (session: AuthSession): Promise<void> => {
    if (session.id === currentSession) {
      // This browser's own row is the ordinary logout: revoking it first would
      // 401 every request in between.
      await actions.signOut();
      return;
    }
    setSessionBusy(session.id);
    onError(null);
    try {
      await api.revokeAuthSession(session.id);
      await onRefresh();
    } catch (err) {
      onError(toErrorMessage(err));
    } finally {
      setSessionBusy(null);
    }
  };

  const signOutEverywhere = async (): Promise<void> => {
    if (sessionBusy !== null) return;
    setSessionBusy('all');
    onError(null);
    try {
      await api.revokeAllAuthSessions();
    } catch (err) {
      onError(toErrorMessage(err));
      setSessionBusy(null);
      return;
    }
    // Every session is gone, this one included, so the sign-in screen is the only
    // honest next thing to show.
    await actions.signOut();
  };

  return (
    <section class="settings-section" aria-labelledby="settings-access-sessions">
      <div class="settings-section-head">
        <h3 id="settings-access-sessions">Sessions</h3>
        <Text size="tiny" tone="muted">
          Browsers signed in right now
        </Text>
      </div>

      <Text as="p" size="tiny" tone="muted">
        Each row is one browser holding a session cookie. Revoking one signs that browser out on its
        next request.
      </Text>

      {sessions.length === 0 ? (
        <Text as="p" size="tiny" tone="muted">
          No browser is signed in.
        </Text>
      ) : (
        <Stack as="ul" gap="1" class="access-list" aria-label="Signed-in browsers">
          {sessions.map((session) => (
            <li class="access-row" key={session.id}>
              <Stack gap="2" class="access-row-main">
                <Inline gap="3" wrap class="access-row-title">
                  <Text size="label" weight="semibold">
                    {session.label}
                  </Text>
                  {session.id === currentSession ? (
                    <Badge tone="accent" mono={false}>
                      This browser
                    </Badge>
                  ) : null}
                </Inline>
                <Text size="tiny" tone="muted" class="access-row-meta">
                  Signed in with {session.method} · since{' '}
                  {absoluteTime(fromUnixSeconds(session.created_at))} · expires{' '}
                  {absoluteTime(fromUnixSeconds(session.expires_at))}
                </Text>
              </Stack>
              <Inline gap="4" class="access-row-actions">
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={sessionBusy !== null}
                  aria-label={`Sign out ${session.label}`}
                  onClick={() => void signOutSession(session)}
                >
                  Sign out
                </Button>
              </Inline>
            </li>
          ))}
        </Stack>
      )}

      <Inline gap="6" wrap class="access-actions">
        <Button
          variant="danger-ghost"
          size="sm"
          loading={sessionBusy === 'all'}
          disabled={sessionBusy !== null}
          onClick={() => void signOutEverywhere()}
        >
          {sessionBusy === 'all' ? 'Signing out…' : 'Sign out everywhere'}
        </Button>
        <Text size="tiny" tone="muted">
          Revokes every session, including this one — you will be asked to sign in again.
        </Text>
      </Inline>

      {error ? (
        <Callout tone="danger" role="alert">
          {error}
        </Callout>
      ) : null}
    </section>
  );
}
