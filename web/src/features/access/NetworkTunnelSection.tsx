import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import type { TunnelKind } from '../../api/types';
import * as actions from '../../state/actions';
import { Button, Callout, Field, Inline, Input, Select, Text, Textarea, icons } from '../../ui';
import {
  type AddressErrors,
  MAX_SESSION_DAYS,
  MIN_SESSION_DAYS,
  type Message,
  parseProxies,
  type PolicyDraft,
  TUNNELS,
} from './helpers';

export interface NetworkTunnelSectionProps {
  policy: PolicyDraft;
  locked: boolean;
  onUpdatePolicy: (patch: Partial<PolicyDraft>) => void;
  onCommitPolicy: (normalized: PolicyDraft) => void;
  onReloadPolicy: () => Promise<void>;
}

export function NetworkTunnelSection({
  policy,
  locked,
  onUpdatePolicy,
  onCommitPolicy,
  onReloadPolicy,
}: NetworkTunnelSectionProps): JSX.Element {
  const [addressErrors, setAddressErrors] = useState<AddressErrors>({});
  const [addressMessage, setAddressMessage] = useState<Message | null>(null);
  const [savingAddress, setSavingAddress] = useState(false);

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
      onCommitPolicy({
        ...policy,
        publicUrl: url,
        sessionDays: String(days),
        proxies: proxies.join('\n'),
      });
      await onReloadPolicy();
      setAddressMessage({ ok: true, text: 'Access settings saved.' });
    } catch (error) {
      setAddressMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setSavingAddress(false);
    }
  };

  return (
    <section class="settings-section" aria-labelledby="settings-access-address">
      <div class="settings-section-head">
        <h3 id="settings-access-address">Address and tunnel</h3>
        <Text size="tiny" tone="muted">
          What this server is called, and what it believes
        </Text>
      </div>

      <Field
        label="Public address"
        htmlFor="access-public-url"
        class="field"
        description="The address people type to reach this server. It is added to the allowed hosts, and a passkey is bound to it. Leave it empty to use this browser’s own origin."
        error={addressErrors.url}
      >
        {(a11y) => (
          <Input
            id={a11y.id}
            mono
            type="url"
            spellCheck={false}
            placeholder="https://karucode.example.com"
            value={policy.publicUrl}
            disabled={locked || savingAddress}
            invalid={a11y.invalid}
            aria-describedby={a11y.describedBy}
            onInput={(event) => onUpdatePolicy({ publicUrl: event.currentTarget.value })}
          />
        )}
      </Field>

      <Field
        label="How is it reached?"
        htmlFor="access-tunnel"
        class="field"
        description={TUNNELS.find((tunnel) => tunnel.value === policy.tunnel)?.description}
      >
        {(a11y) => (
          <Select
            id={a11y.id}
            value={policy.tunnel}
            disabled={locked || savingAddress}
            aria-describedby={a11y.describedBy}
            options={TUNNELS.map((tunnel) => ({ value: tunnel.value, label: tunnel.label }))}
            onInput={(event) =>
              onUpdatePolicy({ tunnel: event.currentTarget.value as TunnelKind })
            }
            onChange={(event) =>
              onUpdatePolicy({ tunnel: event.currentTarget.value as TunnelKind })
            }
          />
        )}
      </Field>

      {policy.tunnel !== 'none' ? (
        <Field
          label="Trusted proxy addresses"
          htmlFor="access-proxies"
          class="field"
          description={`One IP or CIDR per line. Forwarded headers are believed only from these addresses${
            policy.tunnel === 'cloudflare'
              ? '; Cloudflare’s published ranges are trusted automatically for a Cloudflare tunnel'
              : ''
          }. Loopback is always trusted, so a proxy on this machine needs no entry.`}
        >
          {(a11y) => (
            <Textarea
              id={a11y.id}
              mono
              rows={3}
              spellCheck={false}
              placeholder={'10.0.0.0/8\n172.17.0.1'}
              value={policy.proxies}
              disabled={locked || savingAddress}
              aria-describedby={a11y.describedBy}
              onInput={(event) => onUpdatePolicy({ proxies: event.currentTarget.value })}
            />
          )}
        </Field>
      ) : null}

      <Field
        label="Stay signed in for (days)"
        htmlFor="access-session-days"
        class="field"
        description="How long a browser stays signed in before it has to present the credential again. 1–365 days."
        error={addressErrors.days}
      >
        {(a11y) => (
          <Input
            id={a11y.id}
            mono
            type="number"
            min={MIN_SESSION_DAYS}
            max={MAX_SESSION_DAYS}
            step="1"
            value={policy.sessionDays}
            disabled={locked || savingAddress}
            invalid={a11y.invalid}
            aria-describedby={a11y.describedBy}
            onInput={(event) => onUpdatePolicy({ sessionDays: event.currentTarget.value })}
          />
        )}
      </Field>

      <Inline gap="6" wrap class="access-actions">
        <Button
          variant="primary"
          size="sm"
          loading={savingAddress}
          disabled={locked || savingAddress}
          onClick={() => void savePolicy()}
        >
          {savingAddress ? 'Saving…' : 'Save'}
        </Button>
        <Text size="tiny" tone="muted">
          {locked
            ? 'Settings are locked on this server (KARUCODE_LOCK_SETTINGS); change this in the config file or environment.'
            : 'Saving changes where the server believes it is reached. Sign-in itself is saved the moment its switch moves.'}
        </Text>
      </Inline>

      {addressMessage ? (
        <Callout
          tone={addressMessage.ok ? 'success' : 'danger'}
          role={addressMessage.ok ? 'status' : 'alert'}
          icon={addressMessage.ok ? <icons.CheckIcon size={13} /> : undefined}
        >
          {addressMessage.text}
        </Callout>
      ) : null}
    </section>
  );
}
