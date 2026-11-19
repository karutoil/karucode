import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type {
  CatalogLookupResponse,
  CatalogResponse,
  Config,
  ConfigPatch,
  ProviderTestRequest,
  ProviderTestResult,
  UpdateApplyResponse,
  UpdateStatus,
} from '../../api/types';
import * as store from '../store';
import { enterAuthState, isUnauthorized, rt } from './shared';

export async function checkHealth(): Promise<boolean> {
  const generation = ++rt.healthGeneration;
  try {
    const result = await api.getHealth();
    if (generation !== rt.healthGeneration) return false;
    store.health.value = result;
    store.connection.value = {
      ...store.connection.value,
      api: 'online',
      lastError: null,
      checkedAt: Date.now(),
    };
    return true;
  } catch (error) {
    if (generation !== rt.healthGeneration) return false;
    if (isUnauthorized(error)) {
      enterAuthState();
      return false;
    }
    store.connection.value = {
      ...store.connection.value,
      api: 'offline',
      lastError: toErrorMessage(error),
      checkedAt: Date.now(),
    };
    return false;
  }
}

export async function loadConfig(): Promise<Config> {
  const result = await api.getConfig();
  store.config.value = result;
  return result;
}

export async function saveConfig(patch: ConfigPatch): Promise<Config> {
  const result = await api.patchConfig(patch);
  store.config.value = result;
  store.pushToast('Settings saved.', 'success');
  void checkHealth();
  return result;
}

/**
 * Apply a config patch without a toast.
 *
 * The onboarding wizard saves several steps in a row; five "Settings saved."
 * toasts would bury the one message that matters. It still refreshes the health
 * probe, because a provider key change moves the connection badge.
 */
export async function applyConfig(patch: ConfigPatch): Promise<Config> {
  const result = await api.patchConfig(patch);
  store.config.value = result;
  void checkHealth();
  return result;
}

export async function clearApiKey(): Promise<Config> {
  const result = await api.patchConfig({ model: { api_key: null, clear_api_key: true } });
  store.config.value = result;
  store.pushToast('API key cleared.', 'info');
  void checkHealth();
  return result;
}

export function testProvider(body: ProviderTestRequest = {}): Promise<ProviderTestResult> {
  return api.testProvider(body);
}

/** Save provider profiles and/or select the active provider+model. */
export async function saveProviders(
  patch: Pick<ConfigPatch, 'providers' | 'active'>,
  options: { toast?: string | null } = {},
): Promise<Config | null> {
  try {
    const result = await api.patchConfig(patch);
    store.config.value = result;
    if (options.toast !== null) store.pushToast(options.toast ?? 'Providers saved.', 'success');
    void checkHealth();
    return result;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return null;
    }
    store.pushToast(toErrorMessage(error));
    return null;
  }
}

/** Switch the active model (optionally on another provider) for every project. */
export async function switchModel(providerId: string, modelId: string): Promise<boolean> {
  const current = store.config.value;
  if (current?.active.provider === providerId && current.active.model === modelId) return true;
  const result = await saveProviders(
    { active: { provider: providerId, model: modelId } },
    { toast: null },
  );
  if (!result) return false;
  const provider = result.providers.find((profile) => profile.id === providerId);
  const label = provider?.models.find((model) => model.id === modelId)?.name ?? modelId;
  store.pushToast(`Now using ${label}.`, 'success');
  return true;
}

/** models.dev catalog: provider list or one provider's models. */
export async function loadCatalog(
  provider?: string,
  refresh = false,
): Promise<CatalogResponse | null> {
  try {
    return await api.getModelsCatalog({ provider, refresh });
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return null;
  }
}

/**
 * models.dev metadata for one model id, gateway prefixes stripped server-side.
 * Best-effort: any failure returns null and the caller adds the id as-is.
 */
export async function lookupModel(model: string): Promise<CatalogLookupResponse | null> {
  try {
    return await api.lookupModelMetadata(model);
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    return null;
  }
}

/**
 * Refresh `GET /api/update` into the store.
 *
 * Best-effort by design: an older server has no `/api/update` (a `404`), and a
 * release host being unreachable is not a reason to show an error anywhere but
 * inside Settings → Updates. The caller gets `null` and draws nothing.
 */
export async function refreshUpdateStatus(): Promise<UpdateStatus | null> {
  try {
    const status = await api.getUpdate();
    store.updateStatus.value = status;
    return status;
  } catch {
    return null;
  }
}

/**
 * `POST /api/update/check`: ask the release host now.
 *
 * Unlike the boot read this one *throws* on failure: the person clicked a button,
 * and a check that silently did nothing is the bug this exists to avoid. The
 * server also records the reason in the status, so the panel shows both.
 */
export async function runUpdateCheck(): Promise<UpdateStatus> {
  const status = await api.checkForUpdate();
  store.updateStatus.value = status;
  return status;
}

/**
 * `POST /api/update/apply`: stage if needed, then install.
 *
 * The response carries the new status, so the panel and the rail update from one
 * round trip. A refusal (source tree, not writable, `403` without the admin token)
 * is thrown as an `ApiError` with the server's message.
 */
export async function installUpdate(restart = false): Promise<UpdateApplyResponse> {
  const response = await api.applyUpdate({ restart });
  store.updateStatus.value = response.update;
  return response;
}
