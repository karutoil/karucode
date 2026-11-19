import type { JSX } from 'preact';
import type { ApiKeySummary } from '../../api/types';
import { absoluteTime, fromUnixSeconds, relativeTime } from '../../lib/format';
import { Badge, Button, Field, Inline, Input, Stack, Text } from '../../ui';

export interface ApiKeyRowProps {
  apiKey: ApiKeySummary;
  keyBusy: string | null;
  renamingKey: { id: string; name: string } | null;
  confirmRevokeKey: string | null;
  onSetRenamingKey: (renaming: { id: string; name: string } | null) => void;
  onSetConfirmRevokeKey: (id: string | null) => void;
  onSaveRename: (id: string, name: string) => Promise<void>;
  onRotate: (key: ApiKeySummary) => Promise<void>;
  onRevoke: (key: ApiKeySummary) => Promise<void>;
  onPurge: (key: ApiKeySummary) => Promise<void>;
}

export function ApiKeyRow({
  apiKey: key,
  keyBusy,
  renamingKey,
  confirmRevokeKey,
  onSetRenamingKey,
  onSetConfirmRevokeKey,
  onSaveRename,
  onRotate,
  onRevoke,
  onPurge,
}: ApiKeyRowProps): JSX.Element {
  const nameId = `access-api-key-name-${key.id}`;
  const isRenaming = renamingKey !== null && renamingKey.id === key.id;
  const isRemoving = confirmRevokeKey === key.id;
  const dead = key.status !== 'active';

  return (
    <li class="access-row" key={key.id}>
      <Stack gap="2" class="access-row-main">
        <Inline gap="3" wrap class="access-row-title">
          <Text size="label" weight="semibold">
            {key.name}
          </Text>
          {dead ? (
            <Badge tone="neutral" mono={false}>
              {key.status}
            </Badge>
          ) : null}
        </Inline>
        <Text size="tiny" tone="muted" class="access-row-meta">
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
        </Text>
      </Stack>

      {isRenaming && renamingKey !== null ? (
        <div class="access-inline-form">
          <Field label="Name for this key" htmlFor={nameId}>
            <Inline gap="4" wrap class="access-inline-row">
              <Input
                id={nameId}
                class="flex-1 min-w-[160px] w-auto"
                type="text"
                value={renamingKey.name}
                disabled={keyBusy !== null}
                onInput={(event) =>
                  onSetRenamingKey({ id: key.id, name: event.currentTarget.value })
                }
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    void onSaveRename(key.id, renamingKey.name);
                  }
                  if (event.key === 'Escape') onSetRenamingKey(null);
                }}
              />
              <Button
                variant="secondary"
                size="sm"
                disabled={keyBusy !== null}
                onClick={() => void onSaveRename(key.id, renamingKey.name)}
              >
                Save
              </Button>
              <Button
                variant="secondary"
                size="sm"
                disabled={keyBusy !== null}
                onClick={() => onSetRenamingKey(null)}
              >
                Cancel
              </Button>
            </Inline>
          </Field>
        </div>
      ) : isRemoving ? (
        <Stack gap="3" class="access-inline-form">
          <Text size="tiny" tone="muted">
            Revoke “{key.name}”? Anything using it stops working immediately; this cannot be
            undone.
          </Text>
          <Inline gap="4" wrap>
            <Button
              variant="danger-ghost"
              size="sm"
              loading={keyBusy === key.id}
              disabled={keyBusy !== null}
              onClick={() => void onRevoke(key)}
            >
              {keyBusy === key.id ? 'Revoking…' : `Revoke “${key.name}”`}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={keyBusy !== null}
              onClick={() => onSetConfirmRevokeKey(null)}
            >
              Cancel
            </Button>
          </Inline>
        </Stack>
      ) : (
        <Inline gap="4" class="access-row-actions">
          {dead ? (
            <Button
              variant="secondary"
              size="sm"
              disabled={keyBusy !== null}
              aria-label={`Remove ${key.name} from the list`}
              onClick={() => void onPurge(key)}
            >
              Remove
            </Button>
          ) : (
            <>
              <Button
                variant="secondary"
                size="sm"
                disabled={keyBusy !== null}
                aria-label={`Rename ${key.name}`}
                onClick={() => {
                  onSetConfirmRevokeKey(null);
                  onSetRenamingKey({ id: key.id, name: key.name });
                }}
              >
                Rename
              </Button>
              <Button
                variant="secondary"
                size="sm"
                loading={keyBusy === key.id}
                disabled={keyBusy !== null}
                aria-label={`Rotate ${key.name}`}
                onClick={() => void onRotate(key)}
              >
                {keyBusy === key.id ? 'Rotating…' : 'Rotate'}
              </Button>
              <Button
                variant="danger-ghost"
                size="sm"
                disabled={keyBusy !== null}
                aria-label={`Revoke ${key.name}`}
                onClick={() => {
                  onSetRenamingKey(null);
                  onSetConfirmRevokeKey(key.id);
                }}
              >
                Revoke
              </Button>
            </>
          )}
        </Inline>
      )}
    </li>
  );
}
