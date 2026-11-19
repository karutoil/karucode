import type { JSX } from 'preact';
import type { ApiKeySummary } from '../../api/types';
import { Button, Callout, Checkbox, Field, Inline, Input, Stack, Text, icons } from '../../ui';
import { ApiKeyRow } from './ApiKeyRow';
import { API_KEY_SCOPES, MAX_KEY_LIFETIME_DAYS } from './helpers';
import { useApiKeys } from './useApiKeys';

export interface ApiKeysSectionProps {
  apiKeys: ApiKeySummary[];
  apiKeysError: string | null;
  keySignInRequired: boolean;
  keyMax: number;
  onRefresh: () => Promise<void>;
}

export function ApiKeysSection({
  apiKeys,
  apiKeysError,
  keySignInRequired,
  keyMax,
  onRefresh,
}: ApiKeysSectionProps): JSX.Element {
  const {
    keyBusy,
    keyMessage,
    issuedKey,
    newKeyName,
    newKeyScopes,
    newKeyExpiry,
    adminToken,
    confirmRevokeKey,
    renamingKey,
    copiedKey,
    setNewKeyName,
    setNewKeyExpiry,
    setAdminToken,
    setConfirmRevokeKey,
    setRenamingKey,
    dismissIssuedKey,
    toggleKeyScope,
    createKey,
    copyIssuedKey,
    rotateKey,
    revokeKey,
    purgeKey,
    saveKeyRename,
  } = useApiKeys(onRefresh);

  return (
    <section class="settings-section" aria-labelledby="settings-access-api-keys">
      <div class="settings-section-head">
        <h3 id="settings-access-api-keys">API keys</h3>
        <Text size="tiny" tone="muted">
          {apiKeys.length === 0
            ? 'None created'
            : `${apiKeys.filter((key) => key.status === 'active').length} active`}
        </Text>
      </div>

      <Text as="p" size="tiny" tone="muted">
        A key is a bearer credential for one application — a script, a CI job, a bot. It works
        like the auth token, but it is scoped, can expire, and can be rotated or revoked without
        changing your password. The full token is shown once, when it is created.
      </Text>

      {!keySignInRequired ? (
        <Callout tone="danger" role="status" icon={<icons.AlertIcon size={15} />}>
          This server requires no credential yet, so a key would protect nothing and creation is
          refused. Set a password above, or <code>KARUCODE_AUTH_TOKEN</code>, first.
        </Callout>
      ) : null}

      {issuedKey !== null ? (
        <div class="access-issued" role="group" aria-label={`Token for ${issuedKey.name}`}>
          <Text size="label" weight="semibold">
            Copy this token now
          </Text>
          <Text size="tiny" tone="muted">
            It is not stored in the browser and will not be shown again. If it is lost, rotate the
            key to get a new one.
          </Text>
          <code class="access-key-token">{issuedKey.token}</code>
          {issuedKey.warning ? (
            <Text size="tiny" tone="muted">
              {issuedKey.warning}
            </Text>
          ) : null}
          <Inline gap="4" wrap>
            <Button variant="secondary" size="sm" onClick={() => void copyIssuedKey()}>
              {copiedKey ? 'Copied' : 'Copy token'}
            </Button>
            <Button variant="secondary" size="sm" onClick={dismissIssuedKey}>
              I have saved it
            </Button>
          </Inline>
        </div>
      ) : null}

      {apiKeys.length === 0 ? (
        <Text as="p" size="tiny" tone="muted">
          No API keys yet.
        </Text>
      ) : (
        <Stack as="ul" gap="1" class="access-list" aria-label="API keys">
          {apiKeys.map((key) => (
            <ApiKeyRow
              key={key.id}
              apiKey={key}
              keyBusy={keyBusy}
              renamingKey={renamingKey}
              confirmRevokeKey={confirmRevokeKey}
              onSetRenamingKey={setRenamingKey}
              onSetConfirmRevokeKey={setConfirmRevokeKey}
              onSaveRename={saveKeyRename}
              onRotate={rotateKey}
              onRevoke={revokeKey}
              onPurge={purgeKey}
            />
          ))}
        </Stack>
      )}

      <div class="settings-subhead">
        <h4 id="access-api-key-new">Create a key</h4>
        <Text size="tiny" tone="muted">
          {apiKeys.filter((key) => key.status === 'active').length} of {keyMax} live
        </Text>
      </div>

      <div class="access-form" role="group" aria-labelledby="access-api-key-new">
        <Field
          label="Name"
          htmlFor="access-api-key-name-new"
          class="field"
          description="So the list says who holds the key. Up to 64 characters."
        >
          <Input
            id="access-api-key-name-new"
            type="text"
            placeholder="ci-runner"
            value={newKeyName}
            disabled={keyBusy !== null || !keySignInRequired}
            onInput={(event) => setNewKeyName(event.currentTarget.value)}
          />
        </Field>

        <Field
          label="Expires in (days)"
          htmlFor="access-api-key-days"
          class="field"
          description={`Leave it empty for a key that never expires (not recommended). Up to ${MAX_KEY_LIFETIME_DAYS} days.`}
        >
          <Input
            id="access-api-key-days"
            mono
            type="number"
            min={1}
            max={MAX_KEY_LIFETIME_DAYS}
            step="1"
            placeholder="30"
            value={newKeyExpiry}
            disabled={keyBusy !== null || !keySignInRequired}
            onInput={(event) => setNewKeyExpiry(event.currentTarget.value)}
          />
        </Field>

        <div class="field">
          <Text size="label" weight="semibold" tone="secondary">
            Scopes
          </Text>
          <Stack gap="4" class="access-scope-row">
            {API_KEY_SCOPES.map((scope) => (
              <Checkbox
                key={scope.value}
                checked={newKeyScopes.includes(scope.value)}
                disabled={keyBusy !== null || !keySignInRequired}
                onChange={() => toggleKeyScope(scope.value)}
                label={scope.label}
                description={scope.description}
              />
            ))}
          </Stack>
          <Text as="p" size="tiny" tone="muted">
            <code>Write</code> includes <code>Read</code>; <code>Admin</code> includes both. Admin
            can open terminals and answer approvals — grant it only to an integration you trust
            with the whole machine.
          </Text>
        </div>

        <Field
          label="Admin token (only when configured)"
          htmlFor="access-api-key-admin"
          class="field"
        >
          <Input
            id="access-api-key-admin"
            type="password"
            autoComplete="off"
            value={adminToken}
            disabled={keyBusy !== null || !keySignInRequired}
            onInput={(event) => setAdminToken(event.currentTarget.value)}
          />
          <Text as="p" size="tiny" tone="muted">
            With <code>KARUCODE_ADMIN_TOKEN</code> set, creating and revoking keys needs that
            credential. It stays in this tab; the browser does not store it.
          </Text>
        </Field>

        <Inline gap="6" wrap class="access-actions">
          <Button
            variant="primary"
            size="sm"
            loading={keyBusy === 'create'}
            disabled={keyBusy !== null || !keySignInRequired}
            onClick={() => void createKey()}
          >
            {keyBusy === 'create' ? 'Creating…' : 'Create key'}
          </Button>
          <Text size="tiny" tone="muted">
            The token is returned once. Send it as <code>Authorization: Bearer …</code>, and keep
            sending <code>X-Karucode-Client: web</code> on mutations.
          </Text>
        </Inline>
      </div>

      {keyMessage ? (
        <Callout
          tone={keyMessage.ok ? 'success' : 'danger'}
          role={keyMessage.ok ? 'status' : 'alert'}
          icon={keyMessage.ok ? <icons.CheckIcon size={13} /> : undefined}
        >
          {keyMessage.text}
        </Callout>
      ) : null}

      {apiKeysError ? (
        <Callout tone="danger" role="alert">
          {apiKeysError}
        </Callout>
      ) : null}
    </section>
  );
}
