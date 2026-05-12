//! Wire-format contract tests for incoming Codex (OpenAI Responses
//! API) request types.
//!
//! Each test pins one part of the JSON shape Codex sends to
//! `POST /v1/responses`, against the Codex source as of 2026-05-12:
//!
//!   * Request body: `codex-rs/codex-api/src/common.rs`
//!     (`ResponsesApiRequest`)
//!   * Items:        `codex-rs/protocol/src/models.rs`
//!     (`ResponseItem`)
//!
//! These tests are the source of truth for what shapes the translator
//! must accept. Any change to Codex's outgoing wire format that
//! breaks one of these tests demands an explicit translator update.

use codex_anthropic_translator::openai::ContentItem;
use codex_anthropic_translator::openai::Reasoning;
use codex_anthropic_translator::openai::ReasoningContentItem;
use codex_anthropic_translator::openai::ReasoningEffort;
use codex_anthropic_translator::openai::ReasoningSummary;
use codex_anthropic_translator::openai::ReasoningSummaryItem;
use codex_anthropic_translator::openai::ResponseItem;
use codex_anthropic_translator::openai::ResponsesRequest;
use codex_anthropic_translator::openai::TextControls;
use codex_anthropic_translator::openai::TextFormat;
use codex_anthropic_translator::openai::Verbosity;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn parse_request(value: Value) -> ResponsesRequest {
    match serde_json::from_value::<ResponsesRequest>(value) {
        Ok(req) => req,
        Err(err) => panic!("ResponsesRequest must deserialize: {err}"),
    }
}

fn parse_item(value: Value) -> ResponseItem {
    match serde_json::from_value::<ResponseItem>(value) {
        Ok(item) => item,
        Err(err) => panic!("ResponseItem must deserialize: {err}"),
    }
}

// ---------------------------------------------------------------------------
// Top-level request shape
// ---------------------------------------------------------------------------

#[test]
fn minimal_request_with_required_fields_only() {
    // Codex always sends model, input, tools, tool_choice,
    // parallel_tool_calls, store, stream, include — even when empty.
    // Everything else is optional.
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "reasoning": null,
        "store": false,
        "stream": true,
        "include": [],
    }));
    assert_eq!(req.model, "claude-opus-4-7");
    assert_eq!(req.instructions, "");
    assert!(req.input.is_empty());
    assert!(req.tools.is_empty());
    assert_eq!(req.tool_choice, "auto");
    assert!(req.parallel_tool_calls);
    assert!(req.reasoning.is_none());
    assert!(!req.store);
    assert!(req.stream);
    assert!(req.include.is_empty());
    assert!(req.service_tier.is_none());
    assert!(req.prompt_cache_key.is_none());
    assert!(req.text.is_none());
    assert!(req.client_metadata.is_none());
}

#[test]
fn request_with_prompt_cache_key_for_thread_state() {
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "system text",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
        "prompt_cache_key": "thread-abc-123",
    }));
    assert_eq!(req.prompt_cache_key.as_deref(), Some("thread-abc-123"));
}

#[test]
fn request_with_client_metadata_round_trips() {
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
        "client_metadata": {
            "x-codex-installation-id": "install-abc",
        },
    }));
    let metadata = req.client_metadata.expect("metadata present");
    assert_eq!(
        metadata.get("x-codex-installation-id").map(String::as_str),
        Some("install-abc"),
    );
}

#[test]
fn request_with_include_field_round_trips() {
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
    }));
    assert_eq!(req.include, vec!["reasoning.encrypted_content"]);
}

#[test]
fn request_with_service_tier_round_trips() {
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
        "service_tier": "priority",
    }));
    assert_eq!(req.service_tier.as_deref(), Some("priority"));
}

#[test]
fn request_preserves_tools_array_as_opaque_values() {
    // The translator re-parses tools into Anthropic shape; here we
    // just confirm the array round-trips losslessly as JSON values
    // so no field is silently dropped.
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "",
        "input": [],
        "tools": [
            {"type": "function", "name": "shell", "parameters": {"type": "object"}},
            {"type": "custom", "name": "apply_patch", "format": {"syntax": "lark", "definition": "..."}},
            {"type": "local_shell"},
        ],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
    }));
    assert_eq!(req.tools.len(), 3);
    assert_eq!(req.tools[0]["name"], json!("shell"));
    assert_eq!(req.tools[1]["name"], json!("apply_patch"));
    assert_eq!(req.tools[2]["type"], json!("local_shell"));
}

// ---------------------------------------------------------------------------
// Reasoning
// ---------------------------------------------------------------------------

#[test]
fn reasoning_with_effort_and_summary() {
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
        "reasoning": {"effort": "high", "summary": "auto"},
    }));
    let reasoning = req.reasoning.expect("reasoning present");
    assert_eq!(reasoning.effort, Some(ReasoningEffort::High));
    assert_eq!(reasoning.summary, Some(ReasoningSummary::Auto));
}

#[test]
fn reasoning_effort_serializes_each_variant() {
    // The full set of variants Codex exposes per
    // `codex-rs/protocol/src/openai_models.rs::ReasoningEffort`.
    // Without `#[serde(other)]`, missing variants would crash the
    // ENTIRE request deserialization (silent turn failure).
    for (wire, expected) in [
        ("none", ReasoningEffort::None),
        ("minimal", ReasoningEffort::Minimal),
        ("low", ReasoningEffort::Low),
        ("medium", ReasoningEffort::Medium),
        ("high", ReasoningEffort::High),
        ("xhigh", ReasoningEffort::XHigh),
    ] {
        let parsed: Reasoning =
            serde_json::from_value(json!({"effort": wire})).expect("parse Reasoning");
        assert_eq!(parsed.effort, Some(expected), "for wire effort {wire:?}");
    }
}

#[test]
fn reasoning_summary_handles_each_variant() {
    for (wire, expected) in [
        ("auto", ReasoningSummary::Auto),
        ("concise", ReasoningSummary::Concise),
        ("detailed", ReasoningSummary::Detailed),
        ("none", ReasoningSummary::None),
    ] {
        let parsed: Reasoning =
            serde_json::from_value(json!({"summary": wire})).expect("parse Reasoning");
        assert_eq!(parsed.summary, Some(expected), "for wire summary {wire:?}");
    }
}

// ---------------------------------------------------------------------------
// Text controls (verbosity + structured output)
// ---------------------------------------------------------------------------

#[test]
fn text_controls_with_verbosity_only() {
    let parsed: TextControls =
        serde_json::from_value(json!({"verbosity": "high"})).expect("parse TextControls");
    assert_eq!(parsed.verbosity, Some(Verbosity::High));
    assert!(parsed.format.is_none());
}

#[test]
fn text_controls_with_json_schema_format() {
    let parsed: TextControls = serde_json::from_value(json!({
        "format": {
            "type": "json_schema",
            "schema": {"type": "object", "properties": {}},
            "strict": true,
            "name": "codex_output_schema",
        },
    }))
    .expect("parse TextControls");
    let TextFormat::JsonSchema {
        schema,
        strict,
        name,
    } = parsed.format.expect("format present");
    assert_eq!(schema, json!({"type": "object", "properties": {}}));
    assert!(strict);
    assert_eq!(name, "codex_output_schema");
}

#[test]
fn verbosity_handles_each_variant() {
    for (wire, expected) in [
        ("low", Verbosity::Low),
        ("medium", Verbosity::Medium),
        ("high", Verbosity::High),
    ] {
        let parsed: TextControls =
            serde_json::from_value(json!({"verbosity": wire})).expect("parse TextControls");
        assert_eq!(parsed.verbosity, Some(expected), "for wire {wire:?}");
    }
}

// ---------------------------------------------------------------------------
// ResponseItem variants
// ---------------------------------------------------------------------------

#[test]
fn response_item_user_text_message() {
    let item = parse_item(json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "Hello"}],
    }));
    let ResponseItem::Message { role, content, .. } = item else {
        panic!("wrong variant");
    };
    assert_eq!(role, "user");
    assert_eq!(content.len(), 1);
    match &content[0] {
        ContentItem::InputText { text } => assert_eq!(text, "Hello"),
        other => panic!("expected InputText, got {other:?}"),
    }
}

#[test]
fn response_item_assistant_message_with_output_text() {
    let item = parse_item(json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": "Hi back."}],
    }));
    let ResponseItem::Message { role, content, .. } = item else {
        panic!("wrong variant");
    };
    assert_eq!(role, "assistant");
    match &content[0] {
        ContentItem::OutputText { text } => assert_eq!(text, "Hi back."),
        other => panic!("expected OutputText, got {other:?}"),
    }
}

#[test]
fn response_item_message_with_image_input() {
    let item = parse_item(json!({
        "type": "message",
        "role": "user",
        "content": [
            {"type": "input_text", "text": "describe this:"},
            {"type": "input_image", "image_url": "https://example.com/x.png"},
        ],
    }));
    let ResponseItem::Message { content, .. } = item else {
        panic!("wrong variant");
    };
    assert_eq!(content.len(), 2);
    match &content[1] {
        ContentItem::InputImage { image_url, .. } => {
            assert_eq!(image_url, "https://example.com/x.png");
        }
        other => panic!("expected InputImage, got {other:?}"),
    }
}

#[test]
fn response_item_function_call() {
    let item = parse_item(json!({
        "type": "function_call",
        "name": "shell",
        "arguments": "{\"cmd\":\"ls\"}",
        "call_id": "call_1",
    }));
    let ResponseItem::FunctionCall {
        call_id,
        name,
        arguments,
        ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(call_id, "call_1");
    assert_eq!(name, "shell");
    assert_eq!(arguments, "{\"cmd\":\"ls\"}");
}

// `function_call_output.output` is documented as
// "string or an list of output content" in the OpenAI Responses API
// reference (<https://developers.openai.com/api/docs/api-reference/responses/object>),
// and Codex's serializer emits exactly those two shapes (bare string
// or array of input_text/input_image items — see
// `codex-rs/protocol/src/models.rs:1459-1469`). The legacy
// `{content, success}` object never appears on the wire; `success` is
// internal metadata that is intentionally not serialized.

#[test]
fn response_item_function_call_output_string_payload() {
    let item = parse_item(json!({
        "type": "function_call_output",
        "call_id": "call_1",
        "output": "exit 0\nfile1\nfile2",
    }));
    let ResponseItem::FunctionCallOutput { call_id, output } = item else {
        panic!("wrong variant");
    };
    assert_eq!(call_id, "call_1");
    assert_eq!(output, json!("exit 0\nfile1\nfile2"));
}

#[test]
fn response_item_function_call_output_content_items_payload() {
    let item = parse_item(json!({
        "type": "function_call_output",
        "call_id": "call_2",
        "output": [
            {"type": "input_text", "text": "log line"},
            {"type": "input_image", "image_url": "https://example.com/x.png"},
        ],
    }));
    let ResponseItem::FunctionCallOutput { call_id, output } = item else {
        panic!("wrong variant");
    };
    assert_eq!(call_id, "call_2");
    let arr = output.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], json!("input_text"));
    assert_eq!(arr[0]["text"], json!("log line"));
    assert_eq!(arr[1]["type"], json!("input_image"));
    assert_eq!(arr[1]["image_url"], json!("https://example.com/x.png"));
}

#[test]
fn response_item_reasoning_with_encrypted_content() {
    // Codex re-sends reasoning items with encrypted_content on each
    // turn (its include[] requests this); the translator must be
    // able to read that field and round-trip it as the Anthropic
    // thinking-block signature.
    let item = parse_item(json!({
        "type": "reasoning",
        "id": "rs_1",
        "summary": [{"type": "summary_text", "text": "Checking the file..."}],
        "encrypted_content": "EqQB...",
    }));
    let ResponseItem::Reasoning {
        summary,
        encrypted_content,
        ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(summary.len(), 1);
    match &summary[0] {
        ReasoningSummaryItem::SummaryText { text } => {
            assert_eq!(text, "Checking the file...");
        }
    }
    assert_eq!(encrypted_content.as_deref(), Some("EqQB..."));
}

#[test]
fn response_item_reasoning_with_only_summary() {
    let item = parse_item(json!({
        "type": "reasoning",
        "id": "rs_2",
        "summary": [
            {"type": "summary_text", "text": "First step"},
            {"type": "summary_text", "text": "Second step"},
        ],
    }));
    let ResponseItem::Reasoning {
        summary,
        encrypted_content,
        ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(summary.len(), 2);
    assert!(encrypted_content.is_none());
}

#[test]
fn response_item_custom_tool_call() {
    // Codex's apply_patch / exec_command are emitted as custom tool
    // calls with raw string `input` (Lark-grammar output) rather than
    // structured JSON.
    let item = parse_item(json!({
        "type": "custom_tool_call",
        "call_id": "call_3",
        "name": "apply_patch",
        "input": "*** Begin Patch\n*** End Patch\n",
    }));
    let ResponseItem::CustomToolCall {
        call_id,
        name,
        input,
        ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(call_id, "call_3");
    assert_eq!(name, "apply_patch");
    assert_eq!(input, "*** Begin Patch\n*** End Patch\n");
}

#[test]
fn response_item_custom_tool_call_output() {
    // Custom tool call outputs share `FunctionCallOutputPayload`'s
    // serializer (string or content-items array — never `{content,
    // success}`). Apply_patch in particular always emits a bare
    // string here.
    let item = parse_item(json!({
        "type": "custom_tool_call_output",
        "call_id": "call_3",
        "output": "patch applied",
    }));
    let ResponseItem::CustomToolCallOutput {
        call_id, output, ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(call_id, "call_3");
    assert_eq!(output, json!("patch applied"));
}

#[test]
fn response_item_local_shell_call() {
    let item = parse_item(json!({
        "type": "local_shell_call",
        "call_id": "call_4",
        "status": "completed",
        "action": {"type": "exec", "command": ["ls"]},
    }));
    let ResponseItem::LocalShellCall {
        call_id, action, ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(call_id.as_deref(), Some("call_4"));
    assert_eq!(action["command"][0], json!("ls"));
}

#[test]
fn response_item_unknown_kind_uses_unrecognized_variant() {
    // Forward-compat: unknown items (web_search_call, image_generation_call,
    // future variants) deserialize as Unrecognized. The translator
    // drops them when building the Anthropic messages array.
    let item = parse_item(json!({
        "type": "image_generation_call",
        "id": "ig_1",
        "status": "completed",
    }));
    assert!(matches!(item, ResponseItem::Unrecognized));
}

#[test]
fn full_request_with_mixed_items_round_trips() {
    let req = parse_request(json!({
        "model": "claude-opus-4-7",
        "instructions": "You are Codex.",
        "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "List files."}]},
            {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "ENC"},
            {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"ls\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "a\nb"},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done."}]},
        ],
        "tools": [{"type": "function", "name": "shell"}],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "reasoning": {"effort": "high", "summary": "auto"},
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": "thread-x",
        "text": {"verbosity": "medium"},
        "client_metadata": {"x-codex-installation-id": "i-1"},
    }));
    assert_eq!(req.input.len(), 5);
    assert_eq!(req.prompt_cache_key.as_deref(), Some("thread-x"));
    assert_eq!(
        req.reasoning.as_ref().and_then(|r| r.effort),
        Some(ReasoningEffort::High),
    );
    assert_eq!(
        req.text.as_ref().and_then(|t| t.verbosity),
        Some(Verbosity::Medium),
    );
}

// ---------------------------------------------------------------------------
// Tier-2 / Tier-4 fields the prior translator silently dropped
//
// `protocol/src/models.rs` and `protocol/src/openai_models.rs` are the
// source of truth for the Codex outbound wire format. These tests pin
// the variants and fields Codex actually serializes today; missing
// any of them previously caused either:
//
//   * Whole-request deserialization failure (no `#[serde(other)]` on
//     the enum), silently aborting the turn before reaching Anthropic
//     — `ReasoningEffort::{None, XHigh}` and
//     `ReasoningContentItem::Text`.
//   * Silent data loss because the field wasn't modelled at all —
//     `FunctionCall.namespace`, `ContentItem::InputImage.detail`,
//     `Message.phase`. Anthropic has no equivalent for any of the
//     three, so the translator drops them at translation time, but
//     the deserializer must accept them without complaint.
// ---------------------------------------------------------------------------

#[test]
fn reasoning_content_text_variant_deserializes() {
    // Per `codex-rs/protocol/src/models.rs::ReasoningItemContent`,
    // Codex serializes `content` items as either
    // `{type: "reasoning_text", text}` (legacy) or
    // `{type: "text", text}` (current). Without the `Text` variant,
    // a Codex turn that includes a "text" content item fails the
    // entire request deserialization.
    let item = parse_item(json!({
        "type": "reasoning",
        "id": "rs_3",
        "summary": [],
        "content": [
            {"type": "reasoning_text", "text": "older shape"},
            {"type": "text", "text": "current shape"},
        ],
        "encrypted_content": "ENC",
    }));
    let ResponseItem::Reasoning {
        content,
        encrypted_content,
        ..
    } = item
    else {
        panic!("wrong variant");
    };
    let content = content.expect("content present");
    assert_eq!(content.len(), 2);
    match &content[0] {
        ReasoningContentItem::ReasoningText { text } => assert_eq!(text, "older shape"),
        other => panic!("expected ReasoningText, got {other:?}"),
    }
    match &content[1] {
        ReasoningContentItem::Text { text } => assert_eq!(text, "current shape"),
        other => panic!("expected Text, got {other:?}"),
    }
    assert_eq!(encrypted_content.as_deref(), Some("ENC"));
}

#[test]
fn function_call_with_namespace_round_trips() {
    // Per `codex-rs/protocol/src/models.rs::ResponseItem::FunctionCall`,
    // the `namespace` field carries tool-routing metadata. Anthropic
    // has no equivalent so the translator drops it on translation,
    // but the inbound deserializer must accept it without dropping
    // the whole tool call.
    let item = parse_item(json!({
        "type": "function_call",
        "name": "exec_command",
        "namespace": "system",
        "arguments": "{\"cmd\":\"ls\"}",
        "call_id": "call_n",
    }));
    let ResponseItem::FunctionCall {
        name,
        namespace,
        call_id,
        ..
    } = item
    else {
        panic!("wrong variant");
    };
    assert_eq!(name, "exec_command");
    assert_eq!(namespace.as_deref(), Some("system"));
    assert_eq!(call_id, "call_n");
}

#[test]
fn function_call_without_namespace_still_parses() {
    // Backwards compat: namespace is optional on the wire.
    let item = parse_item(json!({
        "type": "function_call",
        "name": "shell",
        "arguments": "{}",
        "call_id": "call_x",
    }));
    let ResponseItem::FunctionCall { namespace, .. } = item else {
        panic!("wrong variant");
    };
    assert!(namespace.is_none());
}

#[test]
fn input_image_with_detail_round_trips() {
    // Per `codex-rs/protocol/src/models.rs::ContentItem::InputImage`,
    // Codex includes a `detail` field for image URLs. Anthropic's
    // `URLImageSource` block has no detail concept so we drop it on
    // translation, but the inbound parser must accept the field.
    let item = parse_item(json!({
        "type": "message",
        "role": "user",
        "content": [
            {
                "type": "input_image",
                "image_url": "https://example.com/x.png",
                "detail": "high",
            },
        ],
    }));
    let ResponseItem::Message { content, .. } = item else {
        panic!("wrong variant");
    };
    match &content[0] {
        ContentItem::InputImage { image_url, detail } => {
            assert_eq!(image_url, "https://example.com/x.png");
            assert_eq!(detail.as_deref(), Some("high"));
        }
        other => panic!("expected InputImage, got {other:?}"),
    }
}

#[test]
fn message_with_phase_round_trips() {
    // Per `codex-rs/protocol/src/models.rs::ResponseItem::Message`,
    // assistant messages carry an optional `phase` field
    // (`commentary` or `final_answer`). Anthropic has no equivalent
    // distinction, so the translator drops the field on translation,
    // but the deserializer must accept it.
    let item = parse_item(json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": "Done."}],
        "phase": "final_answer",
    }));
    let ResponseItem::Message { phase, .. } = item else {
        panic!("wrong variant");
    };
    assert_eq!(phase.as_deref(), Some("final_answer"));
}
