//! Wire-format assertions: the exact JSON body and headers we send.

mod support;

use futures_util::StreamExt;
use kc_core::config::ModelConfig;
use kc_core::message::{Message, ToolCall};
use kc_core::provider::{ChatRequest, Provider, ToolCallAccumulator};
use kc_core::tool::{ToolOrigin, ToolSpec};
use kc_provider::OpenAiProvider;
use std::collections::BTreeMap;
use support::*;
use tokio_util::sync::CancellationToken;

const OK_SSE: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

fn tool(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        }),
        origin: ToolOrigin::Builtin,
    }
}

fn base_config(server: &MockServer) -> ModelConfig {
    ModelConfig {
        base_url: format!("http://{}/v1/", server.addr),
        model: "gpt-test".into(),
        request_timeout_secs: 10,
        ..Default::default()
    }
}

fn full_request() -> ChatRequest {
    ChatRequest {
        admission: None,
        model: "gpt-test".into(),
        send_temperature: true,
        messages: vec![
            Message::system("be helpful"),
            Message::user("read a.txt"),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a.txt\"}".into(),
                }],
            ),
            Message::tool_result("call_1", "read_file", "file contents"),
        ],
        tools: vec![tool("read_file", "Read a file")],
        temperature: 0.35,
        max_tokens: None,
        top_p: None,
        parallel_tool_calls: false,
        extra_headers: vec![("x-request-header".into(), "yes".into())],
        extra_body: serde_json::Map::from_iter([("seed".to_string(), serde_json::json!(7))]),
    }
}

async fn drain(mut stream: kc_core::provider::ProviderStream) {
    while let Some(item) = stream.next().await {
        let _ = item;
    }
}

#[tokio::test]
async fn request_body_and_headers_match_the_wire_contract() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.extra_headers = BTreeMap::from_iter([
        ("x-org".to_string(), "karu".to_string()),
        ("x-extra".to_string(), "1".to_string()),
    ]);
    config.extra_body =
        BTreeMap::from_iter([("reasoning_effort".to_string(), serde_json::json!("low"))]);
    assert!(config.api_key.is_none());

    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;

    let request = server.wait_for_request().await;
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/v1/chat/completions");
    assert!(request
        .header("content-type")
        .unwrap()
        .starts_with("application/json"));
    assert_eq!(
        request.header("accept").as_deref(),
        Some("text/event-stream")
    );
    assert_eq!(request.header("x-org").as_deref(), Some("karu"));
    assert_eq!(request.header("x-extra").as_deref(), Some("1"));
    assert_eq!(request.header("x-request-header").as_deref(), Some("yes"));
    assert!(
        request.header("authorization").is_none(),
        "no api key configured, so no Authorization header may be sent"
    );

    let body = request.json();
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["stream"], true);
    assert_eq!(body["temperature"], 0.35);
    assert_eq!(body["parallel_tool_calls"], false);
    assert!(body.get("max_tokens").is_none(), "{body}");
    assert!(body.get("top_p").is_none(), "{body}");
    // `tool_choice` is intentionally omitted: it defaults to `auto` server-side,
    // and sending it explicitly costs 9-30s of extra latency on some gateways.
    assert!(body.get("tool_choice").is_none(), "{body}");
    assert_eq!(
        body["tools"][0],
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"],
                },
            }
        })
    );
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(body["seed"], 7);

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "be helpful");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[2]["role"], "assistant");
    assert!(
        messages[2].get("content").is_none(),
        "assistant tool-call turns must omit empty content: {}",
        messages[2]
    );
    assert_eq!(
        messages[2]["tool_calls"][0],
        serde_json::json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "read_file", "arguments": "{\"path\":\"a.txt\"}"}
        })
    );
    assert_eq!(messages[3]["role"], "tool");
    assert_eq!(messages[3]["content"], "file contents");
    assert_eq!(messages[3]["tool_call_id"], "call_1");
    assert_eq!(messages[3]["name"], "read_file");
}

#[tokio::test]
async fn api_key_is_sent_as_bearer_token_only_when_configured() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.api_key = Some("sk-test-key".into());
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;

    let request = server.wait_for_request().await;
    assert_eq!(
        request.header("authorization").as_deref(),
        Some("Bearer sk-test-key")
    );
}

#[tokio::test]
async fn configured_authorization_is_not_overwritten_by_api_key() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.api_key = Some("sk-must-not-replace-the-explicit-header".into());
    config
        .extra_headers
        .insert("Authorization".into(), "Custom explicit-credential".into());
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;
    let request = server.wait_for_request().await;
    assert_eq!(
        request.header("authorization").as_deref(),
        Some("Custom explicit-credential")
    );
    assert!(!request
        .header("authorization")
        .unwrap()
        .contains("sk-must-not"));
}

#[tokio::test]
async fn empty_api_key_does_not_send_authorization() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.api_key = Some("   ".into());
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;
    assert!(server
        .wait_for_request()
        .await
        .header("authorization")
        .is_none());
}

#[tokio::test]
async fn base_url_already_ending_in_chat_completions_is_not_doubled() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.base_url = format!("http://{}/v1/chat/completions", server.addr);
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;
    assert_eq!(
        server.wait_for_request().await.path(),
        "/v1/chat/completions"
    );
}

#[tokio::test]
async fn empty_tools_are_omitted_from_the_body() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let provider = OpenAiProvider::new(base_config(&server)).unwrap();
    let mut request = full_request();
    request.tools.clear();
    let stream = provider
        .stream_chat(request, CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;
    let body = server.wait_for_request().await.json();
    assert!(body.get("tools").is_none(), "{body}");
    assert!(body.get("tool_choice").is_none(), "{body}");
}

#[tokio::test]
async fn assistant_message_with_text_keeps_content() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let provider = OpenAiProvider::new(base_config(&server)).unwrap();
    let mut request = full_request();
    request.messages = vec![
        Message::assistant(
            "let me look",
            vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
        ),
        Message::user(""),
    ];
    let stream = provider
        .stream_chat(request, CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;
    let body = server.wait_for_request().await.json();
    assert_eq!(body["messages"][0]["content"], "let me look");
    assert_eq!(body["messages"][1]["content"], "");
}

#[tokio::test]
async fn list_models_returns_ids_from_the_models_endpoint() {
    let server = spawn(move |_req, mut socket| async move {
        let response = response(
            "200 OK",
            "application/json",
            "{\"object\":\"list\",\"data\":[{\"id\":\"gpt-a\"},{\"id\":\"gpt-b\"}]}",
        );
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(base_config(&server)).unwrap();
    let models = provider.list_models().await.unwrap();
    assert_eq!(models, vec!["gpt-a".to_string(), "gpt-b".to_string()]);

    let request = server.wait_for_request().await;
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/v1/models");
    assert_eq!(
        request.header("accept").as_deref(),
        Some("application/json")
    );
}

#[tokio::test]
async fn list_models_reports_an_unavailable_endpoint() {
    let server = spawn(move |_req, mut socket| async move {
        let response = response("404 Not Found", "text/plain", "no such route");
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(base_config(&server)).unwrap();
    let err = provider.list_models().await.unwrap_err().to_string();
    assert!(err.contains("model listing unavailable"), "{err}");
    assert!(err.contains("404"), "{err}");
    assert!(err.contains("/models"), "{err}");
}

#[tokio::test]
async fn output_limit_from_max_output_tokens_reaches_the_wire() {
    // The runtime reserves `window - max_output_tokens` for the answer, so the
    // request must actually carry that limit; otherwise the model can generate
    // past the reserve and hit `finish_reason: "length"` on long answers.
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.max_tokens = None;
    config.max_output_tokens = Some(8_192);
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;

    let body = server.wait_for_request().await.json();
    assert_eq!(body["max_tokens"], 8_192, "{body}");
    assert!(
        body.get("max_completion_tokens").is_none(),
        "only one output-limit spelling may be sent: {body}"
    );
}

#[tokio::test]
async fn max_tokens_wins_over_max_output_tokens() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.max_tokens = Some(1_024);
    config.max_output_tokens = Some(8_192);
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;

    let body = server.wait_for_request().await.json();
    assert_eq!(body["max_tokens"], 1_024, "{body}");
}

#[tokio::test]
async fn no_output_limit_is_sent_when_none_is_configured() {
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.max_tokens = None;
    config.max_output_tokens = None;
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;

    let body = server.wait_for_request().await.json();
    assert!(body.get("max_tokens").is_none(), "{body}");
    assert!(body.get("max_completion_tokens").is_none(), "{body}");
}

#[tokio::test]
async fn extra_body_can_switch_to_max_completion_tokens() {
    // Newer OpenAI models reject `max_tokens` in favour of
    // `max_completion_tokens`; `model.extra_body` is the documented override.
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, OK_SSE).await }).await;
    let mut config = base_config(&server);
    config.max_output_tokens = Some(8_192);
    config.extra_body = BTreeMap::from_iter([(
        "max_completion_tokens".to_string(),
        serde_json::json!(4_096),
    )]);
    let provider = OpenAiProvider::new(config).unwrap();
    let stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    drain(stream).await;

    let body = server.wait_for_request().await.json();
    assert_eq!(body["max_completion_tokens"], 4_096, "{body}");
    assert!(
        body.get("max_tokens").is_none(),
        "the operator's spelling must replace ours, not join it: {body}"
    );
}

#[tokio::test]
async fn tool_call_arguments_survive_a_real_stream() {
    // Sanity check that the wire shape and the delta parser agree end to end.
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\"\"}}]}}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"a.txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let provider = OpenAiProvider::new(base_config(&server)).unwrap();
    let mut stream = provider
        .stream_chat(full_request(), CancellationToken::new())
        .await
        .unwrap();
    let mut accumulator = ToolCallAccumulator::default();
    while let Some(item) = stream.next().await {
        if let kc_core::provider::ProviderChunk::ToolCallDelta {
            index,
            id,
            name,
            arguments_delta,
        } = item.unwrap()
        {
            accumulator.apply(index, id, name, arguments_delta);
        }
    }
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, "{\"path\":\"a.txt\"}");
}
