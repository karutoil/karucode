//! Request construction: URL joining, headers and the chat-completions body.
//!
//! Everything here is pure so the wire format can be asserted in tests without
//! touching the network.

use crate::Error;
use crate::Result;
use kc_core::config::ModelConfig;
use kc_core::message::{Message, Role};
use kc_core::provider::ChatRequest;
use kc_core::tool::ToolSpec;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Map, Value};

/// Keys the provider owns; `extra_body` may override tunables (temperature,
/// `max_tokens`, ...) but never the framing of the stream.
const RESERVED_BODY_KEYS: [&str; 4] = ["model", "messages", "stream", "tools"];

fn normalize_base(base_url: &str) -> Result<&str> {
    let base = base_url.trim();
    if base.is_empty() {
        return Err(Error::Config(
            "model.base_url is empty; set it to an OpenAI-compatible API root such as https://host/v1"
                .into(),
        ));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(Error::Config(format!(
            "model.base_url must start with http:// or https:// (got `{base}`)"
        )));
    }
    Ok(base.trim_end_matches('/'))
}

/// `POST {base}/chat/completions`, tolerating a trailing slash, a base that
/// already ends in `/chat/completions`, and bases such as `.../v1`.
pub fn chat_completions_url(base_url: &str) -> Result<String> {
    let base = normalize_base(base_url)?;
    if base.ends_with("/chat/completions") {
        Ok(base.to_string())
    } else {
        Ok(format!("{base}/chat/completions"))
    }
}

/// `GET {base}/models`. Also accepts a base pointing at an endpoint URL.
pub fn models_url(base_url: &str) -> Result<String> {
    let base = normalize_base(base_url)?;
    let base = base.strip_suffix("/chat/completions").unwrap_or(base);
    if base.ends_with("/models") {
        Ok(base.to_string())
    } else {
        Ok(format!("{base}/models"))
    }
}

/// Build the OpenAI-compatible request body for one streaming turn.
pub fn build_chat_body(request: &ChatRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.clone()));
    body.insert(
        "messages".into(),
        Value::Array(messages_json(&request.messages)),
    );
    body.insert("stream".into(), Value::Bool(true));
    // Ask for the usage block explicitly. OpenAI-compatible gateways are allowed
    // to omit (or zero) the usage on a streamed response unless this is set, and
    // a zeroed usage is worse than none: it makes a 99% prompt-cache hit rate
    // read as 0% on every dashboard built from those records. Measured against
    // ai.karutoil.site: without this key `cached_tokens` came back 0 for a
    // request that was 8_960/9_039 cached; with it, the real number.
    body.insert(
        "stream_options".into(),
        serde_json::json!({ "include_usage": true }),
    );
    if request.send_temperature {
        body.insert("temperature".into(), float_json(request.temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        body.insert("max_tokens".into(), Value::from(max_tokens));
    }
    if let Some(top_p) = request.top_p {
        body.insert("top_p".into(), float_json(top_p));
    }
    body.insert(
        "parallel_tool_calls".into(),
        Value::Bool(request.parallel_tool_calls),
    );
    if !request.tools.is_empty() {
        body.insert("tools".into(), tools_json(&request.tools));
        // Deliberately NO `tool_choice`. `auto` is already the server-side
        // default whenever tools are present, and sending it explicitly costs a
        // lot on some gateways: measured against ai.karutoil.site, explicit
        // `tool_choice:"auto"` pushed time-to-first-token to 9-30s (one request
        // never streamed at all) versus ~1s when omitted. Operators who need a
        // different value can set it through `model.extra_body.tool_choice`,
        // which is allowed through below.
    }
    for (key, value) in &request.extra_body {
        if RESERVED_BODY_KEYS.contains(&key.as_str()) {
            tracing::debug!(key, "ignoring extra_body key reserved by the provider");
            continue;
        }
        body.insert(key.clone(), value.clone());
    }
    Value::Object(body)
}

/// Build the request body for one streaming turn, including the output limit
/// the model config declares.
///
/// The runtime derives its context budget as `window - max_output_tokens`, so
/// that room is reserved on the assumption the answer cannot exceed it. Without
/// a limit on the wire the model is free to generate past the reserve and hit
/// `finish_reason: "length"` on exactly the long answers the reserve protects —
/// and with `max_tokens` unset (the default) there is no output cap at all.
///
/// `max_tokens` wins when set; `max_output_tokens` is the fallback. Only one of
/// them is ever sent. Newer OpenAI models reject `max_tokens` in favour of
/// `max_completion_tokens`; an operator can override the key through
/// `model.extra_body` (e.g. `max_completion_tokens = 8192`), which is merged
/// after this and therefore wins.
pub fn build_chat_body_for(config: &ModelConfig, request: &ChatRequest) -> Value {
    let mut body = build_chat_body(request);
    if let Some(limit) = output_token_limit(config, request) {
        if let Some(map) = body.as_object_mut() {
            // Never send both spellings: a gateway that understands one may
            // reject the other, and two limits disagreeing is undefined.
            map.remove("max_completion_tokens");
            map.insert("max_tokens".into(), Value::from(limit));
        }
    }
    body
}

/// The output cap to send: the request's own `max_tokens`, else the model's
/// `max_tokens`, else its `max_output_tokens`.
fn output_token_limit(config: &ModelConfig, request: &ChatRequest) -> Option<u64> {
    request
        .max_tokens
        .map(u64::from)
        .or(config.max_tokens.map(u64::from))
        .or(config.max_output_tokens)
        .filter(|limit| *limit > 0)
}

/// Serialise an `f32` without leaking binary-float noise (0.9f32 would otherwise
/// become 0.8999999761581421 on the wire).
fn float_json(value: f32) -> Value {
    if !value.is_finite() {
        return Value::from(0);
    }
    let rounded = (f64::from(value) * 1e6).round() / 1e6;
    serde_json::Number::from_f64(rounded)
        .map(Value::Number)
        .unwrap_or_else(|| Value::from(0))
}

/// Merge static `ModelConfig::extra_body` keys into a request body. Keys already
/// present (from `ChatRequest::extra_body`) win, and framing keys are ignored.
pub(crate) fn apply_config_extra_body(
    body: &mut Value,
    extra: &std::collections::BTreeMap<String, Value>,
) {
    let Some(map) = body.as_object_mut() else {
        return;
    };
    for (key, value) in extra {
        if RESERVED_BODY_KEYS.contains(&key.as_str()) {
            continue;
        }
        // `max_tokens` and `max_completion_tokens` are two spellings of one
        // limit. Newer OpenAI models reject the former, so an operator who sets
        // the latter through `extra_body` must get exactly that key on the wire,
        // not both.
        if let Some(other) = output_limit_counterpart(key) {
            map.remove(other);
        }
        map.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

/// The other spelling of the output limit, if `key` is one of them.
fn output_limit_counterpart(key: &str) -> Option<&'static str> {
    match key {
        "max_tokens" => Some("max_completion_tokens"),
        "max_completion_tokens" => Some("max_tokens"),
        _ => None,
    }
}

fn tools_json(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect(),
    )
}

/// Serialise the transcript, expanding attached images.
///
/// A `tool` message can only carry text on the OpenAI-compatible wire format, so
/// an image a tool returned travels in a **separate `user` message** with an
/// `image_url` content part. That message is emitted after the run of tool
/// results, never between them: a request whose assistant `tool_calls` are not
/// followed directly by their `tool` results is rejected by strict endpoints.
fn messages_json(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::with_capacity(messages.len());
    let mut pending: Vec<Value> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        out.push(message_json(message));
        if !message.images.is_empty() {
            pending.push(image_message_json(message));
        }
        // Hold the images back while the tool results of this turn are still
        // arriving; flush as soon as the run ends (or the transcript does).
        let next_is_tool = messages
            .get(index + 1)
            .map(|next| next.role == Role::Tool)
            .unwrap_or(false);
        if !next_is_tool {
            out.append(&mut pending);
        }
    }
    out.append(&mut pending);
    out
}

/// The `user` message that carries one message's images.
fn image_message_json(message: &Message) -> Value {
    let mut parts = Vec::with_capacity(message.images.len() * 2 + 1);
    let source = message
        .name
        .as_deref()
        .or(message.tool_call_id.as_deref())
        .unwrap_or("a tool");
    parts.push(serde_json::json!({
        "type": "text",
        "text": format!(
            "Images returned by `{source}`. Look at them directly; the text of the result \
             describes what was read, it is not the image."
        ),
    }));
    for image in &message.images {
        parts.push(serde_json::json!({"type": "text", "text": image.label()}));
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": image.data_url()},
        }));
    }
    serde_json::json!({"role": "user", "content": parts})
}

fn message_json(message: &Message) -> Value {
    let mut obj = Map::new();
    obj.insert("role".into(), Value::String(message.role.as_str().into()));
    let has_tool_calls = !message.tool_calls.is_empty();
    // Assistant turns that only carry tool calls send no `content`; every other
    // message keeps `content` (possibly empty) so providers see the field.
    if !(has_tool_calls && message.content.is_empty()) {
        obj.insert("content".into(), Value::String(message.content.clone()));
    }
    // Include reasoning_content if present (for models that support it)
    if let Some(reasoning) = &message.reasoning_content {
        obj.insert("reasoning_content".into(), Value::String(reasoning.clone()));
    }
    if has_tool_calls {
        obj.insert(
            "tool_calls".into(),
            Value::Array(
                message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments,
                            }
                        })
                    })
                    .collect(),
            ),
        );
    }
    if message.role == Role::Tool {
        if let Some(id) = &message.tool_call_id {
            obj.insert("tool_call_id".into(), Value::String(id.clone()));
        }
    }
    if let Some(name) = &message.name {
        obj.insert("name".into(), Value::String(name.clone()));
    }
    Value::Object(obj)
}

/// Headers for a chat-completions call (SSE accept).
pub(crate) fn build_headers(config: &ModelConfig, request: &ChatRequest) -> Result<HeaderMap> {
    build_headers_with(config, "text/event-stream", &request.extra_headers)
}

/// Headers for a provider call: content negotiation, configured extra headers,
/// then bearer auth when a non-empty key is present. Extra headers are applied
/// before auth, so an explicitly configured `Authorization` header wins.
pub(crate) fn build_headers_with(
    config: &ModelConfig,
    accept: &'static str,
    extra: &[(String, String)],
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static(accept));
    for (name, value) in config
        .extra_headers
        .iter()
        .chain(extra.iter().map(|(k, v)| (k, v)))
    {
        insert_header(&mut headers, name, value)?;
    }
    if let Some(key) = config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        let value = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| {
            Error::Config(
                "model.api_key contains characters that are not valid in an HTTP header".into(),
            )
        })?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    let header_name = HeaderName::from_bytes(name.trim().as_bytes())
        .map_err(|_| Error::Config(format!("invalid extra header name `{name}`")))?;
    // Never echo the value: extra headers may carry credentials.
    let header_value = HeaderValue::from_str(value)
        .map_err(|_| Error::Config(format!("invalid value for extra header `{name}`")))?;
    headers.insert(header_name, header_value);
    Ok(())
}

/// Header values that must never appear in an error message.
pub(crate) fn sensitive_values(headers: &HeaderMap) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            name == "authorization"
                || name.contains("key")
                || name.contains("token")
                || name.contains("secret")
                || name.contains("auth")
        })
        .filter_map(|(_, value)| value.to_str().ok().map(str::to_string))
        .filter(|value| !value.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::message::{ImageAttachment, ToolCall};

    fn request() -> ChatRequest {
        ChatRequest {
            admission: None,
            model: "gpt-test".into(),
            messages: vec![],
            tools: vec![],
            temperature: 0.2,
            send_temperature: true,
            max_tokens: None,
            top_p: None,
            parallel_tool_calls: true,
            extra_headers: Vec::new(),
            extra_body: Map::new(),
        }
    }

    #[test]
    fn urls_tolerate_slashes_v1_and_full_endpoints() {
        for base in ["http://h:1/v1", "http://h:1/v1/", "http://h:1/v1//"] {
            assert_eq!(
                chat_completions_url(base).unwrap(),
                "http://h:1/v1/chat/completions"
            );
        }
        assert_eq!(
            chat_completions_url("https://h/v1/chat/completions").unwrap(),
            "https://h/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://h/v1/chat/completions/").unwrap(),
            "https://h/v1/chat/completions"
        );
        assert!(chat_completions_url("  ").is_err());
        assert!(chat_completions_url("ftp://h/v1").is_err());
    }

    #[test]
    fn model_urls_are_derived_from_the_base() {
        assert_eq!(models_url("http://h/v1").unwrap(), "http://h/v1/models");
        assert_eq!(models_url("http://h/v1/").unwrap(), "http://h/v1/models");
        assert_eq!(
            models_url("http://h/v1/chat/completions").unwrap(),
            "http://h/v1/models"
        );
        assert_eq!(
            models_url("http://h/v1/models").unwrap(),
            "http://h/v1/models"
        );
    }

    #[test]
    fn body_omits_optional_fields_and_tools() {
        let body = build_chat_body(&request());
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["temperature"], 0.2);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["parallel_tool_calls"], true);
    }

    #[test]
    fn temperature_is_not_sent_unless_it_is_asked_for() {
        // A sampling value the model was not tuned for pushed a reasoning model
        // into repeating itself instead of acting; the default is to send none.
        let mut req = request();
        req.send_temperature = false;
        let body = build_chat_body(&req);
        assert!(
            body.get("temperature").is_none(),
            "temperature was invented: {body}"
        );
        req.send_temperature = true;
        let body = build_chat_body(&req);
        assert_eq!(body["temperature"], 0.2);
    }

    #[test]
    fn usage_is_requested_on_the_stream_so_cache_hits_are_visible() {
        // Without `stream_options.include_usage` a gateway may answer with a
        // zeroed usage block, which reports a 99% prompt-cache hit as 0%.
        let body = build_chat_body(&request());
        assert_eq!(body["stream_options"]["include_usage"], true);

        // An operator can still override it through `extra_body` (a gateway
        // that rejects the field, for instance).
        let mut req = request();
        req.extra_body
            .insert("stream_options".into(), serde_json::json!({}));
        let body = build_chat_body(&req);
        assert_eq!(body["stream_options"], serde_json::json!({}));
    }

    #[test]
    fn body_carries_tools_and_tool_messages() {
        let mut req = request();
        req.max_tokens = Some(64);
        req.top_p = Some(0.9);
        req.tools.push(ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
            origin: Default::default(),
        });
        req.messages = vec![
            Message::system("sys"),
            Message::user("hi"),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a\"}".into(),
                }],
            ),
            Message::tool_result("call_1", "read_file", "contents"),
        ];
        let body = build_chat_body(&req);
        assert_eq!(
            body["tools"][0],
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "read a file",
                    "parameters": {"type": "object"},
                }
            })
        );
        // Omitting tool_choice keeps the server default (`auto`) and avoids the
        // gateway slow path; extra_body can still set it explicitly.
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["top_p"], 0.9);
        // assistant-with-tool-calls has no content key
        assert!(body["messages"][2].get("content").is_none());
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["messages"][2]["tool_calls"][0]["type"], "function");
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );
    }

    #[test]
    fn extra_body_may_override_tool_choice() {
        let mut req = request();
        req.tools.push(ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
            origin: Default::default(),
        });
        req.extra_body
            .insert("tool_choice".into(), serde_json::json!("required"));
        let body = build_chat_body(&req);
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn extra_body_cannot_break_stream_framing() {
        let mut req = request();
        req.extra_body
            .insert("reasoning_effort".into(), Value::String("low".into()));
        req.extra_body.insert("stream".into(), Value::Bool(false));
        req.extra_body
            .insert("model".into(), Value::String("other".into()));
        let body = build_chat_body(&req);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "gpt-test");
    }

    #[test]
    fn config_extra_body_is_merged_without_overriding_the_request() {
        let mut req = request();
        req.extra_body.insert("seed".into(), Value::from(7));
        req.extra_body
            .insert("reasoning_effort".into(), Value::String("high".into()));
        let mut body = build_chat_body(&req);
        let mut config_extra = std::collections::BTreeMap::new();
        config_extra.insert("reasoning_effort".to_string(), Value::String("low".into()));
        config_extra.insert("prompt_cache".to_string(), Value::Bool(true));
        config_extra.insert("stream".to_string(), Value::Bool(false));
        apply_config_extra_body(&mut body, &config_extra);
        assert_eq!(body["seed"], 7);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["prompt_cache"], true);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn an_image_travels_in_a_user_message_after_its_tool_result() {
        // A `tool` message cannot hold an image on this wire format, and a user
        // message wedged between an assistant's `tool_calls` and its results is
        // rejected by strict endpoints. So the pixels follow the whole run of
        // tool results.
        let image = ImageAttachment {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
            bytes: 8,
            name: Some("assets/logo.png".into()),
        };
        let mut req = request();
        req.messages = vec![
            Message::user("look at this"),
            Message::assistant(
                "",
                vec![
                    ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"a.txt\"}".into(),
                    },
                    ToolCall {
                        id: "call_2".into(),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"assets/logo.png\"}".into(),
                    },
                ],
            ),
            Message::tool_result("call_1", "read_file", "a.txt — 1 line"),
            Message::tool_result("call_2", "read_file", "assets/logo.png — image/png")
                .with_images(vec![image]),
        ];
        let body = build_chat_body(&req);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5, "assistant + 2 tool results + image turn");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(
            messages[3]["content"], "assets/logo.png — image/png",
            "the tool text stays text, so a gateway that ignores images still gets the summary"
        );
        assert_eq!(messages[4]["role"], "user");
        let parts = messages[4]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert!(parts[0]["text"].as_str().unwrap().contains("read_file"));
        assert_eq!(parts[1]["type"], "text");
        assert_eq!(parts[1]["text"], "assets/logo.png — image/png, 8 bytes");
        assert_eq!(parts[2]["type"], "image_url");
        assert_eq!(
            parts[2]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
        // No image field leaks onto the tool object itself.
        assert!(messages[3].get("images").is_none());
    }

    #[test]
    fn a_text_only_transcript_keeps_its_shape() {
        let mut req = request();
        req.messages = vec![Message::user("hi"), Message::assistant("hello", vec![])];
        let body = build_chat_body(&req);
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body["messages"][0]["content"].is_string());
    }

    #[test]
    fn auth_header_only_when_key_present() {
        let req = request();
        let config = ModelConfig::default();
        let headers = build_headers(&config, &req).unwrap();
        assert!(headers.get(AUTHORIZATION).is_none());

        let config = ModelConfig {
            api_key: Some(String::new()),
            ..ModelConfig::default()
        };
        let headers = build_headers(&config, &req).unwrap();
        assert!(headers.get(AUTHORIZATION).is_none());

        let config = ModelConfig {
            api_key: Some("  sk-abc  ".into()),
            ..ModelConfig::default()
        };
        let headers = build_headers(&config, &req).unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer sk-abc");
        assert_eq!(
            sensitive_values(&headers),
            vec!["Bearer sk-abc".to_string()]
        );
    }

    #[test]
    fn invalid_extra_header_names_are_rejected_without_leaking_values() {
        let req = request();
        let config = ModelConfig {
            extra_headers: std::collections::BTreeMap::from_iter([(
                "bad header".to_string(),
                "secret-value".to_string(),
            )]),
            ..ModelConfig::default()
        };
        let err = build_headers(&config, &req).unwrap_err().to_string();
        assert!(err.contains("bad header"), "{err}");
        assert!(!err.contains("secret-value"), "{err}");
    }
}
