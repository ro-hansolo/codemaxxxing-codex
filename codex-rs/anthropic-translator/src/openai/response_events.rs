//! Outbound (translator → Codex) SSE event types.
//!
//! These match the OpenAI Responses-API SSE wire format that Codex's
//! parser expects (`codex-rs/codex-api/src/sse/responses.rs`). The
//! translator emits these as the body of a `text/event-stream`
//! response on `POST /v1/responses`.

use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use serde_json::Value;

/// One event the translator pushes back to Codex over SSE.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ResponseStreamEvent {
    #[serde(rename = "response.created")]
    Created { response: ResponseObject },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: OutputItem,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: usize,
        item: OutputItem,
    },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        item_id: String,
        content_index: usize,
        delta: String,
    },

    #[serde(rename = "response.custom_tool_call_input.delta")]
    CustomToolCallInputDelta {
        item_id: String,
        call_id: String,
        delta: String,
    },

    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        item_id: String,
        summary_index: i64,
        delta: String,
    },

    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        item_id: String,
        content_index: i64,
        delta: String,
    },

    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded { item_id: String, summary_index: i64 },

    #[serde(rename = "response.completed")]
    Completed { response: ResponseObject },

    #[serde(rename = "response.failed")]
    Failed { response: ResponseObject },
}

/// The `response` field carried by `created`, `completed`, and
/// `failed` events.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseObject {
    pub id: String,
    /// Always `"response"` per the OpenAI Responses spec.
    pub object: &'static str,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

impl ResponseObject {
    pub fn new(id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            object: "response",
            model: model.into(),
            usage: None,
            end_turn: None,
            error: None,
        }
    }

    pub fn with_usage(mut self, usage: ResponseUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_end_turn(mut self, end_turn: bool) -> Self {
        self.end_turn = Some(end_turn);
        self
    }

    pub fn with_error(mut self, error: Value) -> Self {
        self.error = Some(error);
        self
    }
}

/// Mirrors the shape Codex parses at `codex-api/src/sse/responses.rs:142`.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<ResponseUsageInputDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<ResponseUsageOutputDetails>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseUsageInputDetails {
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseUsageOutputDetails {
    pub reasoning_tokens: u64,
}

/// One entry in the `output` array of `response.output_item.added` /
/// `response.output_item.done` events. Mirrors Codex's
/// `protocol::models::ResponseItem` outgoing variants.
///
/// Manual `Serialize` impl rather than derived because each variant
/// has a different shape on the wire (`message` carries a `role` +
/// `content` array, `reasoning` carries a `summary` array, …) and
/// the alternative — field-level serializers and singleton enums —
/// reads worse than ~50 lines of explicit map building.
#[derive(Debug, Clone)]
pub enum OutputItem {
    /// Assistant-authored text message (single text content block).
    AssistantMessage { id: String, text: String },
    Reasoning {
        id: String,
        encrypted_content: Option<String>,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    CustomToolCall {
        id: String,
        call_id: String,
        name: String,
        input: String,
    },
    /// Synthesized when Anthropic invokes the `local_shell` tool.
    /// Codex's tool router (`codex-rs/core/src/tools/handlers/shell/local_shell.rs`)
    /// expects `ToolPayload::LocalShell` with the documented
    /// `LocalShellAction::Exec` action shape — emitting a generic
    /// `function_call` here crashes the turn with
    /// `FunctionCallError::Fatal("LocalShellHandler expected
    /// ToolPayload::LocalShell")`.
    LocalShellCall {
        id: String,
        call_id: String,
        /// Already-shaped `LocalShellAction` JSON (e.g.
        /// `{"type":"exec","command":[...]}`). Held as a `Value`
        /// because the translator builds it from the streaming
        /// tool-use input without re-modelling the Codex-side type.
        action: Value,
    },
}

impl Serialize for OutputItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OutputItem::AssistantMessage { id, text } => {
                let content = [serde_json::json!({"type": "output_text", "text": text})];
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("type", "message")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("role", "assistant")?;
                map.serialize_entry("content", &content)?;
                map.end()
            }
            OutputItem::Reasoning {
                id,
                encrypted_content,
            } => {
                let len = 3 + usize::from(encrypted_content.is_some());
                let mut map = serializer.serialize_map(Some(len))?;
                map.serialize_entry("type", "reasoning")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("summary", &Vec::<Value>::new())?;
                if let Some(enc) = encrypted_content {
                    map.serialize_entry("encrypted_content", enc)?;
                }
                map.end()
            }
            OutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("type", "function_call")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("arguments", arguments)?;
                map.end()
            }
            OutputItem::CustomToolCall {
                id,
                call_id,
                name,
                input,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("type", "custom_tool_call")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("input", input)?;
                map.end()
            }
            OutputItem::LocalShellCall {
                id,
                call_id,
                action,
            } => {
                // Wire shape per
                // `codex-rs/protocol/src/models.rs::ResponseItem::LocalShellCall`:
                // `{type, id, call_id, status, action}`. Status is
                // always `completed` because Anthropic only delivers
                // the local_shell tool call once it's fully formed
                // — Codex's runtime has not started executing it
                // yet, but from Anthropic's side the tool-use block
                // is closed.
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("type", "local_shell_call")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("status", "completed")?;
                map.serialize_entry("action", action)?;
                map.end()
            }
        }
    }
}
