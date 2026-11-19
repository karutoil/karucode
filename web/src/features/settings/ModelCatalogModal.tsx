import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { CatalogModel, CatalogProviderSummary, ModelEntry, ProviderProfile } from '../../api/types';
import { RefreshIcon } from '../../components/icons';
import * as actions from '../../state/actions';
import { Button, Callout, Chip, Dialog, Input, Skeleton, Text } from '../../ui';
import { catalogModelMeta } from './providerHelpers';
import { Field } from './SharedControls';

export interface CatalogPickerProps {
  profile: ProviderProfile;
  existingIds: string[];
  onAdd: (entries: ModelEntry[]) => Promise<boolean>;
  onClose: () => void;
}

/** Two-step models.dev browser: provider list, then a searchable model checklist. */
export function CatalogPickerDialog({ profile, existingIds, onAdd, onClose }: CatalogPickerProps): JSX.Element {
  const [stage, setStage] = useState<'providers' | 'models'>('providers');
  const [providers, setProviders] = useState<CatalogProviderSummary[]>([]);
  const [provider, setProvider] = useState<CatalogProviderSummary | null>(null);
  const [models, setModels] = useState<CatalogModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  const loadProviders = async (refresh: boolean): Promise<void> => {
    setLoading(true);
    setError(null);
    setNotice(null);
    const response = await actions.loadCatalog(undefined, refresh);
    setLoading(false);
    if (!response) {
      setError('Could not reach the models.dev catalog. You can still add model ids by hand.');
      return;
    }
    setProviders(response.providers);
    if (!response.ok) {
      setError(response.error ?? 'The models.dev catalog is unavailable. You can still add model ids by hand.');
    } else {
      setNotice(response.error ? `Showing a cached catalog: ${response.error}` : response.stale ? 'Showing a cached catalog.' : null);
    }
  };

  useEffect(() => {
    void loadProviders(false);
    // The picker is mounted fresh per profile; load once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const chooseProvider = async (summary: CatalogProviderSummary): Promise<void> => {
    setStage('models');
    setProvider(summary);
    setModels([]);
    setSelected([]);
    setQuery('');
    setLoading(true);
    setError(null);
    setNotice(null);
    const response = await actions.loadCatalog(summary.id, false);
    setLoading(false);
    if (!response) {
      setError('Could not load models for this provider. You can still add model ids by hand.');
      return;
    }
    setModels(response.models);
    setProvider(response.provider ?? summary);
    if (!response.ok) {
      setError(response.error ?? 'The models.dev catalog is unavailable for this provider.');
    } else if (response.error) {
      setNotice(`Showing a cached catalog: ${response.error}`);
    }
  };

  const toggle = (id: string): void => {
    setSelected((current) =>
      current.includes(id) ? current.filter((candidate) => candidate !== id) : [...current, id],
    );
  };

  const submit = async (): Promise<void> => {
    const chosen = models.filter((model) => selected.includes(model.id) && !existingIds.includes(model.id));
    if (chosen.length === 0) return;
    const entries: ModelEntry[] = chosen.map((model) => ({
      id: model.id,
      name: model.name.trim() === '' ? null : model.name.trim(),
      context_window: model.context_window ?? null,
      max_output_tokens: model.max_output_tokens ?? null,
      tool_call: model.tool_call ?? null,
      reasoning: model.reasoning ?? null,
      // models.dev is the source of truth for "does this model take images", so
      // the picker prefills the per-model flag instead of making the user find it.
      vision: model.vision ?? null,
      cost_input: model.cost_input ?? null,
      cost_output: model.cost_output ?? null,
      catalog_provider: provider?.id ?? null,
    }));
    setSaving(true);
    const ok = await onAdd(entries);
    setSaving(false);
    if (ok) onClose();
  };

  const needle = query.trim().toLowerCase();
  const visibleProviders = providers.filter(
    (candidate) =>
      needle === '' ||
      candidate.name.toLowerCase().includes(needle) ||
      candidate.id.toLowerCase().includes(needle),
  );
  const visibleModels = models.filter(
    (candidate) =>
      needle === '' ||
      candidate.name.toLowerCase().includes(needle) ||
      candidate.id.toLowerCase().includes(needle),
  );
  const addable = models.filter((model) => selected.includes(model.id) && !existingIds.includes(model.id)).length;

  return (
    <Dialog
      open
      title={`models.dev: ${profile.name}`}
      onClose={onClose}
      class="dialog-catalog"
      labelId="catalog-title"
    >
      <div class="catalog-body">
        {stage === 'providers' ? (
          <div class="catalog-stage">
            <div class="catalog-toolbar">
              <span class="eyebrow">Providers</span>
              <Button
                size="sm"
                loading={loading}
                icon={<RefreshIcon size={12} />}
                disabled={loading}
                onClick={() => void loadProviders(true)}
              >
                Refresh catalog
              </Button>
            </div>
            <Field
              id="catalog-provider-search"
              label="Search providers"
              hint="Pick a models.dev provider to browse its models."
            >
              <Input
                id="catalog-provider-search"
                type="search"
                value={query}
                autoFocus
                placeholder="anthropic"
                aria-describedby="catalog-provider-search-hint"
                onInput={(event) => setQuery(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') event.preventDefault();
                }}
              />
            </Field>
            {loading ? (
              <div class="catalog-skeletons" aria-busy="true" aria-label="Loading catalog providers">
                {[0, 1, 2].map((index) => (
                  <Skeleton class="catalog-skeleton" key={index} />
                ))}
              </div>
            ) : null}
            {error ? (
              <Callout tone="danger" role="alert">
                {error}
              </Callout>
            ) : null}
            {notice ? <p class="catalog-notice">{notice}</p> : null}
            {!loading && visibleProviders.length === 0 && !error ? (
              <Text as="p" class="field-hint">
                No catalog provider matches “{query}”.
              </Text>
            ) : null}
            <ul class="catalog-provider-list">
              {visibleProviders.map((candidate) => (
                <li key={candidate.id}>
                  <button type="button" class="catalog-provider" onClick={() => void chooseProvider(candidate)}>
                    <span class="catalog-provider-name">{candidate.name}</span>
                    <code class="catalog-provider-id">{candidate.id}</code>
                    <span class="catalog-count">
                      {candidate.model_count} model{candidate.model_count === 1 ? '' : 's'}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <div class="catalog-stage">
            <div class="catalog-models-head">
              <Button
                size="sm"
                onClick={() => {
                  setStage('providers');
                  setQuery('');
                  setSelected([]);
                }}
              >
                ← Providers
              </Button>
              <span class="catalog-models-provider">
                {provider?.name ?? provider?.id ?? ''}
                {provider ? ` · ${provider.model_count} models` : ''}
              </span>
            </div>
            <Field
              id="catalog-model-search"
              label="Search models"
              hint="Tick one or more models, then add them to the profile."
            >
              <Input
                id="catalog-model-search"
                type="search"
                value={query}
                autoFocus
                placeholder="qwen3"
                aria-describedby="catalog-model-search-hint"
                onInput={(event) => setQuery(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') event.preventDefault();
                }}
              />
            </Field>
            {loading ? (
              <div class="catalog-skeletons" aria-busy="true" aria-label="Loading catalog models">
                {[0, 1, 2, 3, 4].map((index) => (
                  <Skeleton class="catalog-skeleton" key={index} />
                ))}
              </div>
            ) : null}
            {error ? (
              <Callout tone="danger" role="alert">
                {error}
              </Callout>
            ) : null}
            {notice ? <p class="catalog-notice">{notice}</p> : null}
            {!loading && visibleModels.length === 0 && !error ? (
              <Text as="p" class="field-hint">
                No catalog model matches “{query}”.
              </Text>
            ) : null}
            <ul class="catalog-model-list">
              {visibleModels.map((model) => {
                const added = existingIds.includes(model.id);
                const meta = catalogModelMeta(model);
                return (
                  <li class="catalog-model" key={model.id}>
                    <label class="catalog-model-label">
                      <input
                        type="checkbox"
                        checked={added || selected.includes(model.id)}
                        disabled={added}
                        onChange={() => toggle(model.id)}
                      />
                      <span class="catalog-model-text">
                        <span class="catalog-model-name">{model.name}</span>
                        <code class="catalog-model-id">{model.id}</code>
                        {meta ? <span class="catalog-model-meta">{meta}</span> : null}
                      </span>
                      {added ? <Chip tone="neutral">added</Chip> : null}
                      {!added && model.tool_call ? <Chip>tools</Chip> : null}
                      {!added && model.reasoning ? <Chip>reasoning</Chip> : null}
                      {!added && model.vision ? <Chip tone="success">vision</Chip> : null}
                    </label>
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        <footer class="catalog-footer">
          <Text as="span" class="field-hint">
            Manual ids always work: if the catalog is down, add a model id to the profile directly.
          </Text>
          <div class="catalog-footer-actions">
            <Button size="sm" onClick={onClose}>
              Cancel
            </Button>
            {stage === 'models' ? (
              <Button
                size="sm"
                variant="primary"
                loading={saving}
                disabled={saving || addable === 0}
                onClick={() => void submit()}
              >
                {saving ? 'Adding…' : `Add selected${addable > 0 ? ` (${addable})` : ''}`}
              </Button>
            ) : null}
          </div>
        </footer>
      </div>
    </Dialog>
  );
}
