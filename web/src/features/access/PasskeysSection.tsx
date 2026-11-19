import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { AuthPasskey } from '../../api/types';
import { absoluteTime, fromUnixSeconds, relativeTime } from '../../lib/format';
import { createPasskey, PasskeyError, passkeySupported } from '../../lib/passkey';
import { Button, Callout, Field, Inline, Input, Stack, Text, icons } from '../../ui';
import { type Message, newPasskeyName } from './helpers';

export interface PasskeysSectionProps {
  passkeys: AuthPasskey[];
  onReloadPolicy: () => Promise<void>;
  onCredentialsChanged: () => Promise<void>;
}

export function PasskeysSection({
  passkeys,
  onReloadPolicy,
  onCredentialsChanged,
}: PasskeysSectionProps): JSX.Element {
  const [passkeyBusy, setPasskeyBusy] = useState(false);
  const [passkeyMessage, setPasskeyMessage] = useState<Message | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [confirmRemovePasskey, setConfirmRemovePasskey] = useState<string | null>(null);

  const supported = passkeySupported();

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
      await onCredentialsChanged();
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
      await onReloadPolicy();
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
      await onCredentialsChanged();
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

  return (
    <section class="settings-section" aria-labelledby="settings-access-passkeys">
      <div class="settings-section-head">
        <h3 id="settings-access-passkeys">Passkeys</h3>
        <Text size="tiny" tone="muted">
          {passkeys.length === 0 ? 'None registered' : `${passkeys.length} registered`}
        </Text>
      </div>

      <Text as="p" size="tiny" tone="muted">
        A passkey signs in with the fingerprint, face, screen lock or security key on a device.
        Nothing secret leaves the device, and a passkey cannot be phished the way a password can.
      </Text>

      {passkeys.length > 0 ? (
        <Stack as="ul" gap="1" class="access-list" aria-label="Registered passkeys">
          {passkeys.map((passkey, index) => {
            const nameId = `access-passkey-name-${index}`;
            const isRenaming = renaming !== null && renaming.id === passkey.id;
            const isRemoving = confirmRemovePasskey === passkey.id;
            return (
              <li class="access-row" key={passkey.id}>
                <Stack gap="2" class="access-row-main">
                  <Text size="label" weight="semibold" class="access-row-title">
                    {passkey.name}
                  </Text>
                  <Text size="tiny" tone="muted" class="access-row-meta">
                    Added {absoluteTime(fromUnixSeconds(passkey.created_at))}
                    {passkey.last_used_at !== null && passkey.last_used_at > 0
                      ? ` · last used ${relativeTime(fromUnixSeconds(passkey.last_used_at))}`
                      : ' · never used'}
                  </Text>
                </Stack>

                {isRenaming && renaming !== null ? (
                  <div class="access-inline-form">
                    <Field label="Name for this passkey" htmlFor={nameId}>
                      <Inline gap="4" wrap class="access-inline-row">
                        <Input
                          id={nameId}
                          class="flex-1 min-w-[160px] w-auto"
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
                        <Button
                          variant="secondary"
                          size="sm"
                          disabled={passkeyBusy}
                          onClick={() => void saveRename(passkey.id, renaming.name)}
                        >
                          Save
                        </Button>
                        <Button
                          variant="secondary"
                          size="sm"
                          disabled={passkeyBusy}
                          onClick={() => setRenaming(null)}
                        >
                          Cancel
                        </Button>
                      </Inline>
                    </Field>
                  </div>
                ) : isRemoving ? (
                  <Stack gap="3" class="access-inline-form">
                    <Text size="tiny" tone="muted">
                      Remove “{passkey.name}”? That device will no longer be able to sign in with
                      it.
                    </Text>
                    <Inline gap="4" wrap>
                      <Button
                        variant="danger-ghost"
                        size="sm"
                        loading={passkeyBusy}
                        disabled={passkeyBusy}
                        onClick={() => void removePasskey(passkey.id)}
                      >
                        {passkeyBusy ? 'Removing…' : `Remove “${passkey.name}”`}
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        disabled={passkeyBusy}
                        onClick={() => setConfirmRemovePasskey(null)}
                      >
                        Cancel
                      </Button>
                    </Inline>
                  </Stack>
                ) : (
                  <Inline gap="4" class="access-row-actions">
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={passkeyBusy}
                      aria-label={`Rename ${passkey.name}`}
                      onClick={() => {
                        setConfirmRemovePasskey(null);
                        setRenaming({ id: passkey.id, name: passkey.name });
                      }}
                    >
                      Rename
                    </Button>
                    <Button
                      variant="danger-ghost"
                      size="sm"
                      disabled={passkeyBusy}
                      aria-label={`Remove ${passkey.name}`}
                      onClick={() => {
                        setRenaming(null);
                        setConfirmRemovePasskey(passkey.id);
                      }}
                    >
                      Remove
                    </Button>
                  </Inline>
                )}
              </li>
            );
          })}
        </Stack>
      ) : (
        <Text as="p" size="tiny" tone="muted">
          No passkeys yet. Add one to sign in without typing a password.
        </Text>
      )}

      <Inline gap="6" wrap class="access-actions">
        <Button
          variant="secondary"
          size="sm"
          icon={<icons.KeyIcon size={14} />}
          loading={passkeyBusy}
          disabled={passkeyBusy || !supported}
          onClick={() => void addPasskey()}
        >
          {passkeyBusy ? 'Waiting for your device…' : 'Add a passkey'}
        </Button>
        <Text size="tiny" tone="muted">
          {supported
            ? 'Fingerprint, face, screen lock or a security key.'
            : 'This browser cannot create a passkey (no WebAuthn, or the page is not on HTTPS or localhost). Use a password instead.'}
        </Text>
      </Inline>

      {passkeyMessage ? (
        <Callout
          tone={passkeyMessage.ok ? 'success' : 'danger'}
          role={passkeyMessage.ok ? 'status' : 'alert'}
          icon={passkeyMessage.ok ? <icons.CheckIcon size={13} /> : undefined}
        >
          {passkeyMessage.text}
        </Callout>
      ) : null}
    </section>
  );
}
