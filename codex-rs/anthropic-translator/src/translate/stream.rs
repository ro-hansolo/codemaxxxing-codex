//! Stream translator state machine.
//!
//! Consumes Anthropic SSE events one-at-a-time and emits zero or more
//! Codex-shaped [`ResponseStreamEvent`]s. State is per-stream:
//!
//!   * Per Anthropic block index: an [`BlockState`] tracking the
//!     output_index, item_id, accumulator, and (for tool_use blocks)
//!     whether the source tool was custom (apply_patch et al.).
//!   * Top-level: response_id + model from `message_start`,
//!     cumulative usage rolled up across `message_start` and
//!     `message_delta`.
//!
//! Translator must be constructed with the set of custom tool names
//! the request side synthesized — that's how we know whether to emit
//! `function_call` or `custom_tool_call` shapes for a tool_use block.

use crate::anthropic::event::ContentBlock as InContent;
use crate::anthropic::event::ContentBlockDelta;
use crate::anthropic::event::ErrorPayload;
use crate::anthropic::event::MessageStart;
use crate::anthropic::event::StopReason;
use crate::anthropic::event::StreamEvent;
use crate::anthropic::event::Usage;
use crate::openai::OutputItem;
use crate::openai::ResponseObject;
use crate::openai::ResponseStreamEvent;
use crate::openai::ResponseUsage;
use crate::openai::ResponseUsageInputDetails;
use crate::openai::ResponseUsageOutputDetails;
use crate::translate::raw_string_extractor::RawStringExtractor;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;

/// Stateful per-stream translator. Construct with the set of tool
/// names the request translator synthesized as `custom` (via
/// `eager_input_streaming`); subsequent tool_use blocks for those
/// names are emitted as `custom_tool_call` items, everything else
/// as `function_call`.
pub struct StreamTranslator {
    custom_tool_names: HashSet<String>,
    response_id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
    message_delta_end_turn: Option<bool>,
    blocks: HashMap<u32, BlockState>,
    next_output_index: usize,
}

/// Tool name Anthropic invokes for the synthesized local-shell
/// function tool. Must match the `name` we register in
/// `translate/tools.rs::translate_local_shell()`.
const LOCAL_SHELL_TOOL_NAME: &str = "local_shell";

struct BlockState {
    output_index: usize,
    kind: BlockKind,
}

enum BlockKind {
    Text {
        item_id: String,
        accumulated: String,
    },
    Thinking {
        item_id: String,
        signature: String,
    },
    /// Round-trips a `redacted_thinking` content block as a Codex
    /// `Reasoning` item whose `encrypted_content` holds the opaque
    /// `data` payload (Anthropic's signature equivalent for
    /// safety-redacted thinking).
    RedactedThinking {
        item_id: String,
        data: String,
    },
    ToolUse {
        item_id: String,
        call_id: String,
        name: String,
        /// Raw JSON accumulator — used for non-custom tools where
        /// Codex receives the full `arguments` string at block_stop.
        input_json: String,
        is_custom: bool,
        /// True when the tool is the synthesized `local_shell` tool;
        /// the stop handler then emits an `OutputItem::LocalShellCall`
        /// instead of a generic `OutputItem::FunctionCall`, with the
        /// streaming JSON re-shaped into a Codex `LocalShellAction`.
        is_local_shell: bool,
        /// Per-block extractor used when `is_custom == true` to
        /// stream the unwrapped `raw` payload chunk-by-chunk to
        /// Codex.
        extractor: Option<RawStringExtractor>,
        /// Reconstructed raw payload for the final OutputItemDone
        /// event.
        raw_payload: String,
    },
    /// Server-tool blocks (web_search) — translator surfaces them
    /// as synthetic assistant text so Codex's TUI shows the search
    /// activity and result citations inline.
    WebSearchCall {
        item_id: String,
        query: String,
    },
    WebSearchResult {
        item_id: String,
        body: String,
    },
    /// Future server-tool variants we don't model yet — silently
    /// consumed so they don't break the stream.
    Dropped,
}

impl StreamTranslator {
    pub fn new(custom_tool_names: HashSet<String>) -> Self {
        Self {
            custom_tool_names,
            response_id: None,
            model: None,
            usage: None,
            message_delta_end_turn: None,
            blocks: HashMap::new(),
            next_output_index: 0,
        }
    }

    /// Translate one Anthropic event into zero or more Codex events.
    pub fn consume(&mut self, event: StreamEvent) -> Vec<ResponseStreamEvent> {
        match event {
            StreamEvent::MessageStart { message } => self.on_message_start(message),
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.on_block_start(index, content_block),
            StreamEvent::ContentBlockDelta { index, delta } => self.on_block_delta(index, delta),
            StreamEvent::ContentBlockStop { index } => self.on_block_stop(index),
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(usage) = usage {
                    self.merge_usage(usage);
                }
                self.message_delta_end_turn = stop_reason_to_end_turn(delta.stop_reason);
                Vec::new()
            }
            StreamEvent::MessageStop => self.on_message_stop(),
            StreamEvent::Ping => Vec::new(),
            StreamEvent::Error { error } => self.on_error(error),
        }
    }

    fn on_message_start(&mut self, message: MessageStart) -> Vec<ResponseStreamEvent> {
        self.response_id = Some(message.id.clone());
        self.model = Some(message.model.clone());
        self.merge_usage(message.usage);
        let _ = message.role; // Role is always Assistant in message_start.
        let response = self.fresh_response_object();
        vec![ResponseStreamEvent::Created { response }]
    }

    fn on_block_start(&mut self, index: u32, block: InContent) -> Vec<ResponseStreamEvent> {
        let output_index = self.next_output_index;
        self.next_output_index += 1;

        match block {
            InContent::Text { .. } => {
                let item_id = format!("msg_{output_index}");
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::Text {
                            item_id: item_id.clone(),
                            accumulated: String::new(),
                        },
                    },
                );
                vec![ResponseStreamEvent::OutputItemAdded {
                    output_index,
                    item: OutputItem::AssistantMessage {
                        id: item_id,
                        text: String::new(),
                    },
                }]
            }
            InContent::Thinking { signature, .. } => {
                let item_id = format!("rs_{output_index}");
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::Thinking {
                            item_id: item_id.clone(),
                            signature,
                        },
                    },
                );
                vec![
                    ResponseStreamEvent::OutputItemAdded {
                        output_index,
                        item: OutputItem::Reasoning {
                            id: item_id.clone(),
                            encrypted_content: None,
                        },
                    },
                    ResponseStreamEvent::ReasoningSummaryPartAdded {
                        item_id,
                        summary_index: 0,
                    },
                ]
            }
            InContent::RedactedThinking { data } => {
                // Redacted thinking carries no streaming text, only
                // the opaque `data` payload. Surface it as a Codex
                // Reasoning item with `encrypted_content = data` so
                // the next turn round-trips the redaction marker
                // back to Anthropic and the thinking-block validation
                // doesn't reject the request.
                let item_id = format!("rs_{output_index}");
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::RedactedThinking {
                            item_id: item_id.clone(),
                            data,
                        },
                    },
                );
                vec![ResponseStreamEvent::OutputItemAdded {
                    output_index,
                    item: OutputItem::Reasoning {
                        id: item_id,
                        encrypted_content: None,
                    },
                }]
            }
            InContent::ToolUse { id, name, .. } => {
                let is_local_shell = name == LOCAL_SHELL_TOOL_NAME;
                let is_custom = !is_local_shell && self.custom_tool_names.contains(&name);
                let item_id = format!("tc_{output_index}");
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::ToolUse {
                            item_id: item_id.clone(),
                            call_id: id.clone(),
                            name: name.clone(),
                            input_json: String::new(),
                            is_custom,
                            is_local_shell,
                            extractor: is_custom.then(RawStringExtractor::new),
                            raw_payload: String::new(),
                        },
                    },
                );
                let item = if is_local_shell {
                    // Pre-stop placeholder: the action is unknown
                    // until input_json finishes streaming. Emit a
                    // LocalShellCall with an empty exec action so
                    // Codex's parser sees the correct item type from
                    // the first event; the OutputItemDone replaces
                    // the action with the streamed shape.
                    OutputItem::LocalShellCall {
                        id: item_id,
                        call_id: id,
                        action: empty_exec_action(),
                    }
                } else if is_custom {
                    OutputItem::CustomToolCall {
                        id: item_id,
                        call_id: id,
                        name,
                        input: String::new(),
                    }
                } else {
                    OutputItem::FunctionCall {
                        id: item_id,
                        call_id: id,
                        name,
                        arguments: String::new(),
                    }
                };
                vec![ResponseStreamEvent::OutputItemAdded { output_index, item }]
            }
            InContent::ServerToolUse { name, input, .. } if name == "web_search" => {
                let query = input
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let item_id = format!("ws_{output_index}");
                let body = format!("\u{1F50E} Web search: {query}");
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::WebSearchCall {
                            item_id: item_id.clone(),
                            query,
                        },
                    },
                );
                vec![ResponseStreamEvent::OutputItemAdded {
                    output_index,
                    item: OutputItem::AssistantMessage {
                        id: item_id,
                        text: body,
                    },
                }]
            }
            InContent::WebSearchToolResult { content, .. } => {
                let item_id = format!("wsr_{output_index}");
                let body = format_web_search_result(&content);
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::WebSearchResult {
                            item_id: item_id.clone(),
                            body: body.clone(),
                        },
                    },
                );
                vec![ResponseStreamEvent::OutputItemAdded {
                    output_index,
                    item: OutputItem::AssistantMessage {
                        id: item_id,
                        text: body,
                    },
                }]
            }
            InContent::ServerToolUse { .. } => {
                // Non-web_search server tools (future versions) —
                // silently consume.
                self.blocks.insert(
                    index,
                    BlockState {
                        output_index,
                        kind: BlockKind::Dropped,
                    },
                );
                Vec::new()
            }
        }
    }

    fn on_block_delta(&mut self, index: u32, delta: ContentBlockDelta) -> Vec<ResponseStreamEvent> {
        // CitationDelta has no Codex-side equivalent and never
        // belongs to a per-block accumulator we track. Drop it
        // before the block-state lookup so it doesn't accidentally
        // mismatch a tool_use/text block.
        if matches!(delta, ContentBlockDelta::CitationDelta { .. }) {
            return Vec::new();
        }
        let Some(state) = self.blocks.get_mut(&index) else {
            return Vec::new();
        };
        let output_index = state.output_index;
        match (&mut state.kind, delta) {
            (
                BlockKind::Text {
                    item_id,
                    accumulated,
                },
                ContentBlockDelta::TextDelta { text },
            ) => {
                accumulated.push_str(&text);
                vec![ResponseStreamEvent::OutputTextDelta {
                    item_id: item_id.clone(),
                    content_index: 0,
                    delta: text,
                }]
            }
            (
                BlockKind::Thinking { item_id, .. },
                ContentBlockDelta::ThinkingDelta { thinking },
            ) => {
                vec![ResponseStreamEvent::ReasoningSummaryTextDelta {
                    item_id: item_id.clone(),
                    summary_index: 0,
                    delta: thinking,
                }]
            }
            (
                BlockKind::Thinking { signature, .. },
                ContentBlockDelta::SignatureDelta {
                    signature: incoming,
                },
            ) => {
                // Buffer the signature; it lands on output_item.done.
                signature.push_str(&incoming);
                Vec::new()
            }
            (
                BlockKind::ToolUse {
                    item_id,
                    call_id,
                    input_json,
                    extractor,
                    raw_payload,
                    ..
                },
                ContentBlockDelta::InputJsonDelta { partial_json },
            ) => {
                input_json.push_str(&partial_json);
                let _ = output_index;
                // For custom tools, run the chunk through the
                // streaming extractor and emit any newly-decoded raw
                // bytes immediately.
                let Some(extractor) = extractor.as_mut() else {
                    return Vec::new();
                };
                let decoded = extractor.push(&partial_json);
                if decoded.is_empty() {
                    return Vec::new();
                }
                raw_payload.push_str(&decoded);
                vec![ResponseStreamEvent::CustomToolCallInputDelta {
                    item_id: item_id.clone(),
                    call_id: call_id.clone(),
                    delta: decoded,
                }]
            }
            // Mismatched delta types (e.g. text_delta on a thinking block)
            // shouldn't happen but are silently dropped to keep the stream
            // robust.
            _ => Vec::new(),
        }
    }

    fn on_block_stop(&mut self, index: u32) -> Vec<ResponseStreamEvent> {
        let Some(state) = self.blocks.remove(&index) else {
            return Vec::new();
        };
        let output_index = state.output_index;
        match state.kind {
            BlockKind::Text {
                item_id,
                accumulated,
            } => {
                vec![ResponseStreamEvent::OutputItemDone {
                    output_index,
                    item: OutputItem::AssistantMessage {
                        id: item_id,
                        text: accumulated,
                    },
                }]
            }
            BlockKind::Thinking { item_id, signature } => {
                vec![ResponseStreamEvent::OutputItemDone {
                    output_index,
                    item: OutputItem::Reasoning {
                        id: item_id,
                        encrypted_content: signature_or_none(signature),
                    },
                }]
            }
            BlockKind::RedactedThinking { item_id, data } => {
                vec![ResponseStreamEvent::OutputItemDone {
                    output_index,
                    item: OutputItem::Reasoning {
                        id: item_id,
                        encrypted_content: Some(data),
                    },
                }]
            }
            BlockKind::ToolUse {
                item_id,
                call_id,
                name,
                input_json,
                is_custom,
                is_local_shell,
                raw_payload,
                ..
            } => {
                if is_local_shell {
                    let action = local_shell_action_from_input(&input_json);
                    vec![ResponseStreamEvent::OutputItemDone {
                        output_index,
                        item: OutputItem::LocalShellCall {
                            id: item_id,
                            call_id,
                            action,
                        },
                    }]
                } else if is_custom {
                    // The streaming extractor has already emitted the
                    // raw bytes per-chunk. If anything is left over
                    // (e.g. extractor never saw the closing quote
                    // because the upstream stream ended early), fall
                    // back to a one-shot extract from the accumulated
                    // JSON.
                    let final_input = if raw_payload.is_empty() {
                        extract_raw_input(&input_json)
                    } else {
                        raw_payload
                    };
                    vec![ResponseStreamEvent::OutputItemDone {
                        output_index,
                        item: OutputItem::CustomToolCall {
                            id: item_id,
                            call_id,
                            name,
                            input: final_input,
                        },
                    }]
                } else {
                    vec![ResponseStreamEvent::OutputItemDone {
                        output_index,
                        item: OutputItem::FunctionCall {
                            id: item_id,
                            call_id,
                            name,
                            arguments: input_json,
                        },
                    }]
                }
            }
            BlockKind::WebSearchCall { item_id, query } => {
                vec![ResponseStreamEvent::OutputItemDone {
                    output_index,
                    item: OutputItem::AssistantMessage {
                        id: item_id,
                        text: format!("\u{1F50E} Web search: {query}"),
                    },
                }]
            }
            BlockKind::WebSearchResult { item_id, body } => {
                vec![ResponseStreamEvent::OutputItemDone {
                    output_index,
                    item: OutputItem::AssistantMessage {
                        id: item_id,
                        text: body,
                    },
                }]
            }
            BlockKind::Dropped => Vec::new(),
        }
    }

    fn on_message_stop(&mut self) -> Vec<ResponseStreamEvent> {
        let mut response = self.fresh_response_object();
        if let Some(usage) = self.usage.take() {
            response.usage = Some(to_response_usage(usage));
        }
        response.end_turn = self.message_delta_end_turn.take();
        vec![ResponseStreamEvent::Completed { response }]
    }

    fn on_error(&mut self, error: ErrorPayload) -> Vec<ResponseStreamEvent> {
        let mut response = self.fresh_response_object();
        response.error = Some(serde_json::json!({
            "type": error.kind.as_wire_str(),
            "message": error.message,
        }));
        vec![ResponseStreamEvent::Failed { response }]
    }

    fn fresh_response_object(&self) -> ResponseObject {
        ResponseObject::new(
            self.response_id.clone().unwrap_or_default(),
            self.model.clone().unwrap_or_default(),
        )
    }

    fn merge_usage(&mut self, incoming: Usage) {
        // Anthropic message_delta usage is *cumulative* (per the
        // streaming docs). We just keep the latest non-zero values.
        let merged = match self.usage.take() {
            None => incoming,
            Some(prev) => Usage {
                input_tokens: nonzero(incoming.input_tokens, prev.input_tokens),
                output_tokens: nonzero(incoming.output_tokens, prev.output_tokens),
                cache_creation_input_tokens: nonzero(
                    incoming.cache_creation_input_tokens,
                    prev.cache_creation_input_tokens,
                ),
                cache_read_input_tokens: nonzero(
                    incoming.cache_read_input_tokens,
                    prev.cache_read_input_tokens,
                ),
                cache_creation: incoming.cache_creation.or(prev.cache_creation),
            },
        };
        self.usage = Some(merged);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stop_reason_to_end_turn(stop: Option<StopReason>) -> Option<bool> {
    let stop = stop?;
    Some(match stop {
        StopReason::EndTurn
        | StopReason::MaxTokens
        | StopReason::StopSequence
        | StopReason::Refusal
        | StopReason::Unknown => true,
        StopReason::ToolUse | StopReason::PauseTurn => false,
    })
}

fn signature_or_none(signature: String) -> Option<String> {
    if signature.is_empty() {
        None
    } else {
        Some(signature)
    }
}

fn nonzero(incoming: u64, fallback: u64) -> u64 {
    if incoming > 0 { incoming } else { fallback }
}

fn to_response_usage(usage: Usage) -> ResponseUsage {
    let total = usage.input_tokens + usage.output_tokens;
    let input_details = if usage.cache_read_input_tokens > 0 {
        Some(ResponseUsageInputDetails {
            cached_tokens: usage.cache_read_input_tokens,
        })
    } else {
        None
    };
    let output_details = (usage.output_tokens > 0).then_some(ResponseUsageOutputDetails {
        reasoning_tokens: 0,
    });
    ResponseUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: total,
        input_tokens_details: input_details,
        output_tokens_details: output_details,
    }
}

/// Extract the `raw` field from `{"raw": "..."}` accumulator. Falls
/// back to the entire accumulator string if the JSON parse fails or
/// the shape doesn't match — better to surface the raw bytes than
/// drop the tool call entirely.
fn extract_raw_input(accumulator: &str) -> String {
    serde_json::from_str::<Value>(accumulator)
        .ok()
        .and_then(|v| v.get("raw").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| accumulator.to_string())
}

/// Build an empty `LocalShellAction::Exec` JSON object — used as the
/// pre-stop placeholder action on `OutputItemAdded` so Codex sees the
/// item type immediately, before the streaming `input_json_delta`
/// chunks have committed the actual `command` array.
fn empty_exec_action() -> Value {
    serde_json::json!({
        "type": "exec",
        "command": [],
    })
}

/// Reshape the streamed tool-use JSON into a Codex-compatible
/// `LocalShellAction::Exec` value. Anthropic's `tool_use.input` for
/// our synthesized `local_shell` function tool is the
/// `LocalShellAction` schema we registered (i.e. already
/// `{type:"exec", command:[...]}`) per
/// `translate/tools.rs::translate_local_shell()`. If the model adds
/// rationale fields or wraps the action under an `action` key (some
/// adapters do), unwrap one level so Codex's
/// `protocol/src/models.rs::LocalShellAction` deserializer sees the
/// expected shape. Falls back to a benign empty exec action when the
/// payload is unusable rather than crashing the turn.
fn local_shell_action_from_input(accumulator: &str) -> Value {
    let Ok(parsed) = serde_json::from_str::<Value>(accumulator) else {
        return empty_exec_action();
    };
    if let Some(inner) = parsed.get("action").cloned()
        && action_looks_like_exec(&inner)
    {
        return inner;
    }
    if action_looks_like_exec(&parsed) {
        return parsed;
    }
    empty_exec_action()
}

fn action_looks_like_exec(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("exec")
        && value.get("command").is_some_and(Value::is_array)
}

/// Format an Anthropic `web_search_tool_result.content` payload into
/// human-readable assistant text. Handles both the success case
/// (array of `web_search_result` items with title/url/page_age) and
/// the error case (`web_search_tool_result_error` object).
fn format_web_search_result(content: &Value) -> String {
    if let Some(error_code) = content
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| *kind == "web_search_tool_result_error")
        .and_then(|_| content.get("error_code"))
        .and_then(Value::as_str)
    {
        return format!("\u{26A0} Web search error: {error_code}");
    }

    let Some(results) = content.as_array() else {
        return "Web search returned no results.".into();
    };
    let mut out = String::from("Web search results:");
    for item in results {
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled");
        let url = item.get("url").and_then(Value::as_str).unwrap_or("");
        let age = item
            .get("page_age")
            .and_then(Value::as_str)
            .map(|age| format!(" ({age})"))
            .unwrap_or_default();
        out.push_str(&format!("\n  \u{2022} {title}{age} \u{2014} {url}"));
    }
    out
}
