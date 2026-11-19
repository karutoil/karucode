import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import { Button, Callout, Field, Inline, Input, Text, icons } from '../../ui';
import type { Message, PasswordErrors } from './helpers';

export interface PasswordSectionProps {
  passwordSet: boolean;
  minPasswordLen: number;
  onCredentialsChanged: () => Promise<void>;
}

export function PasswordSection({
  passwordSet,
  minPasswordLen,
  onCredentialsChanged,
}: PasswordSectionProps): JSX.Element {
  const [passwordForm, setPasswordForm] = useState({ current: '', next: '', confirm: '' });
  const [passwordErrors, setPasswordErrors] = useState<PasswordErrors>({});
  const [passwordMessage, setPasswordMessage] = useState<Message | null>(null);
  const [passwordBusy, setPasswordBusy] = useState(false);
  const [removingPassword, setRemovingPassword] = useState(false);
  const [removePasswordValue, setRemovePasswordValue] = useState('');
  const [removeBusy, setRemoveBusy] = useState(false);

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
      await onCredentialsChanged();
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
      await onCredentialsChanged();
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

  return (
    <section class="settings-section" aria-labelledby="settings-access-password">
      <div class="settings-section-head">
        <h3 id="settings-access-password">Password</h3>
        <Text size="tiny" tone="muted">
          {passwordSet ? 'Changing or removing it needs the current one' : 'Nothing to sign in with yet'}
        </Text>
      </div>

      <div class="settings-subhead">
        <h4 id="access-password-heading">{passwordSet ? 'Change password' : 'Set a password'}</h4>
        <Text size="tiny" tone="muted">
          {passwordSet
            ? 'Knowing the current password is what stops a hijacked browser from locking you out.'
            : `At least ${minPasswordLen} characters. Setting one turns sign-in on.`}
        </Text>
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
          <Field
            label="Current password"
            htmlFor="access-password-current"
            class="field"
            error={passwordErrors.current}
          >
            {(a11y) => (
              <Input
                id={a11y.id}
                type="password"
                autoComplete="current-password"
                value={passwordForm.current}
                disabled={passwordBusy}
                invalid={a11y.invalid}
                aria-describedby={a11y.describedBy}
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setPasswordForm((form) => ({ ...form, current: value }));
                }}
              />
            )}
          </Field>
        ) : null}

        <Field
          label={passwordSet ? 'New password' : 'Password'}
          htmlFor="access-password-new"
          class="field"
          description={`At least ${minPasswordLen} characters.`}
          error={passwordErrors.next}
        >
          {(a11y) => (
            <Input
              id={a11y.id}
              type="password"
              autoComplete="new-password"
              value={passwordForm.next}
              disabled={passwordBusy}
              invalid={a11y.invalid}
              aria-describedby={a11y.describedBy}
              onInput={(event) => {
                const value = event.currentTarget.value;
                setPasswordForm((form) => ({ ...form, next: value }));
              }}
            />
          )}
        </Field>

        <Field
          label="Confirm password"
          htmlFor="access-password-confirm"
          class="field"
          error={passwordErrors.confirm}
        >
          {(a11y) => (
            <Input
              id={a11y.id}
              type="password"
              autoComplete="new-password"
              value={passwordForm.confirm}
              disabled={passwordBusy}
              invalid={a11y.invalid}
              aria-describedby={a11y.describedBy}
              onInput={(event) => {
                const value = event.currentTarget.value;
                setPasswordForm((form) => ({ ...form, confirm: value }));
              }}
            />
          )}
        </Field>

        <Inline gap="4" class="access-actions">
          <Button
            variant="secondary"
            size="sm"
            loading={passwordBusy}
            disabled={passwordBusy}
            onClick={() => void submitPassword()}
          >
            {passwordBusy
              ? 'Saving…'
              : passwordSet
                ? 'Save new password'
                : 'Set password'}
          </Button>
        </Inline>
      </div>

      {passwordSet ? (
        <div class="access-danger">
          <div class="setting-copy">
            <Text size="label" weight="semibold">
              Remove the password
            </Text>
            <Text size="tiny" tone="muted">
              The current password is required. Your passkeys keep working; the last credential
              removed also turns sign-in off.
            </Text>
          </div>
          {removingPassword ? (
            <div class="access-inline-form">
              <Field label="Current password to remove it" htmlFor="access-password-remove">
                <Inline gap="4" wrap class="access-inline-row">
                  <Input
                    id="access-password-remove"
                    class="flex-1 min-w-[160px] w-auto"
                    type="password"
                    autoComplete="current-password"
                    value={removePasswordValue}
                    disabled={removeBusy}
                    onInput={(event) => setRemovePasswordValue(event.currentTarget.value)}
                  />
                  <Button
                    variant="danger-ghost"
                    size="sm"
                    loading={removeBusy}
                    disabled={removeBusy}
                    onClick={() => void removePassword()}
                  >
                    {removeBusy ? 'Removing…' : 'Yes, remove it'}
                  </Button>
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={removeBusy}
                    onClick={() => {
                      setRemovingPassword(false);
                      setRemovePasswordValue('');
                    }}
                  >
                    Cancel
                  </Button>
                </Inline>
              </Field>
            </div>
          ) : (
            <Button
              variant="danger-ghost"
              size="sm"
              onClick={() => {
                setRemovingPassword(true);
                setPasswordMessage(null);
              }}
            >
              Remove password
            </Button>
          )}
        </div>
      ) : null}

      {passwordMessage ? (
        <Callout
          tone={passwordMessage.ok ? 'success' : 'danger'}
          role={passwordMessage.ok ? 'status' : 'alert'}
          icon={passwordMessage.ok ? <icons.CheckIcon size={13} /> : undefined}
        >
          {passwordMessage.text}
        </Callout>
      ) : null}
    </section>
  );
}
