import type { JSX } from 'preact';
import type { TunnelKind } from '../../api/types';
import { KeyIcon } from '../../components/icons';
import { passkeySupported } from '../../lib/passkey';
import { Button, Field, Inline, Input, Select, Stack, Text, Textarea, Toggle } from '../../ui';
import { TUNNELS, type AccessDraft } from './steps';

export interface AccessStepProps {
  access: AccessDraft;
  /** A password already exists on the server, so this step only reports it. */
  passwordSet: boolean;
  /** Passkeys the server already holds, plus any added in this session. */
  passkeyCount: number;
  passkeyBusy: boolean;
  /** The server's own floor, so the hint cannot drift from what it enforces. */
  minPasswordLength: number;
  onPatch: (patch: Partial<AccessDraft>) => void;
  onAddPasskey: () => void;
}

/**
 * Step 5: who can reach this server.
 *
 * The credential comes before the policy, and the server is the authority on
 * both: a password is `POST /api/auth/password` (which is what turns sign-in on),
 * the tunnel and the trusted proxies are a `PATCH /api/config`. A password that
 * was just set is not in the payload this step was built from, so the parent
 * passes `passwordSet` down as the union of the stored state and this session.
 */
export function AccessStep({
  access,
  passwordSet,
  passkeyCount,
  passkeyBusy,
  minPasswordLength,
  onPatch,
  onAddPasskey,
}: AccessStepProps): JSX.Element {
  const mismatch = access.confirm !== '' && access.password !== access.confirm;
  const tunnel = TUNNELS.find((entry) => entry.value === access.tunnel);
  const passkeyAvailable = passkeySupported();

  return (
    <Stack gap="5">
      <Toggle
        id="onboarding-access-enabled"
        checked={access.enabled}
        label="Require sign-in"
        description="A password or passkey before the web UI answers. Leave off for a loopback-only install."
        onChange={(next) => onPatch({ enabled: next })}
      />

      {passwordSet ? (
        <Text as="p" size="tiny" tone="muted">
          A password is already set. Change it later in Settings → Access.
        </Text>
      ) : (
        <>
          <Field
            label="Password"
            htmlFor="onboarding-password"
            description={`At least ${minPasswordLength} characters. Setting a password turns sign-in on for this server.`}
          >
            {(a11y) => (
              <Input
                id={a11y.id}
                type="password"
                autoComplete="new-password"
                value={access.password}
                aria-describedby={a11y.describedBy}
                onInput={(event) => onPatch({ password: event.currentTarget.value })}
              />
            )}
          </Field>
          <Field
            label="Confirm password"
            htmlFor="onboarding-password-confirm"
            error={mismatch ? 'The two passwords do not match.' : undefined}
          >
            {(a11y) => (
              <Input
                id={a11y.id}
                type="password"
                autoComplete="new-password"
                value={access.confirm}
                invalid={a11y.invalid}
                aria-describedby={a11y.describedBy}
                onInput={(event) => onPatch({ confirm: event.currentTarget.value })}
              />
            )}
          </Field>
        </>
      )}

      <Inline gap="4" wrap>
        <Button
          size="sm"
          icon={<KeyIcon size={14} />}
          disabled={passkeyBusy || !passkeyAvailable}
          onClick={onAddPasskey}
        >
          {passkeyBusy ? 'Waiting for your device…' : 'Add a passkey instead'}
        </Button>
        <Text as="span" size="tiny" tone="muted">
          {passkeyCount > 0
            ? `${passkeyCount} passkey${passkeyCount === 1 ? '' : 's'} registered.`
            : passkeyAvailable
              ? 'Fingerprint, face, screen lock or a security key.'
              : 'This browser cannot create a passkey; use a password.'}
        </Text>
      </Inline>

      <Field
        label="Public address (optional)"
        htmlFor="onboarding-domain"
        description="The address people type to reach this server. It is added to the allowed hosts, and passkeys are bound to it."
      >
        {(a11y) => (
          <Input
            id={a11y.id}
            mono
            type="url"
            spellCheck={false}
            placeholder="https://karucode.example.com"
            value={access.domain}
            aria-describedby={a11y.describedBy}
            onInput={(event) => onPatch({ domain: event.currentTarget.value })}
          />
        )}
      </Field>

      <Field
        label="How is it reached?"
        htmlFor="onboarding-tunnel"
        description={tunnel?.description}
      >
        {(a11y) => (
          <Select
            id={a11y.id}
            value={access.tunnel}
            aria-describedby={a11y.describedBy}
            options={TUNNELS.map((entry) => ({ value: entry.value, label: entry.label }))}
            onInput={(event) => onPatch({ tunnel: event.currentTarget.value as TunnelKind })}
          />
        )}
      </Field>

      {access.tunnel !== 'none' ? (
        <Field
          label="Trusted proxy addresses (optional)"
          htmlFor="onboarding-proxies"
          description={
            access.tunnel === 'cloudflare'
              ? 'One IP or CIDR per line. Forwarded headers are only believed from these addresses; Cloudflare’s own ranges are always included for a Cloudflare tunnel. Loopback is always trusted.'
              : 'One IP or CIDR per line. Forwarded headers are only believed from these addresses. Loopback is always trusted.'
          }
        >
          {(a11y) => (
            <Textarea
              id={a11y.id}
              mono
              rows={3}
              spellCheck={false}
              placeholder={'10.0.0.0/8\n172.17.0.1'}
              value={access.proxies}
              aria-describedby={a11y.describedBy}
              onInput={(event) => onPatch({ proxies: event.currentTarget.value })}
            />
          )}
        </Field>
      ) : null}
    </Stack>
  );
}
