/** One model the user keeps available on a provider profile. */
export interface ModelEntry {
  id: string;
  name?: string | null;
  context_window?: number | null;
  max_output_tokens?: number | null;
  tool_call?: boolean | null;
  reasoning?: boolean | null;
  /**
   * Whether the model accepts **image** input, so `read_file` may show it a
   * PNG/JPEG/GIF/WebP. `null`/absent means "not declared": the install-wide
   * `model.supports_images` default applies. Prefilled from models.dev
   * (`modalities.input` containing `image`) and editable, because a gateway may
   * serve a vision model under a text-only id or the other way round.
   */
  vision?: boolean | null;
  cost_input?: number | null;
  cost_output?: number | null;
  /** models.dev provider the metadata came from, when prefilled. */
  catalog_provider?: string | null;
  /**
   * The reasoning levels this model accepts, each one a map of request-body keys
   * sent for a run pinned to that level.
   *
   * Declaring variants is what lets a level mean "32k thinking tokens" instead of
   * a bare OpenAI-shaped `reasoning_effort` string, and what removes the levels
   * the endpoint does not implement: present (even as `{}`) means *only* these
   * levels are offered and accepted. Absent means "not declared", and every
   * built-in level stays available and is sent as `reasoning_effort: "<level>"`.
   */
  variants?: Record<string, Record<string, unknown>> | null;
}

/** A named OpenAI-compatible endpoint plus its curated model list. */
export interface ProviderProfile {
  id: string;
  name: string;
  provider: string;
  base_url: string;
  models: ModelEntry[];
  request_timeout_secs: number;
  /**
   * The endpoint fronts several upstream channels, so a 401/403/404 means "this
   * channel's key is dead" and is worth another attempt.
   */
  aggregator: boolean;
  /** The key itself is write-only and never sent to the browser. */
  api_key_set: boolean;
}

export interface ActiveSelection {
  provider: string | null;
  model: string | null;
}

export interface ProviderPatch {
  id: string;
  name?: string;
  provider?: string;
  base_url?: string;
  /** Write-only; empty strings are ignored. */
  api_key?: string;
  clear_api_key?: boolean;
  /** Replaces the whole list when present. */
  models?: ModelEntry[];
  request_timeout_secs?: number;
  /** Retry 401/403/404: the next attempt lands on another upstream channel. */
  aggregator?: boolean;
}

export interface ProvidersPatch {
  upsert?: ProviderPatch[];
  delete?: string[];
}

export interface ActivePatch {
  provider?: string;
  model?: string;
}

/** One models.dev model, normalized by the server for prefilling. */
export interface CatalogModel {
  id: string;
  name: string;
  context_window?: number;
  max_output_tokens?: number;
  tool_call?: boolean;
  reasoning?: boolean;
  /**
   * Whether models.dev says this model takes **image** input. Present only when
   * the document answers: a model whose inputs are `audio` or `pdf` alone is
   * `false` even though it is an "attachment" model, because handing it a PNG
   * fails the whole turn.
   */
  vision?: boolean;
  cost_input?: number;
  cost_output?: number;
}

export interface CatalogProviderSummary {
  id: string;
  name: string;
  model_count: number;
}

export interface CatalogResponse {
  ok: boolean;
  providers: CatalogProviderSummary[];
  provider?: CatalogProviderSummary | null;
  models: CatalogModel[];
  fetched_at_ms: number;
  /** True when an unreachable catalog served a cached copy. */
  stale: boolean;
  error: string | null;
}

/** The best catalog entry for an arbitrary (possibly gateway-prefixed) id. */
export interface CatalogLookupMatch {
  /** models.dev provider id the metadata came from. */
  provider: string;
  /** The suffix of the queried id that matched; equals `model.id`. */
  matched_id: string;
  model: CatalogModel;
}

export interface CatalogLookupResponse {
  ok: boolean;
  /** Null when the catalog does not carry the model (an ordinary miss). */
  match: CatalogLookupMatch | null;
  fetched_at_ms: number;
  stale: boolean;
  error: string | null;
}

export interface ProviderTestRequest {
  profile?: string;
  base_url?: string;
  api_key?: string;
  model?: string;
  request_timeout_secs?: number;
}

export interface PublicModelConfig {
  provider: string;
  base_url: string;
  model: string;
  temperature: number;
  /** Whether a temperature is sent at all (off by default). */
  send_temperature: boolean;
  max_tokens: number | null;
  top_p: number | null;
  request_timeout_secs: number;
  parallel_tool_calls: boolean;
  /**
   * Install-wide default: may `read_file` show the model an image? A
   * `[[providers.models]] vision` entry overrides it per model.
   */
  supports_images: boolean;
  api_key_set: boolean;
}

export interface ModelConfigPatch {
  provider?: string;
  base_url?: string;
  model?: string;
  temperature?: number;
  send_temperature?: boolean;
  max_tokens?: number | null;
  top_p?: number | null;
  request_timeout_secs?: number;
  parallel_tool_calls?: boolean;
  /** Install-wide default for image input; a per-model `vision` wins. */
  supports_images?: boolean;
  /** Write-only. Never rendered back. */
  api_key?: string | null;
  clear_api_key?: boolean;
}

export interface ProviderTestResult {
  ok: boolean;
  models?: string[];
  error: string | null;
  latency_ms: number;
}
