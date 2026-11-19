import type { JSX } from 'preact';
import type { ModelEntry, ProviderProfile } from '../../api/types';
import { EditIcon, RefreshIcon } from '../../components/icons';
import { Badge, Button, Chip, Input, Text } from '../../ui';
import { modelMetaParts } from './providerHelpers';

export interface ProviderModelLibraryProps {
  profile: ProviderProfile;
  selectedModelId: string | null;
  disabled: boolean;
  pending: Record<string, boolean>;
  modelsPending: boolean;
  usePending: boolean;
  modelDraft: string;
  modelError?: string;
  onModelDraftChange: (value: string) => void;
  onAddModelById: () => void;
  onUseModel: (modelId: string) => void;
  onEditModel: (model: ModelEntry) => void;
  onRefreshModel: (model: ModelEntry) => void;
  onRemoveModel: (modelId: string) => void;
}

export function ProviderModelLibrary({
  profile,
  selectedModelId,
  disabled,
  pending,
  modelsPending,
  usePending,
  modelDraft,
  modelError,
  onModelDraftChange,
  onAddModelById,
  onUseModel,
  onEditModel,
  onRefreshModel,
  onRemoveModel,
}: ProviderModelLibraryProps): JSX.Element {
  return (
    <div class="model-library">
      <div class="model-library-head">
        <span class="eyebrow">Model library</span>
        {profile.models.length === 0 ? (
          <Text as="span" class="field-hint">
            No models yet — add one below or browse models.dev.
          </Text>
        ) : null}
      </div>
      {profile.models.length > 0 ? (
        <ul class="model-list" aria-label={`Models on ${profile.name}`}>
          {profile.models.map((model) => {
            const using = selectedModelId === model.id;
            const label = model.name?.trim() ? model.name : model.id;
            const meta = modelMetaParts(model);
            const refreshPending = pending[`${profile.id}:refresh:${model.id}`] === true;
            return (
              <li class="model-row" key={model.id}>
                <div class="model-row-main">
                  <Text as="span" class="model-name">
                    {label}
                  </Text>
                  {label !== model.id ? <code class="model-id">{model.id}</code> : null}
                  {using ? <Badge tone="success">in use</Badge> : null}
                  {model.catalog_provider ? (
                    <Chip tone="neutral">via {model.catalog_provider}</Chip>
                  ) : null}
                </div>
                <div class="model-meta">
                  {meta.length > 0 ? <span>{meta.join(' · ')}</span> : <span>metadata unknown</span>}
                  {model.tool_call ? <Chip>tools</Chip> : null}
                  {model.reasoning ? <Chip>reasoning</Chip> : null}
                  {model.vision === true ? (
                    <Chip tone="success">vision</Chip>
                  ) : model.vision === false ? (
                    <Chip tone="neutral">no images</Chip>
                  ) : (
                    <Chip tone="neutral">images: default</Chip>
                  )}
                </div>
                <div class="model-actions">
                  <Button
                    size="sm"
                    disabled={disabled || usePending || using}
                    aria-label={
                      using ? `In use: ${label} on ${profile.name}` : `Use ${label} on ${profile.name}`
                    }
                    onClick={() => onUseModel(model.id)}
                  >
                    {using ? 'In use' : 'Use'}
                  </Button>
                  <Button
                    size="sm"
                    icon={<EditIcon size={12} />}
                    disabled={disabled || modelsPending}
                    aria-label={`Edit ${label} on ${profile.name}`}
                    onClick={() => onEditModel(model)}
                  >
                    Edit
                  </Button>
                  <Button
                    size="sm"
                    loading={refreshPending}
                    icon={<RefreshIcon size={12} />}
                    disabled={disabled || modelsPending || refreshPending}
                    aria-label={`Refresh metadata for ${label} on ${profile.name}`}
                    title="Re-read this model's metadata from models.dev (routing prefixes are stripped to find it)."
                    onClick={() => onRefreshModel(model)}
                  >
                    {refreshPending ? 'Refreshing…' : 'Refresh'}
                  </Button>
                  <Button
                    size="sm"
                    variant="danger-ghost"
                    disabled={disabled || modelsPending}
                    aria-label={`Remove ${label} from ${profile.name}`}
                    onClick={() => onRemoveModel(model.id)}
                  >
                    Remove
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      ) : null}
      <div class="model-add">
        <Text as="label" htmlFor={`model-add-${profile.id}`} class="field-label">
          Add a model id
        </Text>
        <div class="model-add-row">
          <Input
            id={`model-add-${profile.id}`}
            mono
            type="text"
            class="flex-1 min-w-0"
            placeholder="qwen3-32b"
            value={modelDraft}
            disabled={disabled || modelsPending}
            onInput={(event) => onModelDraftChange(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                onAddModelById();
              }
            }}
          />
          <Button
            size="sm"
            disabled={disabled || modelsPending || modelDraft.trim() === ''}
            aria-label={`Add model id to ${profile.name}`}
            onClick={onAddModelById}
          >
            Add
          </Button>
        </div>
        {modelError ? (
          <p class="field-error" role="alert">
            {modelError}
          </p>
        ) : null}
      </div>
    </div>
  );
}
