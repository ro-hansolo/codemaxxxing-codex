//! HTTP server: accepts Codex `POST /v1/responses` requests,
//! translates them to Anthropic Messages requests, ships them at the
//! configured upstream URL (anthroproxy), and streams the translated
//! response back to Codex over SSE.

use crate::anthropic::FunctionTool;
use crate::anthropic::MessageRequest;
use crate::anthropic::Tool;
use crate::anthropic::event::StreamEvent;
use crate::openai::ResponseStreamEvent;
use crate::openai::ResponsesRequest;
use crate::translate::StreamTranslator;
use crate::translate::TranslationError;
use crate::translate::translate_request;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;
use tracing::warn;

/// Static configuration shared across handler invocations.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Base URL of anthroproxy (e.g. `http://127.0.0.1:6969`). The
    /// translator appends `/v1/messages`.
    pub upstream_url: String,
    /// Anthropic beta-feature identifiers to enable on every
    /// upstream request (sent as a comma-joined `anthropic-beta`
    /// header). Empty means no header is sent.
    ///
    /// Useful values include `context-management-2025-06-27`
    /// (context editing), compaction beta strings, and other
    /// per-feature gates Anthropic ships in beta. Anthroproxy
    /// forwards the header through to Vertex unchanged.
    pub beta_features: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    config: Arc<AppConfig>,
    http: reqwest::Client,
}

/// Bind the translator to the supplied listener and serve requests
/// until cancelled. Used by both the binary entry point and the
/// integration tests.
pub async fn serve(listener: TcpListener, config: AppConfig) -> std::io::Result<()> {
    let state = AppState {
        config: Arc::new(config),
        http: reqwest::Client::builder()
            // No global timeout; SSE streams may live for a long time.
            .build()
            .map_err(|err| std::io::Error::other(err.to_string()))?,
    };
    let app = Router::new()
        .route("/v1/responses", post(handle_responses))
        .route("/v1/models", get(handle_models))
        .with_state(state);
    axum::serve(listener, app).await
}

/// Codex `ModelsResponse` body served at `GET /v1/models`. Embedded at
/// build time from `data/models.json` so the binary stays self-contained
/// (no runtime file lookup, no dependency on `codex-protocol`).
///
/// The desktop app (and TUI) hits this endpoint when populating the
/// model picker for a custom `model_provider`. Without it the picker is
/// empty for our provider even when `requires_openai_auth = true` flips
/// the picker visibility on (see openai/codex#10867 for context).
///
/// The JSON payload's shape is `codex_protocol::openai_models::
/// ModelsResponse`, kept literal here to avoid linking the protocol
/// crate. Update it whenever a new Claude model is added to the
/// translator's supported set.
const MODELS_RESPONSE_JSON: &str = include_str!("../data/models.json");

async fn handle_models() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(Body::from(MODELS_RESPONSE_JSON))
        .unwrap_or_else(|err| {
            error!(%err, "failed to build /v1/models response");
            internal_error_response()
        })
}

fn internal_error_response() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("internal error"))
        .expect("static internal-error response always builds")
}

async fn handle_responses(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<ResponsesRequest>,
) -> Response {
    // Translate the request body.
    let message_request = match translate_request(request) {
        Ok(req) => req,
        Err(err) => return translation_error_response(err),
    };

    // Determine which tools the request translator synthesized as
    // custom (those with eager_input_streaming) so the stream
    // translator can route their tool_use events as custom_tool_call.
    let custom_tools = collect_custom_tool_names(&message_request);

    // POST upstream and stream the response.
    let upstream_url = format!(
        "{}/v1/messages",
        state.config.upstream_url.trim_end_matches('/')
    );
    let mut upstream_request = state
        .http
        .post(&upstream_url)
        .header(header::ACCEPT, "text/event-stream")
        .json(&message_request);
    if !state.config.beta_features.is_empty() {
        upstream_request =
            upstream_request.header("anthropic-beta", state.config.beta_features.join(","));
    }
    let upstream_response = match upstream_request.send().await {
        Ok(resp) => resp,
        Err(err) => return upstream_error_response(err),
    };

    if !upstream_response.status().is_success() {
        let status = upstream_response.status();
        let body = upstream_response.text().await.unwrap_or_default();
        error!(%status, %body, "upstream returned non-200");
        return upstream_status_response(status, body);
    }

    // Pipe the upstream SSE through the stream translator and emit
    // Codex-shaped SSE on the way out.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);
    let upstream_body = upstream_response.bytes_stream();
    tokio::spawn(pump_stream(upstream_body, tx, custom_tools));

    let body = Body::from_stream(ReceiverStream::new(rx));
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
        .body(body)
        .unwrap_or_else(|err| {
            error!(?err, "failed to build SSE response");
            empty_500()
        })
}

async fn pump_stream<S>(
    upstream: S,
    tx: tokio::sync::mpsc::Sender<Result<bytes::Bytes, std::io::Error>>,
    custom_tools: HashSet<String>,
) where
    S: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let mut translator = StreamTranslator::new(custom_tools);
    let mut events = upstream.eventsource();
    while let Some(event_result) = events.next().await {
        let raw = match event_result {
            Ok(event) => event,
            Err(err) => {
                error!(?err, "upstream SSE error");
                break;
            }
        };
        // The eventsource crate strips the `event:` and `data:`
        // framing; `raw.data` is the JSON payload.
        let parsed: StreamEvent = match serde_json::from_str(&raw.data) {
            Ok(e) => e,
            Err(err) => {
                warn!(?err, payload = %raw.data, "failed to parse upstream SSE event");
                continue;
            }
        };
        for codex_event in translator.consume(parsed) {
            if !send_event(&tx, &codex_event).await {
                return;
            }
        }
    }
}

async fn send_event(
    tx: &tokio::sync::mpsc::Sender<Result<bytes::Bytes, std::io::Error>>,
    event: &ResponseStreamEvent,
) -> bool {
    let payload = match serde_json::to_string(event) {
        Ok(s) => s,
        Err(err) => {
            error!(?err, "failed to serialize Codex event");
            return false;
        }
    };
    let event_name = sse_event_name(event);
    let frame = format!("event: {event_name}\ndata: {payload}\n\n");
    tx.send(Ok(bytes::Bytes::from(frame))).await.is_ok()
}

fn sse_event_name(event: &ResponseStreamEvent) -> &'static str {
    match event {
        ResponseStreamEvent::Created { .. } => "response.created",
        ResponseStreamEvent::OutputItemAdded { .. } => "response.output_item.added",
        ResponseStreamEvent::OutputItemDone { .. } => "response.output_item.done",
        ResponseStreamEvent::OutputTextDelta { .. } => "response.output_text.delta",
        ResponseStreamEvent::CustomToolCallInputDelta { .. } => {
            "response.custom_tool_call_input.delta"
        }
        ResponseStreamEvent::ReasoningSummaryTextDelta { .. } => {
            "response.reasoning_summary_text.delta"
        }
        ResponseStreamEvent::ReasoningTextDelta { .. } => "response.reasoning_text.delta",
        ResponseStreamEvent::ReasoningSummaryPartAdded { .. } => {
            "response.reasoning_summary_part.added"
        }
        ResponseStreamEvent::Completed { .. } => "response.completed",
        ResponseStreamEvent::Failed { .. } => "response.failed",
    }
}

fn collect_custom_tool_names(request: &MessageRequest) -> HashSet<String> {
    request
        .tools
        .iter()
        .filter_map(|tool| match tool {
            Tool::Function(FunctionTool {
                name,
                eager_input_streaming: true,
                ..
            }) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn translation_error_response(err: TranslationError) -> Response {
    let body = serde_json::json!({
        "error": {
            "type": "translation_error",
            "message": err.to_string(),
        }
    });
    json_error(StatusCode::BAD_REQUEST, body)
}

fn upstream_error_response(err: reqwest::Error) -> Response {
    error!(?err, "upstream request failed");
    let body = serde_json::json!({
        "error": {
            "type": "upstream_unavailable",
            "message": err.to_string(),
        }
    });
    json_error(StatusCode::BAD_GATEWAY, body)
}

fn upstream_status_response(status: reqwest::StatusCode, body: String) -> Response {
    let payload = serde_json::json!({
        "error": {
            "type": "upstream_status",
            "status": status.as_u16(),
            "body": body,
        }
    });
    json_error(
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        payload,
    )
}

fn json_error(status: StatusCode, body: serde_json::Value) -> Response {
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(Body::from(bytes))
        .unwrap_or_else(|_| empty_500())
}

fn empty_500() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response
}
