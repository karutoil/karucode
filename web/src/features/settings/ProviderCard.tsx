import type { JSX } from 'preact';
import type { ModelEntry, ProviderProfile } from '../../api/types';
import { CheckIcon, RefreshIcon } from '../../components/icons';
import { Badge, Button, Checkbox, Chip, Input, Spinner, Text } from '../../ui';
import { latencyLabel } from './providerHelpers';
import { ProviderModelLibrary } from './ProviderModelLibrary';
import type { TestState } from './types';

export interface ProviderCardProps {
  profile: ProviderProfile;
  isActive: boolean;
  selectedModelId: string | null;
  test?: TestState;
  disabled: boolean;
  pending: Record<string, boolean>;
  keyEditorOpen: boolean;
  keyDraft: string;
  modelDraft: string;
  modelError?: string;
  onRunTest: () => void;
  onSetActive: () => void;
  onToggleAggregator: () => void;
  onToggleKeyEditor: () => void;
  onKeyDraftChange: (value: string) => void;
  onSaveKey: () => void;
  onCancelKeyEditor: () => void;
  onRequestClearKey: () => void;
  onOpenCatalog: () => void;
  onRequestRemove: () => void;
  onModelDraftChange: (value: string) => void;
  onAddModelById: () => void;
  onUseModel: (modelId: string) => void;
  onEditModel: (model: ModelEntry) => void;
  onRefreshModel: (model: ModelEntry) => void;
  onRemoveModel: (modelId: string) => void;
}

export function ProviderCard({
  profile,
  isActive,
  selectedModelId,
  test,
  disabled,
  pending,
  keyEditorOpen,
  keyDraft,
  modelDraft,
  modelError,
  onRunTest,
  onSetActive,
  onToggleAggregator,
  onToggleKeyEditor,
  onKeyDraftChange,
  onSaveKey,
  onCancelKeyEditor,
  onRequestClearKey,
  onOpenCatalog,
  onRequestRemove,
  onModelDraftChange,
  onAddModelById,
  onUseModel,
  onEditModel,
  onRefreshModel,
  onRemoveModel,
}: ProviderCardProps): JSX.Element {
  const headingId = `provider-${profile.id}-name`;
  const testPending = pending[`${profile.id}:test`] === true;
  const keyPending = pending[`${profile.id}:key`] === true;
  const modelsPending = pending[`${profile.id}:models`] === true;
  const usePending = pending[`${profile.id}:use`] === true;
  const activePending = pending[`${profile.id}:active`] === true;
  const aggregatorPending = pending[`${profile.id}:aggregator`] === true;

  return (
    <article class="provider-card" aria-labelledby={headingId}>
      <header class="provider-card-head">
        <div class="provider-card-title">
          <Text as="h4" class="provider-name" id={headingId}>
            {profile.name}
          </Text>
          <code class="provider-id">{profile.id}</code>
        </div>
        <div class="provider-badges">
          {isActive ? (
            <Badge tone="success" class="provider-badge">
              active
            </Badge>
          ) : null}
          <Chip
            tone={profile.api_key_set ? 'success' : 'neutral'}
            icon={profile.api_key_set ? <CheckIcon size={11} /> : undefined}
          >
            {profile.api_key_set ? 'key set' : 'no key'}
          </Chip>
        </div>
      </header>
      <div class="provider-meta">
        <Chip tone="neutral">{profile.provider}</Chip>
        <code class="provider-url" title={profile.base_url}>
          {profile.base_url}
        </code>
        <span class="provider-count">
          {profile.models.length} model{profile.models.length === 1 ? '' : 's'}
        </span>
      </div>

      <Checkbox
        class="provider-toggle w-full"
        checked={profile.aggregator}
        disabled={disabled || aggregatorPending}
        label="Multi-channel gateway"
        description="Retry a 401, 403 or 404 on another upstream channel instead of ending the run. On for a proxy that fronts several providers; off for a direct vendor API, where a revoked key or a missing model stays that way."
        onChange={() => onToggleAggregator()}
      />

      <div class="provider-actions">
        <Button
          size="sm"
          loading={testPending}
          icon={<RefreshIcon size={12} />}
          disabled={disabled || testPending}
          aria-label={`Test connection for ${profile.name}`}
          onClick={onRunTest}
        >
          {testPending ? 'Testing…' : 'Test'}
        </Button>
        <Button
          size="sm"
          disabled={disabled || isActive || activePending}
          aria-label={`Set active provider: ${profile.name}`}
          onClick={onSetActive}
        >
          {isActive ? 'Active' : 'Set active'}
        </Button>
        <Button
          size="sm"
          disabled={disabled}
          aria-expanded={keyEditorOpen}
          aria-label={
            profile.api_key_set ? `Replace key for ${profile.name}` : `Add key for ${profile.name}`
          }
          onClick={onToggleKeyEditor}
        >
          {profile.api_key_set ? 'Replace key' : 'Add key'}
        </Button>
        {profile.api_key_set ? (
          <Button
            size="sm"
            variant="danger-ghost"
            disabled={disabled || keyPending}
            aria-label={`Clear key for ${profile.name}`}
            onClick={onRequestClearKey}
          >
            Clear key
          </Button>
        ) : null}
        <Button
          size="sm"
          disabled={disabled}
          aria-label={`Browse models.dev for ${profile.name}`}
          onClick={onOpenCatalog}
        >
          Browse models.dev
        </Button>
        <Button
          size="sm"
          variant="danger-ghost"
          disabled={disabled}
          aria-label={`Remove profile ${profile.name}`}
          onClick={onRequestRemove}
        >
          Remove profile
        </Button>
      </div>

      {test && (test.running || test.result || test.error) ? (
        <div
          class={`test-result${test.result ? (test.result.ok ? ' is-ok' : ' is-error') : ''}`}
          role="status"
        >
          {test.running ? (
            <>
              <Spinner size={14} />
              <span>Testing {profile.name}…</span>
            </>
          ) : null}
          {test.result ? (
            <>
              <strong>{test.result.ok ? 'Connection succeeded' : 'Connection failed'}</strong>
              <span>{latencyLabel(test.result.latency_ms)}</span>
              {test.result.models && test.result.models.length > 0 ? (
                <span class="test-models">
                  {test.result.models.length} model{test.result.models.length === 1 ? '' : 's'}:{' '}
                  {test.result.models.slice(0, 8).join(', ')}
                  {test.result.models.length > 8 ? '…' : ''}
                </span>
              ) : null}
              {test.result.error ? <span class="test-error">{test.result.error}</span> : null}
            </>
          ) : null}
          {test.error ? <span class="test-error">{test.error}</span> : null}
        </div>
      ) : null}

      {keyEditorOpen ? (
        <div class="provider-key-editor">
          <Text as="label" htmlFor={`provider-key-${profile.id}`} class="field-label">
            {profile.api_key_set ? 'New API key' : 'API key'} for {profile.name}
          </Text>
          <div class="provider-key-row">
            <Input
              id={`provider-key-${profile.id}`}
              mono
              type="password"
              class="flex-1 basis-[220px] min-w-0"
              placeholder={profile.api_key_set ? 'Enter a new key to replace the stored one' : 'sk-…'}
              value={keyDraft}
              disabled={disabled || keyPending}
              onInput={(event) => onKeyDraftChange(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  onSaveKey();
                }
              }}
            />
            <Button
              size="sm"
              variant="primary"
              loading={keyPending}
              disabled={disabled || keyPending || keyDraft.trim() === ''}
              aria-label={`Save key for ${profile.name}`}
              onClick={onSaveKey}
            >
              {keyPending ? 'Saving…' : 'Save key'}
            </Button>
            <Button
              size="sm"
              aria-label={`Cancel key change for ${profile.name}`}
              onClick={onCancelKeyEditor}
            >
              Cancel
            </Button>
          </div>
          <Text as="p" class="field-hint">
            Write-only: the server never returns the stored key. Saving replaces the current one.
          </Text>
        </div>
      ) : null}

      <ProviderModelLibrary
        profile={profile}
        selectedModelId={selectedModelId}
        disabled={disabled}
        pending={pending}
        modelsPending={modelsPending}
        usePending={usePending}
        modelDraft={modelDraft}
        modelError={modelError}
        onModelDraftChange={onModelDraftChange}
        onAddModelById={onAddModelById}
        onUseModel={onUseModel}
        onEditModel={onEditModel}
        onRefreshModel={onRefreshModel}
        onRemoveModel={onRemoveModel}
      />
    </article>
  );
}
