import { apiRequest, withQuery } from '../client';
import type {
  CatalogLookupResponse,
  CatalogResponse,
  ProviderTestRequest,
  ProviderTestResult,
} from '../types/providers';
import type { CallOptions } from './common';

export function testProvider(
  body: ProviderTestRequest = {},
  options: CallOptions = {},
): Promise<ProviderTestResult> {
  return apiRequest<ProviderTestResult>('/api/provider/test', {
    ...options,
    method: 'POST',
    body,
  });
}

/** models.dev metadata: provider list, or one provider's models. */
export function getModelsCatalog(
  params: { provider?: string; refresh?: boolean } = {},
  options: CallOptions = {},
): Promise<CatalogResponse> {
  return apiRequest<CatalogResponse>(
    withQuery('/api/models/catalog', {
      provider: params.provider,
      refresh: params.refresh ? 'true' : undefined,
    }),
    options,
  );
}

/**
 * models.dev metadata for one model id. Gateway routing prefixes
 * (`a6api/model`, `a6api/upstream/model`) are stripped server-side until a
 * suffix matches; `match` is null when the catalog does not carry the model.
 */
export function lookupModelMetadata(
  model: string,
  options: CallOptions = {},
): Promise<CatalogLookupResponse> {
  return apiRequest<CatalogLookupResponse>(
    withQuery('/api/models/lookup', { model }),
    options,
  );
}
