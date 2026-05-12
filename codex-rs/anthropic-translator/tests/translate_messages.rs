//! Translate Codex's `input: Vec<ResponseItem>` into the Anthropic
//! `messages: Vec<Message>` array, including the role-grouping
//! gymnastics Anthropic requires.
//!
//! Constraints encoded by these tests:
//!
//!   * Anthropic requires alternating user/assistant roles. The
//!     translator must merge consecutive same-role items.
//!   * `tool_result` blocks must appear FIRST in a user message's
//!     content (Anthropic 400s otherwise).
//!   * `thinking` blocks must precede the `tool_use` block they
//!     belong to in an assistant message.
//!   * `Reasoning` round-trips encrypted content into `signature`.
//!   * Assistant turn boundaries (last assistant message before a
//!     new user message) are surfaced so the cache planner can pin
//!     breakpoints there.

use codex_anthropic_translator::anthropic::ContentBlock;
use codex_anthropic_translator::anthropic::ImageSource;
use codex_anthropic_translator::anthropic::Message;
use codex_anthropic_translator::anthropic::Role;
use codex_anthropic_translator::anthropic::ToolResultContent;
use codex_anthropic_translator::openai::ContentItem;
use codex_anthropic_translator::openai::ReasoningSummaryItem;
use codex_anthropic_translator::openai::ResponseItem;
use codex_anthropic_translator::translate::TranslatedMessages;
use codex_anthropic_translator::translate::translate_messages;
use pretty_assertions::assert_eq;
use serde_json::json;

fn user_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![ContentItem::InputText { text: text.into() }],
        phase: None,
    }
}

fn assistant_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".into(),
        content: vec![ContentItem::OutputText { text: text.into() }],
        phase: None,
    }
}

fn translate(items: Vec<ResponseItem>) -> TranslatedMessages {
    translate_messages(items)
}

// ---------------------------------------------------------------------------
// Basic round-trips
// ---------------------------------------------------------------------------

#[test]
fn single_user_text_message_round_trips() {
    let result = translate(vec![user_text("Hello")]);
    assert_eq!(
        result.messages,
        vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text("Hello")],
        }],
    );
    assert!(result.assistant_turn_boundaries.is_empty());
}

#[test]
fn assistant_text_message_round_trips() {
    let result = translate(vec![user_text("hi"), assistant_text("there")]);
    assert_eq!(
        result.messages[1],
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text("there")],
        },
    );
    assert_eq!(result.assistant_turn_boundaries, vec![1]);
}

#[test]
fn input_image_round_trips_as_url_image_block() {
    let result = translate(vec![ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![
            ContentItem::InputText {
                text: "describe:".into(),
            },
            ContentItem::InputImage {
                image_url: "https://example.com/x.png".into(),
                detail: None,
            },
        ],
        phase: None,
    }]);
    assert_eq!(result.messages.len(), 1);
    let user = &result.messages[0];
    assert_eq!(user.role, Role::User);
    assert_eq!(user.content.len(), 2);
    let ContentBlock::Image { source, .. } = &user.content[1] else {
        panic!("expected image block");
    };
    let ImageSource::Url { url } = source else {
        panic!("expected url image source");
    };
    assert_eq!(url, "https://example.com/x.png");
}

// ---------------------------------------------------------------------------
// Role grouping
// ---------------------------------------------------------------------------

#[test]
fn consecutive_user_messages_are_merged_into_one() {
    let result = translate(vec![user_text("first"), user_text("second")]);
    assert_eq!(result.messages.len(), 1, "must merge same-role neighbours");
    assert_eq!(
        result.messages[0].content,
        vec![ContentBlock::text("first"), ContentBlock::text("second")],
    );
}

#[test]
fn consecutive_assistant_messages_are_merged_into_one() {
    let result = translate(vec![
        user_text("u"),
        assistant_text("a1"),
        assistant_text("a2"),
    ]);
    assert_eq!(result.messages.len(), 2);
    assert_eq!(
        result.messages[1].content,
        vec![ContentBlock::text("a1"), ContentBlock::text("a2")],
    );
}

#[test]
fn alternating_messages_stay_separate() {
    let result = translate(vec![
        user_text("u1"),
        assistant_text("a1"),
        user_text("u2"),
        assistant_text("a2"),
    ]);
    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.assistant_turn_boundaries, vec![1, 3]);
}

// ---------------------------------------------------------------------------
// Function call + output → assistant tool_use, user tool_result
// ---------------------------------------------------------------------------

#[test]
fn function_call_becomes_assistant_tool_use_block() {
    let result = translate(vec![
        user_text("ls"),
        ResponseItem::FunctionCall {
            id: None,
            name: "shell".into(),
            namespace: None,
            arguments: "{\"cmd\":\"ls\"}".into(),
            call_id: "call_1".into(),
        },
    ]);
    assert_eq!(result.messages.len(), 2);
    let assistant = &result.messages[1];
    assert_eq!(assistant.role, Role::Assistant);
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = &assistant.content[0]
    else {
        panic!("expected tool_use");
    };
    assert_eq!(id, "call_1");
    assert_eq!(name, "shell");
    assert_eq!(input, &json!({"cmd": "ls"}));
}

// `function_call_output.output` is serialized by Codex as either a
// bare JSON string or an array of input_text/input_image content
// items (see `codex-rs/protocol/src/models.rs:1459-1469`,
// `impl Serialize for FunctionCallOutputPayload`). The OpenAI
// Responses API documents the same union:
// <https://developers.openai.com/api/docs/api-reference/responses/object>
// > "Can be a string or an list of output content."
// The `success` field on Codex's internal `FunctionCallOutputPayload`
// is metadata and is never serialized — `is_error` cannot be inferred
// from the wire payload, so it always defaults to false here.
#[test]
fn function_call_output_with_string_payload_becomes_user_tool_result() {
    let result = translate(vec![
        user_text("ls"),
        ResponseItem::FunctionCall {
            id: None,
            name: "shell".into(),
            namespace: None,
            arguments: "{}".into(),
            call_id: "call_1".into(),
        },
        ResponseItem::FunctionCallOutput {
            call_id: "call_1".into(),
            // Real wire shape: bare JSON string. This is what Codex
            // emits for every shell/exec_command/non-image tool result.
            output: json!("file1\nfile2"),
        },
    ]);
    assert_eq!(result.messages.len(), 3);
    let user_with_result = &result.messages[2];
    assert_eq!(user_with_result.role, Role::User);
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error,
        ..
    } = &user_with_result.content[0]
    else {
        panic!("expected tool_result");
    };
    assert_eq!(tool_use_id, "call_1");
    assert!(!is_error);
    let ToolResultContent::Text(text) = content else {
        panic!("expected text content");
    };
    assert_eq!(text, "file1\nfile2");
}

#[test]
fn function_call_output_with_text_only_content_items_array_becomes_text() {
    // Codex's `FunctionCallOutputContentItem::InputText` carries plain
    // text. When the array is text-only we collapse into a single
    // string `ToolResultContent::Text`, which is the most cache-
    // friendly shape on the Anthropic side.
    let result = translate(vec![ResponseItem::FunctionCallOutput {
        call_id: "call_1".into(),
        output: json!([
            {"type": "input_text", "text": "line one"},
            {"type": "input_text", "text": "line two"},
        ]),
    }]);
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = &result.messages[0].content[0]
    else {
        panic!("expected tool_result");
    };
    assert!(!is_error);
    let ToolResultContent::Text(text) = content else {
        panic!("expected text content");
    };
    assert_eq!(text, "line one\nline two");
}

#[test]
fn function_call_output_with_mixed_content_items_becomes_blocks() {
    // Image-bearing tool results must land as `Blocks` so the image
    // survives end-to-end. We keep text and image blocks in source
    // order; this matches Anthropic's `tool_result.content` array
    // semantics (TextBlockParam | ImageBlockParam).
    let result = translate(vec![ResponseItem::FunctionCallOutput {
        call_id: "call_1".into(),
        output: json!([
            {"type": "input_text", "text": "before"},
            {"type": "input_image", "image_url": "https://example.com/x.png"},
            {"type": "input_text", "text": "after"},
        ]),
    }]);
    let ContentBlock::ToolResult { content, .. } = &result.messages[0].content[0] else {
        panic!("expected tool_result");
    };
    let ToolResultContent::Blocks(blocks) = content else {
        panic!("expected blocks content for mixed text+image output");
    };
    assert_eq!(blocks.len(), 3);
    assert!(matches!(&blocks[0], ContentBlock::Text { text, .. } if text == "before"));
    assert!(matches!(
        &blocks[1],
        ContentBlock::Image {
            source: ImageSource::Url { url },
            ..
        } if url == "https://example.com/x.png"
    ));
    assert!(matches!(&blocks[2], ContentBlock::Text { text, .. } if text == "after"));
}

#[test]
fn function_call_output_with_empty_content_items_array_becomes_empty_text() {
    // Defensive: an empty array shouldn't drop the tool_result
    // entirely (Anthropic 400s on assistant tool_use without a
    // matching user tool_result). Keep the result block with an
    // empty text payload so the conversation stays well-formed.
    let result = translate(vec![ResponseItem::FunctionCallOutput {
        call_id: "call_1".into(),
        output: json!([]),
    }]);
    let ContentBlock::ToolResult { content, .. } = &result.messages[0].content[0] else {
        panic!("expected tool_result");
    };
    let ToolResultContent::Text(text) = content else {
        panic!("expected text content for empty array");
    };
    assert_eq!(text, "");
}

#[test]
fn function_call_output_with_empty_string_keeps_tool_result_block() {
    // Same well-formedness invariant as above for the bare-string
    // path: never drop the tool_result, just emit empty text.
    let result = translate(vec![ResponseItem::FunctionCallOutput {
        call_id: "call_1".into(),
        output: json!(""),
    }]);
    let ContentBlock::ToolResult { content, .. } = &result.messages[0].content[0] else {
        panic!("expected tool_result");
    };
    let ToolResultContent::Text(text) = content else {
        panic!("expected text content");
    };
    assert_eq!(text, "");
}

#[test]
fn function_call_arguments_with_invalid_json_pass_through_as_string() {
    // If Codex (or the model) sends garbage in `arguments`, we still
    // need to round-trip it — Anthropic's `input` accepts any JSON
    // value. The translator wraps the raw string under a `__raw_input`
    // key so the round-trip is observable.
    let result = translate(vec![ResponseItem::FunctionCall {
        id: None,
        name: "shell".into(),
        namespace: None,
        arguments: "not actually json".into(),
        call_id: "call_x".into(),
    }]);
    let ContentBlock::ToolUse { input, .. } = &result.messages[0].content[0] else {
        panic!("expected tool_use");
    };
    assert_eq!(input, &json!({"__raw_input": "not actually json"}));
}

// ---------------------------------------------------------------------------
// Custom tool call (apply_patch) → assistant tool_use with raw payload
// ---------------------------------------------------------------------------

#[test]
fn custom_tool_call_wraps_raw_string_under_raw_key() {
    let result = translate(vec![ResponseItem::CustomToolCall {
        id: None,
        status: None,
        call_id: "call_p".into(),
        name: "apply_patch".into(),
        input: "*** Begin Patch\n*** End Patch\n".into(),
    }]);
    assert_eq!(result.messages.len(), 1);
    let ContentBlock::ToolUse { name, input, .. } = &result.messages[0].content[0] else {
        panic!("expected tool_use");
    };
    assert_eq!(name, "apply_patch");
    assert_eq!(input, &json!({"raw": "*** Begin Patch\n*** End Patch\n"}));
}

#[test]
fn custom_tool_call_output_becomes_user_tool_result() {
    // CustomToolCallOutput shares the FunctionCallOutputPayload
    // serializer (see protocol/src/models.rs:1459-1469) so its `output`
    // is also the bare-string-or-content-items union.
    let result = translate(vec![ResponseItem::CustomToolCallOutput {
        call_id: "call_p".into(),
        name: Some("apply_patch".into()),
        output: json!("patch applied"),
    }]);
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        ..
    } = &result.messages[0].content[0]
    else {
        panic!("expected tool_result");
    };
    assert_eq!(tool_use_id, "call_p");
    let ToolResultContent::Text(text) = content else {
        panic!("expected text content");
    };
    assert_eq!(text, "patch applied");
}

// ---------------------------------------------------------------------------
// Reasoning round-trip with signature
// ---------------------------------------------------------------------------

#[test]
fn reasoning_with_encrypted_content_becomes_thinking_with_signature() {
    let result = translate(vec![ResponseItem::Reasoning {
        id: Some("rs_1".into()),
        summary: vec![ReasoningSummaryItem::SummaryText {
            text: "Step one...".into(),
        }],
        content: None,
        encrypted_content: Some("EQ1...".into()),
    }]);
    assert_eq!(result.messages.len(), 1);
    let ContentBlock::Thinking {
        thinking,
        signature,
    } = &result.messages[0].content[0]
    else {
        panic!("expected thinking block");
    };
    // For Opus 4.7 with display:omitted, thinking text on the way back
    // should be empty (any content there is ignored by Anthropic). We
    // emit empty to match the documented round-trip rule.
    assert_eq!(thinking, "");
    assert_eq!(signature, "EQ1...");
}

#[test]
fn reasoning_without_encrypted_content_is_dropped() {
    // Per the docs: a thinking block must include both `thinking`
    // and `signature`. Without a signature we can't form a valid
    // block, so we drop the reasoning entirely.
    let result = translate(vec![
        user_text("hi"),
        ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: None,
            encrypted_content: None,
        },
    ]);
    assert_eq!(result.messages.len(), 1, "reasoning without sig dropped");
}

// ---------------------------------------------------------------------------
// Reasoning + tool_use must precede tool_use in the same assistant message
// ---------------------------------------------------------------------------

#[test]
fn reasoning_function_call_pair_share_one_assistant_message_with_thinking_first() {
    let result = translate(vec![
        user_text("do X"),
        ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: None,
            encrypted_content: Some("ENC".into()),
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "shell".into(),
            namespace: None,
            arguments: "{}".into(),
            call_id: "c1".into(),
        },
    ]);
    assert_eq!(result.messages.len(), 2, "user + assistant");
    let assistant = &result.messages[1];
    assert_eq!(assistant.role, Role::Assistant);
    assert_eq!(
        assistant.content.len(),
        2,
        "thinking + tool_use share one msg"
    );
    assert!(matches!(
        assistant.content[0],
        ContentBlock::Thinking { .. }
    ));
    assert!(matches!(assistant.content[1], ContentBlock::ToolUse { .. }));
}

// ---------------------------------------------------------------------------
// LocalShellCall → tool_use with action input
// ---------------------------------------------------------------------------

#[test]
fn local_shell_call_becomes_tool_use_with_action_as_input() {
    let result = translate(vec![ResponseItem::LocalShellCall {
        id: None,
        call_id: Some("ls_1".into()),
        status: "completed".into(),
        action: json!({"type": "exec", "command": ["/bin/sh", "-c", "ls"]}),
    }]);
    assert_eq!(result.messages.len(), 1);
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = &result.messages[0].content[0]
    else {
        panic!("expected tool_use");
    };
    assert_eq!(id, "ls_1");
    assert_eq!(name, "local_shell");
    assert_eq!(
        input,
        &json!({"type": "exec", "command": ["/bin/sh", "-c", "ls"]})
    );
}

// ---------------------------------------------------------------------------
// Unrecognized items are dropped
// ---------------------------------------------------------------------------

#[test]
fn unrecognized_items_are_dropped() {
    let result = translate(vec![
        user_text("hi"),
        ResponseItem::Unrecognized,
        assistant_text("hello"),
    ]);
    assert_eq!(result.messages.len(), 2, "unrecognized items dropped");
}

// ---------------------------------------------------------------------------
// Assistant turn boundaries
// ---------------------------------------------------------------------------

#[test]
fn assistant_turn_boundaries_track_each_assistant_message_index() {
    let result = translate(vec![
        user_text("u1"),
        ResponseItem::FunctionCall {
            id: None,
            name: "shell".into(),
            namespace: None,
            arguments: "{}".into(),
            call_id: "c1".into(),
        },
        ResponseItem::FunctionCallOutput {
            call_id: "c1".into(),
            output: json!("x"),
        },
        assistant_text("done."),
    ]);
    // Messages: [u, a(tool_use), u(tool_result), a(text)]
    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.assistant_turn_boundaries, vec![1, 3]);
}
