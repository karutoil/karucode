//! The models.dev catalog: one server-side fetch, cached, exposed per provider.
//!
//! Why server-side? The UI's Content-Security-Policy is `connect-src 'self'`,
//! so the browser cannot call models.dev directly, and the full document is
//! several megabytes — sending it to the settings screen to filter would be
//! worse than filtering here.
//!
//! Behaviour, in order of preference:
//!
//! 1. a fresh cached copy (default TTL [`CACHE_TTL`]) is served without network;
//! 2. a fetch of [`DEFAULT_URL`] (or `KARUCODE_MODELS_DEV_URL`, so an air-gapped
//!    install can point at a mirror or a file served locally);
//! 3. if the fetch fails but a stale copy exists, the stale copy is served and
//!    the error is reported alongside it;
//! 4. otherwise the error is returned and the caller turns it into an
//!    `{ok:false,error}` body. Nothing in the settings path depends on the
//!    catalog being reachable: it only prefills metadata.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Override for tests, mirrors and air-gapped installs.
pub const URL_ENV: &str = "KARUCODE_MODELS_DEV_URL";
pub const DEFAULT_URL: &str = "https://models.dev/api.json";
/// The catalog changes on the order of days; six hours is a good balance.
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Hard cap. The real document is ~6 MB; 32 MiB leaves room for growth without
/// letting a hostile mirror exhaust memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// One model as models.dev describes it, normalized to the metadata the
/// settings screen can prefill.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// Whether this model accepts **image** input.
    ///
    /// Read from `modalities.input` containing `image`, which is the precise
    /// signal. `attachment: true` is not a substitute: it is set for models that
    /// only take `pdf` or `audio` attachments (a whisper or a document model), so
    /// using it alone would offer `read_file` a picture the endpoint rejects.
    /// The legacy `attachment` flag is therefore only consulted as a fallback,
    /// when the document carries no `modalities` at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_output: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    pub model_count: usize,
    #[serde(skip)]
    pub models: Vec<CatalogModel>,
}

/// The parsed document, keyed by models.dev provider id (sorted for stable
/// output).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    providers: BTreeMap<String, CatalogProvider>,
}

/// The best catalog entry for an arbitrary model id, found by [`Catalog::lookup`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogMatch {
    /// models.dev provider id the match came from.
    pub provider: String,
    /// The suffix of the queried id that matched (always equals `model.id`);
    /// shorter than the query when routing prefixes were stripped.
    pub matched_id: String,
    #[serde(flatten)]
    pub model: CatalogModel,
}

impl Catalog {
    pub fn providers(&self) -> impl Iterator<Item = &CatalogProvider> {
        self.providers.values()
    }

    pub fn provider(&self, id: &str) -> Option<&CatalogProvider> {
        self.providers.get(id)
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Find metadata for a model id that may carry gateway routing prefixes.
    ///
    /// Gateways fronting several providers expose ids like `a6api/model` or
    /// `a6api/upstream/model`, while models.dev keys models under its own
    /// provider ids with (usually) bare ids. So the whole id is tried first
    /// (catalog ids can themselves contain slashes, e.g. OpenRouter's
    /// `openai/gpt-4o`), then leading `segment/` prefixes are stripped one at
    /// a time until something matches: longest surviving suffix wins.
    ///
    /// Matching is case-insensitive. When several catalog providers carry the
    /// winning suffix, a provider whose id appears as a segment of the queried
    /// id is preferred (`a6api/azure/gpt-4o` favours the `azure` entry over
    /// the `openai` one); ties break alphabetically so the answer is stable.
    pub fn lookup(&self, model_id: &str) -> Option<CatalogMatch> {
        let query = model_id.trim().to_lowercase();
        if query.is_empty() {
            return None;
        }
        // Candidate suffixes, longest first: the id itself, then everything
        // after the 1st, 2nd, ... slash.
        let mut candidates = vec![query.as_str()];
        candidates.extend(query.match_indices('/').map(|(at, _)| &query[at + 1..]));

        // Segments of the original id, for the provider-id tie-break.
        let segments: Vec<&str> = query.split('/').collect();

        for candidate in candidates {
            if candidate.is_empty() {
                continue;
            }
            let mut hits: Vec<(&CatalogProvider, &CatalogModel)> = self
                .providers
                .values()
                .filter_map(|provider| {
                    provider
                        .models
                        .iter()
                        .find(|model| model.id.eq_ignore_ascii_case(candidate))
                        .map(|model| (provider, model))
                })
                .collect();
            if hits.is_empty() {
                continue;
            }
            // BTreeMap values iterate in id order, so ties are already
            // deterministic; the segment bonus just reorders stably.
            hits.sort_by_key(|(provider, _)| {
                !segments
                    .iter()
                    .any(|segment| provider.id.eq_ignore_ascii_case(segment))
            });
            let (provider, model) = hits[0];
            return Some(CatalogMatch {
                provider: provider.id.clone(),
                matched_id: model.id.clone(),
                model: model.clone(),
            });
        }
        None
    }
}

/// Parse the models.dev document.
///
/// Shape: `{ "<provider>": { "id", "name", "models": { "<model>": { ... } } } }`.
/// Unknown fields are ignored (the upstream schema grows), malformed providers
/// are skipped rather than failing the whole document, and a document with no
/// usable provider is an error so a proxied error page cannot masquerade as an
/// empty catalog.
pub fn parse_catalog(value: &serde_json::Value) -> Result<Catalog, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "models.dev returned a JSON document that is not an object".to_string())?;

    let mut catalog = Catalog::default();
    for (key, raw) in object {
        let Some(entry) = raw.as_object() else {
            continue;
        };
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(key)
            .trim()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(&id)
            .to_string();

        let mut models: Vec<CatalogModel> = Vec::new();
        if let Some(raw_models) = entry.get("models").and_then(|v| v.as_object()) {
            for (model_key, raw_model) in raw_models {
                let Some(model) = raw_model.as_object() else {
                    continue;
                };
                let model_id = model
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or(model_key)
                    .trim()
                    .to_string();
                if model_id.is_empty() {
                    continue;
                }
                let model_name = model
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .unwrap_or(&model_id)
                    .to_string();
                let limit = model.get("limit");
                let context_window = limit
                    .and_then(|l| l.get("context"))
                    .and_then(json_u64)
                    .filter(|v| *v > 0);
                let max_output_tokens = limit
                    .and_then(|l| l.get("output"))
                    .and_then(json_u64)
                    .filter(|v| *v > 0);
                let cost = model.get("cost");
                let cost_input = cost.and_then(|c| c.get("input")).and_then(json_f64);
                let cost_output = cost.and_then(|c| c.get("output")).and_then(json_f64);
                models.push(CatalogModel {
                    id: model_id,
                    name: model_name,
                    context_window,
                    max_output_tokens,
                    tool_call: model.get("tool_call").and_then(|v| v.as_bool()),
                    reasoning: model.get("reasoning").and_then(|v| v.as_bool()),
                    vision: json_vision(model),
                    cost_input,
                    cost_output,
                });
            }
        }
        if models.is_empty() {
            // A provider without models is noise in a picker.
            continue;
        }
        models.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        catalog.providers.insert(
            id.clone(),
            CatalogProvider {
                id,
                name,
                model_count: models.len(),
                models,
            },
        );
    }

    if catalog.providers.is_empty() {
        return Err("models.dev returned no providers with models (a proxy error page?)".into());
    }
    Ok(catalog)
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| v as u64)
    })
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}

/// Does models.dev say this model takes **image** input?
///
/// `modalities.input` is the authoritative field, and `image` in it is what
/// `read_file` needs: a model whose inputs are `text`+`pdf` or `audio` cannot be
/// shown a PNG, even though models.dev marks both as `attachment: true` (136 of
/// the live entries are attachment-only for exactly that reason). So
/// `attachment` is only a fallback for a document that predates `modalities`;
/// when `modalities` is present but unreadable, the answer is "unknown" rather
/// than a guess that could send an image to an endpoint that rejects it.
fn json_vision(model: &serde_json::Map<String, serde_json::Value>) -> Option<bool> {
    match model.get("modalities") {
        Some(serde_json::Value::Object(modalities)) => match modalities.get("input") {
            Some(serde_json::Value::Array(inputs)) => Some(
                inputs
                    .iter()
                    .filter_map(|value| value.as_str())
                    .any(|value| value.trim().eq_ignore_ascii_case("image")),
            ),
            // `modalities` without a usable `input` list: fall through to the
            // legacy flag rather than claiming the model cannot see.
            _ => model.get("attachment").and_then(|v| v.as_bool()),
        },
        // No `modalities` at all (older document): the legacy flag stands in.
        None => model.get("attachment").and_then(|v| v.as_bool()),
        Some(_) => model.get("attachment").and_then(|v| v.as_bool()),
    }
}

/// A cached catalog plus when it was fetched (unix ms).
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub catalog: Arc<Catalog>,
    pub fetched_at_ms: i64,
    /// `true` when the returned copy is older than [`CACHE_TTL`] because the
    /// refresh failed.
    pub stale: bool,
    pub error: Option<String>,
}

struct Cache {
    catalog: Option<Arc<Catalog>>,
    fetched_at: Option<Instant>,
    fetched_at_ms: i64,
    last_error: Option<String>,
}

/// Process-wide catalog cache. Cheap to clone through `Arc`; requests serialize
/// only while a fetch is in flight.
pub struct CatalogCache {
    cache: Mutex<Cache>,
    fetch_lock: tokio::sync::Mutex<()>,
    /// Explicit source; production reads [`URL_ENV`] once at construction.
    url: String,
}

impl Default for CatalogCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(Cache {
                catalog: None,
                fetched_at: None,
                fetched_at_ms: 0,
                last_error: None,
            }),
            fetch_lock: tokio::sync::Mutex::new(()),
            url: catalog_url(),
        }
    }

    /// A cache that reads from an explicit URL (tests, offline mirrors).
    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::new()
        }
    }

    /// Seed the cache (tests and embedders).
    pub fn with_catalog(catalog: Catalog) -> Self {
        let cache = Self::new();
        cache.seed(catalog);
        cache
    }

    /// Replace the cached catalog and mark it fresh.
    pub fn seed(&self, catalog: Catalog) {
        let mut state = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        state.catalog = Some(Arc::new(catalog));
        state.fetched_at = Some(Instant::now());
        state.fetched_at_ms = now_ms();
        state.last_error = None;
    }

    fn fresh(&self) -> Option<(Arc<Catalog>, i64)> {
        let state = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let catalog = state.catalog.clone()?;
        let fetched_at = state.fetched_at?;
        if fetched_at.elapsed() <= CACHE_TTL {
            Some((catalog, state.fetched_at_ms))
        } else {
            None
        }
    }

    fn any(&self) -> Option<(Arc<Catalog>, i64, Option<String>)> {
        let state = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let catalog = state.catalog.clone()?;
        Some((catalog, state.fetched_at_ms, state.last_error.clone()))
    }

    /// Get the catalog, fetching when there is no fresh copy (or `refresh`).
    pub async fn get(&self, refresh: bool) -> Result<FetchOutcome, String> {
        if !refresh {
            if let Some((catalog, fetched_at_ms)) = self.fresh() {
                return Ok(FetchOutcome {
                    catalog,
                    fetched_at_ms,
                    stale: false,
                    error: None,
                });
            }
        }

        // One fetch at a time; a request that waited re-checks, so a burst of
        // settings loads triggers exactly one upstream call.
        let _guard = self.fetch_lock.lock().await;
        if !refresh {
            if let Some((catalog, fetched_at_ms)) = self.fresh() {
                return Ok(FetchOutcome {
                    catalog,
                    fetched_at_ms,
                    stale: false,
                    error: None,
                });
            }
        }

        match fetch_catalog(&self.url).await {
            Ok(catalog) => {
                let at_ms = now_ms();
                let catalog = Arc::new(catalog);
                {
                    let mut state = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                    state.catalog = Some(Arc::clone(&catalog));
                    state.fetched_at = Some(Instant::now());
                    state.fetched_at_ms = at_ms;
                    state.last_error = None;
                }
                Ok(FetchOutcome {
                    catalog,
                    fetched_at_ms: at_ms,
                    stale: false,
                    error: None,
                })
            }
            Err(error) => {
                if let Some((catalog, fetched_at_ms, _)) = self.any() {
                    let mut state = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                    state.last_error = Some(error.clone());
                    Ok(FetchOutcome {
                        catalog,
                        fetched_at_ms,
                        stale: true,
                        error: Some(error),
                    })
                } else {
                    Err(error)
                }
            }
        }
    }
}

fn catalog_url() -> String {
    std::env::var(URL_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

async fn fetch_catalog(url: &str) -> Result<Catalog, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("catalog URL must be http(s): {url}"));
    }
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("karucode/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("could not build the catalog HTTP client: {e}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("could not fetch models.dev from {url}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("models.dev answered {status} for {url}"));
    }
    let body = response
        .bytes()
        .await
        .map_err(|e| format!("could not read the models.dev response: {e}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "models.dev response is larger than {} MiB; refusing it",
            MAX_BODY_BYTES / (1024 * 1024)
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("models.dev returned invalid JSON: {e}"))?;
    parse_catalog(&value)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> serde_json::Value {
        json!({
            "openai": {
                "id": "openai",
                "name": "OpenAI",
                "models": {
                    "gpt-small": {
                        "id": "gpt-small",
                        "name": "GPT Small",
                        "tool_call": true,
                        "reasoning": false,
                        "limit": {"context": 128000, "output": 16384},
                        "cost": {"input": 0.25, "output": 1.5}
                    },
                    "gpt-big": {
                        "id": "gpt-big",
                        "name": "GPT Big",
                        "tool_call": true,
                        "limit": {"context": 1000000}
                    }
                }
            },
            "empty": {"id": "empty", "name": "No models", "models": {}},
            "broken": 42
        })
    }

    #[test]
    fn parses_models_and_metadata() {
        let catalog = parse_catalog(&sample()).unwrap();
        assert_eq!(catalog.len(), 1, "providers without models are skipped");
        let openai = catalog.provider("openai").unwrap();
        assert_eq!(openai.name, "OpenAI");
        assert_eq!(openai.model_count, 2);
        // Sorted by display name.
        assert_eq!(openai.models[0].id, "gpt-big");
        let small = &openai.models[1];
        assert_eq!(small.name, "GPT Small");
        assert_eq!(small.context_window, Some(128_000));
        assert_eq!(small.max_output_tokens, Some(16_384));
        assert_eq!(small.tool_call, Some(true));
        assert_eq!(small.reasoning, Some(false));
        assert_eq!(small.cost_input, Some(0.25));
        assert_eq!(small.cost_output, Some(1.5));
        assert_eq!(openai.models[0].max_output_tokens, None);
    }

    #[test]
    fn a_document_without_usable_providers_is_an_error() {
        let err = parse_catalog(&json!({})).unwrap_err();
        assert!(err.contains("no providers"), "{err}");
        let err = parse_catalog(&json!([1, 2, 3])).unwrap_err();
        assert!(err.contains("not an object"), "{err}");
        let err = parse_catalog(&json!({"x": {"models": {}}})).unwrap_err();
        assert!(err.contains("no providers"), "{err}");
    }

    #[test]
    fn numeric_strings_and_odd_costs_are_tolerated() {
        let value = json!({
            "p": {
                "models": {
                    "m": {
                        "limit": {"context": 200000.0, "output": -1},
                        "cost": {"input": -3.0, "output": "free"}
                    }
                }
            }
        });
        let catalog = parse_catalog(&value).unwrap();
        let model = &catalog.provider("p").unwrap().models[0];
        assert_eq!(model.context_window, Some(200_000));
        assert_eq!(model.max_output_tokens, None, "negative limits are dropped");
        assert_eq!(model.cost_input, None, "negative costs are dropped");
        assert_eq!(model.cost_output, None);
    }

    /// The `image` entry in `modalities.input` is the signal `read_file` needs.
    #[test]
    fn image_input_is_read_from_modalities() {
        let value = json!({
            "openai": {
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o",
                        "attachment": true,
                        "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]}
                    },
                    "gpt-text": {
                        "id": "gpt-text",
                        "attachment": false,
                        "modalities": {"input": ["text"], "output": ["text"]}
                    }
                }
            }
        });
        let catalog = parse_catalog(&value).unwrap();
        let models = &catalog.provider("openai").unwrap().models;
        let find = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert_eq!(find("gpt-4o").vision, Some(true));
        assert_eq!(find("gpt-text").vision, Some(false));
        // It rides through the lookup shape the settings screen prefills from.
        let hit = catalog.lookup("openai/gpt-4o").unwrap();
        assert_eq!(hit.model.vision, Some(true));
    }

    /// `attachment: true` does not mean "can see": a pdf-only or audio-only model
    /// is an attachment model too, and handing it a PNG fails the whole turn.
    #[test]
    fn an_attachment_only_model_is_not_a_vision_model() {
        let value = json!({
            "p": {
                "models": {
                    "whisper": {
                        "id": "whisper",
                        "attachment": true,
                        "modalities": {"input": ["audio"], "output": ["text"]}
                    },
                    "doc": {
                        "id": "doc",
                        "attachment": true,
                        "modalities": {"input": ["text", "pdf"], "output": ["text"]}
                    },
                    "omni": {
                        "id": "omni",
                        "attachment": true,
                        "modalities": {"input": ["text", "audio", "image"], "output": ["text"]}
                    }
                }
            }
        });
        let catalog = parse_catalog(&value).unwrap();
        let models = &catalog.provider("p").unwrap().models;
        let find = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert_eq!(find("whisper").vision, Some(false));
        assert_eq!(find("doc").vision, Some(false));
        assert_eq!(find("omni").vision, Some(true), "one image is enough");
    }

    /// A document without `modalities` (an older mirror) still answers from the
    /// legacy `attachment` flag, and an entry with neither stays unknown.
    #[test]
    fn vision_falls_back_to_the_legacy_attachment_flag() {
        let value = json!({
            "p": {
                "models": {
                    "legacy-vision": {"id": "legacy-vision", "attachment": true},
                    "legacy-text": {"id": "legacy-text", "attachment": false},
                    "silent": {"id": "silent"},
                    "odd": {"id": "odd", "modalities": "yes please"}
                }
            }
        });
        let catalog = parse_catalog(&value).unwrap();
        let models = &catalog.provider("p").unwrap().models;
        let find = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert_eq!(find("legacy-vision").vision, Some(true));
        assert_eq!(find("legacy-text").vision, Some(false));
        assert_eq!(
            find("silent").vision,
            None,
            "unknown must stay unknown, not become a silent 'no'"
        );
        // An unreadable `modalities` is not trusted either way.
        assert_eq!(find("odd").vision, None);
    }

    fn lookup_catalog() -> Catalog {
        parse_catalog(&json!({
            "deepseek": {
                "id": "deepseek",
                "name": "DeepSeek",
                "models": {
                    "deepseek-v4.1-flash-1017": {
                        "id": "deepseek-v4.1-flash-1017",
                        "name": "DeepSeek V4.1 Flash",
                        "limit": {"context": 128000, "output": 8192}
                    }
                }
            },
            "openrouter": {
                "id": "openrouter",
                "name": "OpenRouter",
                "models": {
                    "openai/gpt-4o": {"id": "openai/gpt-4o", "name": "GPT-4o (OR)"}
                }
            },
            "openai": {
                "id": "openai",
                "name": "OpenAI",
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o",
                        "name": "GPT-4o",
                        "limit": {"context": 128000}
                    },
                    "gpt-4o-mini": {"id": "gpt-4o-mini", "name": "GPT-4o mini"}
                }
            },
            "azure": {
                "id": "azure",
                "name": "Azure",
                "models": {
                    "gpt-4o": {"id": "gpt-4o", "name": "GPT-4o (Azure)"},
                    "gpt-4o-mini": {"id": "gpt-4o-mini", "name": "GPT-4o mini (Azure)"}
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn lookup_matches_bare_and_exact_ids() {
        let catalog = lookup_catalog();
        let hit = catalog.lookup("deepseek-v4.1-flash-1017").unwrap();
        assert_eq!(hit.provider, "deepseek");
        assert_eq!(hit.matched_id, "deepseek-v4.1-flash-1017");
        assert_eq!(hit.model.context_window, Some(128_000));
        // A catalog id that itself contains a slash matches whole.
        let hit = catalog.lookup("openai/gpt-4o").unwrap();
        assert_eq!(hit.provider, "openrouter", "exact id beats suffixes");
        assert_eq!(hit.matched_id, "openai/gpt-4o");
    }

    #[test]
    fn lookup_strips_gateway_prefixes() {
        let catalog = lookup_catalog();
        // provider/model
        let hit = catalog.lookup("a6api/deepseek-v4.1-flash-1017").unwrap();
        assert_eq!(hit.provider, "deepseek");
        assert_eq!(hit.matched_id, "deepseek-v4.1-flash-1017");
        // provider/upstream/model
        let hit = catalog
            .lookup("a6api/ali/deepseek-v4.1-flash-1017")
            .unwrap();
        assert_eq!(hit.provider, "deepseek");
        assert_eq!(hit.model.name, "DeepSeek V4.1 Flash");
        // Deeper routing keeps working.
        let hit = catalog
            .lookup("gw/region/ali/deepseek-v4.1-flash-1017")
            .unwrap();
        assert_eq!(hit.provider, "deepseek");
    }

    #[test]
    fn lookup_is_case_insensitive_and_prefers_the_longest_suffix() {
        let catalog = lookup_catalog();
        let hit = catalog
            .lookup("  A6API/ALI/DeepSeek-V4.1-Flash-1017 ")
            .unwrap();
        assert_eq!(hit.provider, "deepseek");
        // `gpt-4o` exists bare (openai, azure) and under a slash (openrouter):
        // the longer suffix `openai/gpt-4o` wins for a gateway id ending in it.
        let hit = catalog.lookup("a6api/openai/gpt-4o").unwrap();
        assert_eq!(hit.provider, "openrouter");
        assert_eq!(hit.matched_id, "openai/gpt-4o");
    }

    #[test]
    fn lookup_breaks_ties_with_a_provider_segment() {
        let catalog = lookup_catalog();
        // `gpt-4o-mini` exists under both azure and openai. Alphabetical order
        // would pick azure; a segment naming openai overrides that.
        let hit = catalog.lookup("a6api/openai/gpt-4o-mini").unwrap();
        assert_eq!(hit.provider, "openai");
        assert_eq!(hit.matched_id, "gpt-4o-mini");
        // No segment names a catalog provider: the tie breaks alphabetically,
        // so the answer is deterministic.
        let hit = catalog.lookup("a6api/east/gpt-4o-mini").unwrap();
        assert_eq!(hit.provider, "azure");
    }

    #[test]
    fn lookup_reports_nothing_for_unknown_or_empty_ids() {
        let catalog = lookup_catalog();
        assert!(catalog.lookup("a6api/ali/no-such-model").is_none());
        assert!(catalog.lookup("").is_none());
        assert!(catalog.lookup("   ").is_none());
        assert!(catalog.lookup("/").is_none());
    }

    #[tokio::test]
    async fn a_seeded_cache_is_served_without_network() {
        let cache = CatalogCache::with_catalog(parse_catalog(&sample()).unwrap());
        let outcome = cache.get(false).await.unwrap();
        assert_eq!(outcome.catalog.len(), 1);
        assert!(!outcome.stale);
        assert!(outcome.error.is_none());
    }

    #[tokio::test]
    async fn a_fetch_failure_without_a_cache_is_an_error() {
        // Point at a closed port so the fetch fails fast and deterministically.
        let cache = CatalogCache::with_url("http://127.0.0.1:9/api.json");
        let error = cache.get(false).await.unwrap_err();
        assert!(error.contains("could not fetch"), "{error}");
    }

    #[tokio::test]
    async fn a_fetch_failure_with_a_cache_serves_the_stale_copy() {
        let seeded = CatalogCache::with_url("http://127.0.0.1:9/api.json");
        {
            let mut state = seeded.cache.lock().unwrap();
            state.catalog = Some(Arc::new(parse_catalog(&sample()).unwrap()));
            // Older than the TTL, so `get` must try the network first.
            state.fetched_at = Some(Instant::now() - CACHE_TTL - Duration::from_secs(1));
            state.fetched_at_ms = 1234;
        }
        let outcome = seeded.get(false).await.unwrap();
        assert!(outcome.stale, "the stale copy is flagged");
        assert!(outcome.error.is_some());
        assert_eq!(outcome.fetched_at_ms, 1234);
        assert_eq!(outcome.catalog.len(), 1);
    }
}
