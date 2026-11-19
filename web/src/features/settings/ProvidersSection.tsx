import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import type { ConfigPatch, ModelEntry, ProviderPatch, ProviderProfile } from '../../api/types';
import { PlusIcon } from '../../components/icons';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { Button, Callout, ConfirmDialog, Text } from '../../ui';
import { AddProviderForm } from './AddProviderForm';
import { CatalogPickerDialog } from './ModelCatalogModal';
import { ModelEditDialog } from './ModelEditDialog';
import { ProviderCard } from './ProviderCard';
import {
  DEFAULT_PROVIDER_ID,
  MAX_MODELS_PER_PROFILE,
  applyRefreshedMetadata,
  baseUrlError,
  deriveProfileId,
  emptyDraft,
  modelEntryFromLookup,
  providerIdError,
} from './providerHelpers';
import type { AddProviderDraft, TestState } from './types';

export function ProvidersSection({ disabled }: { disabled: boolean }): JSX.Element | null {
  const config = store.config.value;
  const [pending, setPending] = useState<Record<string, boolean>>({});
  const [tests, setTests] = useState<Record<string, TestState>>({});
  const [actionError, setActionError] = useState<string | null>(null);
  const [keyEditor, setKeyEditor] = useState<string | null>(null);
  const [keyDraft, setKeyDraft] = useState('');
  const [confirmClearKey, setConfirmClearKey] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [modelDrafts, setModelDrafts] = useState<Record<string, string>>({});
  const [modelErrors, setModelErrors] = useState<Record<string, string>>({});
  const [catalogFor, setCatalogFor] = useState<ProviderProfile | null>(null);
  const [editingModel, setEditingModel] = useState<{ profile: ProviderProfile; model: ModelEntry } | null>(
    null,
  );
  const [showAdd, setShowAdd] = useState(false);
  const [draft, setDraft] = useState<AddProviderDraft>(emptyDraft);
  const [draftErrors, setDraftErrors] = useState<Record<string, string>>({});

  if (!config) return null;

  const profiles = config.providers;
  const active = config.active;
  const supportedProviders = config.available_providers?.length
    ? config.available_providers
    : profiles.length > 0
      ? [profiles[0]?.provider ?? DEFAULT_PROVIDER_ID]
      : [DEFAULT_PROVIDER_ID];
  const defaultProvider = supportedProviders[0] ?? DEFAULT_PROVIDER_ID;
  const takenIds = profiles.map((profile) => profile.id);
  const derivedId = deriveProfileId(draft.name, draft.baseUrl, takenIds);

  const markPending = (key: string, value: boolean): void => {
    setPending((current) => ({ ...current, [key]: value }));
  };

  const savePatch = async (
    patch: Pick<ConfigPatch, 'providers' | 'active'>,
    toast: string | null,
  ): Promise<boolean> => {
    setActionError(null);
    const result = await actions.saveProviders(patch, { toast });
    if (!result) {
      setActionError('The server rejected that change. Check the connection and try again.');
      return false;
    }
    return true;
  };

  const runTest = async (profile: ProviderProfile): Promise<void> => {
    markPending(`${profile.id}:test`, true);
    setTests((current) => ({ ...current, [profile.id]: { running: true, result: null, error: null } }));
    try {
      const result = await actions.testProvider({ profile: profile.id });
      setTests((current) => ({
        ...current,
        [profile.id]: { running: false, result, error: null },
      }));
    } catch (cause) {
      setTests((current) => ({
        ...current,
        [profile.id]: { running: false, result: null, error: toErrorMessage(cause) },
      }));
    } finally {
      markPending(`${profile.id}:test`, false);
    }
  };

  const setActive = async (profile: ProviderProfile): Promise<void> => {
    const currentModel = active.provider === profile.id ? active.model : null;
    const model = currentModel ?? profile.models[0]?.id ?? '';
    markPending(`${profile.id}:active`, true);
    const ok = await savePatch(
      { active: { provider: profile.id, model } },
      `Now using ${profile.name}.`,
    );
    markPending(`${profile.id}:active`, false);
    if (!ok) setActionError(`Could not switch to ${profile.name}.`);
  };

  const saveKey = async (profile: ProviderProfile): Promise<void> => {
    const key = keyDraft.trim();
    if (key === '') return;
    markPending(`${profile.id}:key`, true);
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, api_key: key }] } },
      `API key saved for ${profile.name}.`,
    );
    markPending(`${profile.id}:key`, false);
    if (ok) {
      setKeyEditor(null);
      setKeyDraft('');
    }
  };

  const clearKey = async (profile: ProviderProfile): Promise<void> => {
    markPending(`${profile.id}:key`, true);
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, clear_api_key: true }] } },
      `API key cleared for ${profile.name}.`,
    );
    markPending(`${profile.id}:key`, false);
    if (ok) setConfirmClearKey(null);
  };

  /// A 401/403/404 is permanent on a direct vendor API and transient behind a
  /// gateway that fronts several upstream channels: the next attempt lands on a
  /// different one. That is a property of the endpoint, so it is a switch on the
  /// profile, not a global retry setting.
  const toggleAggregator = async (profile: ProviderProfile): Promise<void> => {
    const next = !profile.aggregator;
    markPending(`${profile.id}:aggregator`, true);
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, aggregator: next }] } },
      next
        ? `${profile.name} now retries a 401/403/404 on another channel.`
        : `${profile.name} now stops at the first 401/403/404.`,
    );
    markPending(`${profile.id}:aggregator`, false);
    if (!ok) setActionError(`Could not change how ${profile.name} treats auth errors.`);
  };

  const removeModel = async (profile: ProviderProfile, modelId: string): Promise<void> => {
    const models = profile.models.filter((model) => model.id !== modelId);
    const patch: Pick<ConfigPatch, 'providers' | 'active'> = {
      providers: { upsert: [{ id: profile.id, models }] },
    };
    // The server refuses an active model that is missing from its profile, so
    // move the selection in the same request instead of failing the removal.
    if (active.provider === profile.id && active.model === modelId) {
      patch.active = { provider: profile.id, model: models[0]?.id ?? '' };
    }
    markPending(`${profile.id}:models`, true);
    await savePatch(patch, `Removed ${modelId} from ${profile.name}.`);
    markPending(`${profile.id}:models`, false);
  };

  const addModelById = async (profile: ProviderProfile): Promise<void> => {
    const modelId = (modelDrafts[profile.id] ?? '').trim();
    if (modelId === '') return;
    if (profile.models.some((model) => model.id === modelId)) {
      setModelErrors((current) => ({ ...current, [profile.id]: `${modelId} is already in this profile.` }));
      return;
    }
    if (profile.models.length >= MAX_MODELS_PER_PROFILE) {
      setModelErrors((current) => ({
        ...current,
        [profile.id]: `A profile can hold at most ${MAX_MODELS_PER_PROFILE} models.`,
      }));
      return;
    }
    setModelErrors((current) => ({ ...current, [profile.id]: '' }));
    markPending(`${profile.id}:models`, true);
    // Best-effort metadata detection: the server strips gateway routing
    // prefixes (a6api/model, a6api/upstream/model) until a models.dev entry
    // matches. Any failure just adds the bare id, as before.
    const lookup = await actions.lookupModel(modelId);
    const detected = lookup?.ok ? lookup.match : null;
    const entry = modelEntryFromLookup(modelId, detected);
    const entries: ModelEntry[] = [...profile.models, entry];
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, models: entries }] } },
      detected
        ? `Added ${modelId} to ${profile.name} (metadata via ${detected.provider}).`
        : `Added ${modelId} to ${profile.name}.`,
    );
    markPending(`${profile.id}:models`, false);
    if (ok) setModelDrafts((current) => ({ ...current, [profile.id]: '' }));
  };

  const refreshModel = async (profile: ProviderProfile, model: ModelEntry): Promise<void> => {
    const key = `${profile.id}:refresh:${model.id}`;
    markPending(key, true);
    try {
      const lookup = await actions.lookupModel(model.id);
      const detected = lookup?.ok ? lookup.match : null;
      if (!detected) {
        store.pushToast(
          lookup && !lookup.ok
            ? `Could not refresh ${model.id}: ${lookup.error ?? 'the catalog is unavailable'}`
            : `No models.dev entry matches ${model.id}; its metadata is unchanged.`,
          'info',
        );
        return;
      }
      // Refresh metadata only: a custom display name stays the user's.
      const models = profile.models.map((entry) =>
        entry.id === model.id ? applyRefreshedMetadata(entry, detected) : entry,
      );
      await savePatch(
        { providers: { upsert: [{ id: profile.id, models }] } },
        `Refreshed metadata for ${model.id} (via ${detected.provider}).`,
      );
    } finally {
      markPending(key, false);
    }
  };

  const saveEditedModel = async (entry: ModelEntry): Promise<boolean> => {
    const editing = editingModel;
    if (!editing) return false;
    const models = editing.profile.models.map((model) => (model.id === entry.id ? entry : model));
    return savePatch(
      { providers: { upsert: [{ id: editing.profile.id, models }] } },
      `Saved ${entry.name?.trim() || entry.id}.`,
    );
  };

  const useModel = async (profile: ProviderProfile, modelId: string): Promise<void> => {
    markPending(`${profile.id}:use`, true);
    const ok = await actions.switchModel(profile.id, modelId);
    markPending(`${profile.id}:use`, false);
    if (!ok) setActionError(`Could not switch to ${modelId}.`);
  };

  const addCatalogModels = async (profile: ProviderProfile, entries: ModelEntry[]): Promise<boolean> => {
    const merged = [...profile.models];
    for (const entry of entries) {
      if (!merged.some((model) => model.id === entry.id)) merged.push(entry);
    }
    if (merged.length === profile.models.length) return true;
    const capped = merged.slice(0, MAX_MODELS_PER_PROFILE);
    const added = capped.length - profile.models.length;
    const ok = await savePatch(
      { providers: { upsert: [{ id: profile.id, models: capped }] } },
      `Added ${added} model${added === 1 ? '' : 's'} to ${profile.name}.`,
    );
    return ok;
  };

  const removeProfile = async (profile: ProviderProfile): Promise<void> => {
    const remaining = profiles.filter((candidate) => candidate.id !== profile.id);
    const next = remaining[0];
    const payload: Pick<ConfigPatch, 'providers' | 'active'> = {
      providers: { delete: [profile.id] },
    };
    if (active.provider === profile.id && next) {
      payload.active = { provider: next.id, model: next.models[0]?.id ?? '' };
    }
    markPending(`${profile.id}:remove`, true);
    const ok = await savePatch(payload, `Removed ${profile.name}.`);
    markPending(`${profile.id}:remove`, false);
    if (ok) setConfirmRemove(null);
  };

  const addProvider = async (): Promise<void> => {
    const errors: Record<string, string> = {};
    const urlError = baseUrlError(draft.baseUrl);
    if (urlError) errors.baseUrl = urlError;
    const id = draft.id.trim() === '' ? derivedId : draft.id.trim();
    const idError = providerIdError(id, takenIds);
    if (idError) errors.id = idError;
    setDraftErrors(errors);
    if (Object.keys(errors).length > 0) return;

    const patch: ProviderPatch = {
      id,
      base_url: draft.baseUrl.trim(),
      provider: draft.provider || defaultProvider,
    };
    const name = draft.name.trim();
    if (name !== '') patch.name = name;
    const apiKey = draft.apiKey.trim();
    if (apiKey !== '') patch.api_key = apiKey;

    const payload: Pick<ConfigPatch, 'providers' | 'active'> = { providers: { upsert: [patch] } };
    // The first profile becomes the active one; the server requires an explicit
    // selection whenever profiles exist.
    if (profiles.length === 0) payload.active = { provider: id, model: '' };

    markPending('add', true);
    const ok = await savePatch(payload, `Added ${name === '' ? id : name}.`);
    markPending('add', false);
    if (ok) {
      setDraft(emptyDraft());
      setDraftErrors({});
      setShowAdd(false);
    }
  };

  const clearKeyTarget = confirmClearKey
    ? profiles.find((profile) => profile.id === confirmClearKey) ?? null
    : null;
  const removeTarget = confirmRemove
    ? profiles.find((profile) => profile.id === confirmRemove) ?? null
    : null;

  return (
    <section class="settings-section" aria-labelledby="settings-providers">
      <div class="settings-section-head">
        <h3 id="settings-providers">Providers</h3>
        <div class="settings-section-actions">
          <span class="settings-section-note">OpenAI-compatible endpoints and their model libraries</span>
          {profiles.length > 0 ? (
            <Button
              size="sm"
              icon={<PlusIcon size={12} />}
              aria-expanded={showAdd}
              onClick={() => {
                setDraftErrors({});
                setShowAdd((current) => !current);
              }}
            >
              Add provider
            </Button>
          ) : null}
        </div>
      </div>

      {actionError ? (
        <Callout tone="danger" role="alert">
          {actionError}
        </Callout>
      ) : null}

      {profiles.length === 0 ? (
        <Text as="p" class="field-hint">
          No providers configured yet. Add one below, then pick its models — manually or from models.dev.
        </Text>
      ) : null}

      <div class="providers-list">
        {profiles.map((profile) => {
          const isActive = active.provider === profile.id;
          const selectedModelId = isActive ? active.model ?? profile.models[0]?.id ?? null : null;
          return (
            <ProviderCard
              key={profile.id}
              profile={profile}
              isActive={isActive}
              selectedModelId={selectedModelId}
              test={tests[profile.id]}
              disabled={disabled}
              pending={pending}
              keyEditorOpen={keyEditor === profile.id}
              keyDraft={keyDraft}
              modelDraft={modelDrafts[profile.id] ?? ''}
              modelError={modelErrors[profile.id]}
              onRunTest={() => void runTest(profile)}
              onSetActive={() => void setActive(profile)}
              onToggleAggregator={() => void toggleAggregator(profile)}
              onToggleKeyEditor={() => {
                setConfirmClearKey(null);
                setConfirmRemove(null);
                setKeyDraft('');
                setKeyEditor((current) => (current === profile.id ? null : profile.id));
              }}
              onKeyDraftChange={setKeyDraft}
              onSaveKey={() => void saveKey(profile)}
              onCancelKeyEditor={() => {
                setKeyEditor(null);
                setKeyDraft('');
              }}
              onRequestClearKey={() => {
                setKeyEditor(null);
                setConfirmRemove(null);
                setConfirmClearKey(profile.id);
              }}
              onOpenCatalog={() => setCatalogFor(profile)}
              onRequestRemove={() => {
                setConfirmClearKey(null);
                setKeyEditor(null);
                setConfirmRemove(profile.id);
              }}
              onModelDraftChange={(value) =>
                setModelDrafts((current) => ({ ...current, [profile.id]: value }))
              }
              onAddModelById={() => void addModelById(profile)}
              onUseModel={(modelId) => void useModel(profile, modelId)}
              onEditModel={(model) => setEditingModel({ profile, model })}
              onRefreshModel={(model) => void refreshModel(profile, model)}
              onRemoveModel={(modelId) => void removeModel(profile, modelId)}
            />
          );
        })}
      </div>

      {showAdd || profiles.length === 0 ? (
        <AddProviderForm
          draft={draft}
          draftErrors={draftErrors}
          derivedId={derivedId}
          supportedProviders={supportedProviders}
          defaultProvider={defaultProvider}
          disabled={disabled}
          adding={pending['add'] === true}
          canCancel={profiles.length > 0}
          onDraftChange={setDraft}
          onCancel={() => setShowAdd(false)}
          onAdd={() => void addProvider()}
        />
      ) : null}

      {clearKeyTarget ? (
        <ConfirmDialog
          open
          tone="danger"
          title={`Clear API key for ${clearKeyTarget.name}?`}
          description="Requests using this profile will fail until a new key is saved."
          confirmLabel="Clear key"
          busy={pending[`${clearKeyTarget.id}:key`] === true}
          onClose={() => setConfirmClearKey(null)}
          onConfirm={() => void clearKey(clearKeyTarget)}
        />
      ) : null}

      {removeTarget ? (
        <ConfirmDialog
          open
          tone="danger"
          title={`Remove provider ${removeTarget.name}?`}
          description="This deletes the profile and its model list from your configuration."
          confirmLabel="Remove profile"
          busy={pending[`${removeTarget.id}:remove`] === true}
          onClose={() => setConfirmRemove(null)}
          onConfirm={() => void removeProfile(removeTarget)}
        />
      ) : null}

      {catalogFor ? (
        <CatalogPickerDialog
          profile={catalogFor}
          existingIds={catalogFor.models.map((model) => model.id)}
          onAdd={(entries) => addCatalogModels(catalogFor, entries)}
          onClose={() => setCatalogFor(null)}
        />
      ) : null}

      {editingModel ? (
        <ModelEditDialog
          profile={editingModel.profile}
          model={editingModel.model}
          onSave={saveEditedModel}
          onClose={() => setEditingModel(null)}
        />
      ) : null}
    </section>
  );
}
