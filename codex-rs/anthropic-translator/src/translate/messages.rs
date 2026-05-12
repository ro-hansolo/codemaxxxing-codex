//! Translate Codex's `input: Vec<ResponseItem>` into Anthropic
//! `messages: Vec<Message>`.
//!
//! The translator handles four jobs in one pass:
//!
//!   1. Convert each `ResponseItem` to its Anthropic content-block
//!      equivalent.
//!   2. Decide whether each block lives in a `user` or `assistant`
//!      message (Anthropic's role grouping is rigid: tool_results
//!      go in user messages, tool_use/thinking in assistant messages).
//!   3. Merge consecutive same-role outputs (Anthropic 400s on two
//!      assistant messages in a row).
//!   4. Surface the indices of completed assistant turns so the cache
//!      planner can pin breakpoints there.

use crate::anthropic::ContentBlock;
use crate::anthropic::ImageSource;
use crate::anthropic::Message;
use crate::anthropic::Role;
use crate::anthropic::ToolResultContent;
use crate::openai::ContentItem;
use crate::openai::ResponseItem;
use serde_json::Value;
use serde_json::json;

/// Result of translating Codex's input array.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslatedMessages {
    /// Anthropic-shaped messages, ready for the request builder.
    pub messages: Vec<Message>,
    /// Indices into [`Self::messages`] where an assistant turn ends
    /// (oldest first). The cache planner uses these to anchor message-
    /// tail breakpoints.
    pub assistant_turn_boundaries: Vec<usize>,
}

/// Convert a Codex input array into Anthropic messages with
/// turn-boundary metadata.
pub fn translate_messages(items: Vec<ResponseItem>) -> TranslatedMessages {
    let mut builder = Builder::default();
    for item in items {
        builder.consume(item);
    }
    builder.finish()
}

#[derive(Default)]
struct Builder {
    messages: Vec<Message>,
    pending: Option<Pending>,
    boundaries: Vec<usize>,
}

struct Pending {
    role: Role,
    content: Vec<ContentBlock>,
}

impl Builder {
    fn consume(&mut self, item: ResponseItem) {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let target = parse_role(&role);
                let blocks = content.into_iter().map(content_item_to_block).collect();
                self.append(target, blocks);
            }
            ResponseItem::Reasoning {
                encrypted_content: Some(signature),
                ..
            } => {
                let block = ContentBlock::Thinking {
                    // Empty `thinking`: Anthropic explicitly states the
                    // text field is ignored on round-trip when display
                    // was `omitted`. Sending empty avoids any chance of
                    // accidentally leaking summarized text we round-
                    // tripped through Codex's UI.
                    thinking: String::new(),
                    signature,
                };
                self.append(Role::Assistant, vec![block]);
            }
            ResponseItem::Reasoning {
                encrypted_content: None,
                ..
            } => {
                // Cannot form a valid thinking block without signature.
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let input = parse_arguments(&arguments);
                let block = ContentBlock::ToolUse {
                    id: call_id,
                    name,
                    input,
                    cache_control: None,
                };
                self.append(Role::Assistant, vec![block]);
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                let content = parse_output(&output);
                self.append(Role::User, vec![tool_result(call_id, content)]);
            }
            ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
                ..
            } => {
                let block = ContentBlock::ToolUse {
                    id: call_id,
                    name,
                    input: json!({ "raw": input }),
                    cache_control: None,
                };
                self.append(Role::Assistant, vec![block]);
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                let content = parse_output(&output);
                self.append(Role::User, vec![tool_result(call_id, content)]);
            }
            ResponseItem::LocalShellCall {
                call_id, action, ..
            } => {
                let block = ContentBlock::ToolUse {
                    id: call_id.unwrap_or_default(),
                    name: "local_shell".into(),
                    input: action,
                    cache_control: None,
                };
                self.append(Role::Assistant, vec![block]);
            }
            ResponseItem::Unrecognized => {
                // Forward-compat catch-all: drop silently. The request
                // translator may add a system note in a future revision.
            }
        }
    }

    fn append(&mut self, role: Role, blocks: Vec<ContentBlock>) {
        if blocks.is_empty() {
            return;
        }
        match &mut self.pending {
            Some(pending) if pending.role == role => {
                pending.content.extend(blocks);
            }
            _ => {
                self.flush();
                self.pending = Some(Pending {
                    role,
                    content: blocks,
                });
            }
        }
    }

    fn flush(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.content.is_empty() {
            return;
        }
        let index = self.messages.len();
        let role = pending.role;
        self.messages.push(Message {
            role,
            content: pending.content,
        });
        if matches!(role, Role::Assistant) {
            self.boundaries.push(index);
        }
    }

    fn finish(mut self) -> TranslatedMessages {
        self.flush();
        TranslatedMessages {
            messages: self.messages,
            assistant_turn_boundaries: self.boundaries,
        }
    }
}

fn parse_role(role: &str) -> Role {
    if role == "assistant" {
        Role::Assistant
    } else {
        Role::User
    }
}

fn content_item_to_block(item: ContentItem) -> ContentBlock {
    match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
            ContentBlock::text(text)
        }
        ContentItem::InputImage {
            image_url,
            // Codex's `detail` axis (`auto`/`low`/`high`/`original`)
            // has no Anthropic equivalent on URLImageSource — drop
            // it explicitly so future maintainers don't think
            // there's a missing translation step.
            detail: _,
        } => ContentBlock::Image {
            source: ImageSource::Url { url: image_url },
            cache_control: None,
        },
    }
}

/// Codex emits tool-call arguments as a JSON-encoded *string*; we
/// re-parse to get a structured `input` for Anthropic. If the string
/// isn't valid JSON we wrap it as `{"__raw_input": <string>}` so the
/// model still sees the data on the next turn.
fn parse_arguments(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({ "__raw_input": raw }))
}

/// Decode the `output` value of a `function_call_output` /
/// `custom_tool_call_output` item into Anthropic
/// `tool_result.content`.
///
/// The wire shape is the union documented in the OpenAI Responses
/// API reference (`function_call_output.output` =
/// "string or an list of output content") and emitted by
/// `impl Serialize for FunctionCallOutputPayload` in
/// `codex-rs/protocol/src/models.rs:1459-1469`. Concretely:
///
///   * `Value::String(s)` — collapsed `Text` content.
///   * `Value::Array(items)` — each item is a
///     `FunctionCallOutputContentItem` (`input_text` or
///     `input_image`). A text-only array is collapsed back to
///     `Text` for cache-friendliness; otherwise we emit `Blocks` so
///     image content survives the round-trip.
///   * Anything else (numbers, bools, null, or — in legacy fixtures
///     — an object) falls back to its JSON-stringified form so
///     nothing is dropped silently.
fn parse_output(value: &Value) -> ToolResultContent {
    match value {
        Value::String(text) => ToolResultContent::Text(text.clone()),
        Value::Array(items) => parse_output_items(items),
        Value::Null => ToolResultContent::Text(String::new()),
        other => ToolResultContent::Text(other.to_string()),
    }
}

fn parse_output_items(items: &[Value]) -> ToolResultContent {
    if items.is_empty() {
        return ToolResultContent::Text(String::new());
    }
    let blocks: Vec<ContentBlock> = items.iter().filter_map(content_item_value).collect();
    if blocks.is_empty() {
        return ToolResultContent::Text(String::new());
    }
    if blocks.iter().all(is_text_block) {
        let joined = blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return ToolResultContent::Text(joined);
    }
    ToolResultContent::Blocks(blocks)
}

fn is_text_block(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Text { .. })
}

/// Map a single Codex `FunctionCallOutputContentItem` (one element of
/// the `output` array) to its Anthropic `tool_result` block. Unknown
/// item types are dropped — Codex's enum is closed over `input_text`
/// and `input_image` (see `protocol/src/models.rs::FunctionCallOutputContentItem`),
/// so this is forward-compat only.
fn content_item_value(item: &Value) -> Option<ContentBlock> {
    let kind = item.get("type")?.as_str()?;
    match kind {
        "input_text" | "output_text" | "text" => {
            let text = item.get("text")?.as_str()?.to_string();
            Some(ContentBlock::text(text))
        }
        "input_image" | "image" => {
            let url = item.get("image_url")?.as_str()?.to_string();
            Some(ContentBlock::Image {
                source: ImageSource::Url { url },
                cache_control: None,
            })
        }
        _ => None,
    }
}

fn tool_result(call_id: String, content: ToolResultContent) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: call_id,
        content,
        // `is_error` cannot be inferred from the wire payload — Codex's
        // `FunctionCallOutputPayload::success` is internal metadata and
        // is intentionally not serialized (see
        // `codex-rs/protocol/src/models.rs:1459-1469`). Tools encode
        // failure inside their textual body or via the structured
        // `output_schema` (e.g. `exit_code` for `exec_command`).
        is_error: false,
        cache_control: None,
    }
}
