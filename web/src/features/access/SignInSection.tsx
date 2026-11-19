import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import * as actions from '../../state/actions';
import { Callout, Text, Toggle, icons } from '../../ui';
import type { Message } from './helpers';

export interface SignInSectionProps {
  enabled: boolean;
  hasCredential: boolean;
  locked: boolean;
  onUpdateDraftEnabled: (enabled: boolean) => void;
  onUpdateSettingsEnabled: (enabled: boolean) => void;
}

export function SignInSection({
  enabled,
  hasCredential,
  locked,
  onUpdateDraftEnabled,
  onUpdateSettingsEnabled,
}: SignInSectionProps): JSX.Element {
  const [enabledBusy, setEnabledBusy] = useState(false);
  const [signInMessage, setSignInMessage] = useState<Message | null>(null);

  /**
   * Flip the sign-in requirement.
   *
   * This one saves on the click rather than waiting for the Save button: it is a
   * security decision the user just made explicitly, and a switch that only takes
   * effect when they later remember to press something else is a trap.
   */
  const setEnabled = async (next: boolean): Promise<void> => {
    if (enabledBusy) return;
    const previous = enabled;
    setEnabledBusy(true);
    setSignInMessage(null);
    onUpdateDraftEnabled(next);
    try {
      await actions.applyConfig({ access: { enabled: next } });
      onUpdateSettingsEnabled(next);
      setSignInMessage({
        ok: true,
        text: next
          ? 'Sign-in is now required. Every browser must present a password or passkey.'
          : 'Sign-in is off. Anyone who can reach this port can use this server.',
      });
    } catch (error) {
      // The server refused, so it is still what it was: put the switch back
      // instead of leaving the screen claiming a policy that is not in force.
      onUpdateDraftEnabled(previous);
      setSignInMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setEnabledBusy(false);
    }
  };

  return (
    <section class="settings-section" aria-labelledby="settings-access-signin">
      <div class="settings-section-head">
        <h3 id="settings-access-signin">Sign-in</h3>
        <Text size="tiny" tone="muted">
          Who may use this web UI at all
        </Text>
      </div>

      <Toggle
        id="access-enabled"
        checked={enabled}
        disabled={locked || enabledBusy}
        label="Require a password or passkey"
        description="A credential before the API answers. Off means anything that can reach this port can use the agent, its files and its shell."
        onChange={(next) => void setEnabled(next)}
      />

      {!enabled && hasCredential ? (
        <Callout tone="danger" role="status" icon={<icons.AlertIcon size={15} />}>
          Sign-in is off while a credential exists: this server is open to anyone who can reach the
          port. Turn the switch on to require it.
        </Callout>
      ) : null}

      {!hasCredential ? (
        <Text as="p" size="tiny" tone="muted">
          Turning this on needs a credential first — the server refuses it otherwise, so that nobody
          can lock themselves out. Set a password or add a passkey below, then come back.
        </Text>
      ) : null}

      {signInMessage ? (
        <Callout
          tone={signInMessage.ok ? 'success' : 'danger'}
          role={signInMessage.ok ? 'status' : 'alert'}
          icon={signInMessage.ok ? <icons.CheckIcon size={13} /> : undefined}
        >
          {signInMessage.text}
        </Callout>
      ) : null}
    </section>
  );
}
