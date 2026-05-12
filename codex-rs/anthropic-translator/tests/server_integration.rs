//! End-to-end integration test for the translator HTTP server.
//!
//! Spins up the translator pointed at a wiremock-faked anthroproxy,
//! POSTs a Codex `/v1/responses` request, and verifies:
//!   * the server returns the correct SSE stream to Codex,
//!   * the upstream received the correctly-translated Anthropic
//!     `/v1/messages` payload.

use codex_anthropic_translator::server::AppConfig;
use codex_anthropic_translator::server::serve;
use serde_json::Value;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// Anthropic SSE stream that yields a simple text-only turn.
const TEXT_TURN_SSE: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-7\",\"usage\":{\"input_tokens\":42,\"output_tokens\":1}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

async fn start_translator(upstream: String) -> SocketAddr {
    start_translator_with(upstream, Vec::new()).await
}

async fn start_translator_with(upstream: String, beta_features: Vec<String>) -> SocketAddr {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(err) => panic!("bind translator: {err}"),
    };
    let addr = match listener.local_addr() {
        Ok(a) => a,
        Err(err) => panic!("local_addr: {err}"),
    };
    tokio::spawn(async move {
        let _ = serve(
            listener,
            AppConfig {
                upstream_url: upstream,
                beta_features,
            },
        )
        .await;
    });
    // Give the server a beat to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

fn codex_request_body() -> Value {
    json!({
        "model": "claude-opus-4-7",
        "instructions": "You are Codex.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Hello"}],
            }
        ],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
        "prompt_cache_key": "thread-x",
    })
}

#[tokio::test]
async fn end_to_end_text_turn_translates_request_and_streams_back_codex_events() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_TURN_SSE),
        )
        .mount(&upstream)
        .await;

    let addr = start_translator(upstream.uri()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .json(&codex_request_body())
        .send()
        .await
        .expect("translator request succeeds");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("read body");

    // Expected sequence of Codex SSE events: created, output_item.added,
    // 2x output_text.delta, output_item.done, completed.
    assert!(
        body.contains("response.created"),
        "missing response.created in: {body}"
    );
    assert!(
        body.contains("response.output_item.added"),
        "missing output_item.added"
    );
    assert!(body.contains("Hello"));
    assert!(body.contains(" world"));
    assert!(
        body.contains("response.completed"),
        "missing response.completed"
    );
    assert!(body.contains("\"end_turn\":true"));

    // Verify the upstream received an Anthropic-shaped request.
    let received = upstream.received_requests().await.expect("requests");
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).expect("upstream body json");
    assert_eq!(body["model"], json!("claude-opus-4-7"));
    assert_eq!(body["max_tokens"], json!(128_000));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(
        body["thinking"],
        json!({"type": "adaptive", "display": "summarized"}),
    );
    assert_eq!(body["output_config"]["effort"], json!("xhigh"));
    assert_eq!(body["system"][0]["text"], json!("You are Codex."));
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({"type": "ephemeral"}),
    );
    assert_eq!(body["messages"][0]["role"], json!("user"));
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type": "text", "text": "Hello"}),
    );
}

#[tokio::test]
async fn unsupported_model_returns_400_with_translator_error() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ignored"))
        .mount(&upstream)
        .await;
    let addr = start_translator(upstream.uri()).await;

    let mut body = codex_request_body();
    body["model"] = json!("gpt-5-codex");

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(response.status(), 400);
    let err_text = response.text().await.unwrap_or_default();
    assert!(
        err_text.contains("unsupported model") || err_text.contains("gpt-5-codex"),
        "expected unsupported-model error, got: {err_text}",
    );

    // No upstream call should have happened.
    let received = upstream.received_requests().await.expect("requests");
    assert!(
        received.is_empty(),
        "must not call upstream on translation error"
    );
}

#[tokio::test]
async fn upstream_failure_translates_to_response_failed_event() {
    const ERROR_SSE: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-7\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\
\n\
event: error\n\
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\
\n";

    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(ERROR_SSE),
        )
        .mount(&upstream)
        .await;
    let addr = start_translator(upstream.uri()).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .json(&codex_request_body())
        .send()
        .await
        .expect("send");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(
        body.contains("response.failed"),
        "expected response.failed event in stream, got: {body}",
    );
    assert!(body.contains("overloaded_error"));
}

#[tokio::test]
async fn beta_features_are_sent_via_anthropic_beta_header() {
    // When beta_features is non-empty, every upstream POST must
    // carry a comma-joined `anthropic-beta` header.
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_TURN_SSE),
        )
        .mount(&upstream)
        .await;
    let addr = start_translator_with(
        upstream.uri(),
        vec![
            "context-management-2025-06-27".into(),
            "compaction-1.0".into(),
        ],
    )
    .await;

    let _ = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .json(&codex_request_body())
        .send()
        .await
        .expect("send");

    let received = upstream.received_requests().await.expect("requests");
    assert_eq!(received.len(), 1);
    let header = received[0]
        .headers
        .get("anthropic-beta")
        .expect("anthropic-beta header set when beta_features is non-empty");
    let value = header.to_str().expect("header value is utf-8");
    assert!(value.contains("context-management-2025-06-27"));
    assert!(value.contains("compaction-1.0"));
}

#[tokio::test]
async fn beta_features_empty_does_not_send_anthropic_beta_header() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_TURN_SSE),
        )
        .mount(&upstream)
        .await;
    let addr = start_translator(upstream.uri()).await;

    let _ = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .json(&codex_request_body())
        .send()
        .await
        .expect("send");

    let received = upstream.received_requests().await.expect("requests");
    assert!(
        received[0].headers.get("anthropic-beta").is_none(),
        "no header expected when beta_features is empty",
    );
}

/// `GET /v1/models` must serve a Codex `ModelsResponse` containing the
/// Claude models we support. The Codex desktop app and TUI both call
/// this endpoint when populating the model picker for a custom
/// `model_provider`. If we don't serve it, the picker is empty even
/// when `requires_openai_auth = true` workaround is applied.
///
/// Source-of-truth shape: `codex-rs/protocol/src/openai_models.rs`
/// (`ModelsResponse { models: Vec<ModelInfo> }`). We don't link the
/// crate to keep the translator standalone, so this test only asserts
/// the wire-level invariants the desktop relies on.
#[tokio::test]
async fn models_endpoint_lists_supported_anthropic_models() {
    // The /v1/models route never reaches upstream, so a placeholder
    // upstream URL is fine. Avoids needing a wiremock bind, which the
    // CI sandbox sometimes refuses.
    let addr = start_translator("http://127.0.0.1:1".to_string()).await;

    let response = reqwest::Client::new()
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .expect("send GET /v1/models");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json body");

    let models = body
        .get("models")
        .and_then(Value::as_array)
        .expect("`models` array on response");
    assert!(!models.is_empty(), "models array must not be empty");

    let opus = models
        .iter()
        .find(|m| m.get("slug").and_then(Value::as_str) == Some("claude-opus-4-7"))
        .expect("claude-opus-4-7 entry present");

    // Spot-check the fields the desktop's model picker actually
    // surfaces and the app-server uses to bootstrap a turn.
    assert_eq!(opus.get("display_name").and_then(Value::as_str), Some("Claude Opus 4.7"));
    assert_eq!(opus.get("supported_in_api").and_then(Value::as_bool), Some(true));
    assert_eq!(opus.get("visibility").and_then(Value::as_str), Some("list"));
    assert!(
        opus.get("base_instructions").and_then(Value::as_str).is_some_and(|s| !s.is_empty()),
        "base_instructions must be a non-empty string for the desktop's system-prompt build",
    );
    let levels = opus
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .expect("supported_reasoning_levels array");
    assert!(
        levels
            .iter()
            .any(|l| l.get("effort").and_then(Value::as_str) == Some("high")),
        "high effort must be listed so the desktop reasoning-effort picker offers it",
    );
}
