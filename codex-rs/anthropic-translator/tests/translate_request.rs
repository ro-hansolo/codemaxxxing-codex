//! End-to-end request translation: codex `ResponsesRequest` →
//! Anthropic `MessageRequest`.
//!
//! Wires together the per-component translators (`messages`,
//! `tools`, `thinking`, `cache`) plus the per-model rule table.

use codex_anthropic_translator::anthropic::CacheControl;
use codex_anthropic_translator::anthropic::ContentBlock;
use codex_anthropic_translator::anthropic::Effort;
use codex_anthropic_translator::anthropic::JsonOutputFormat;
use codex_anthropic_translator::anthropic::Role;
use codex_anthropic_translator::anthropic::ThinkingConfig;
use codex_anthropic_translator::anthropic::ThinkingDisplay;
use codex_anthropic_translator::openai::ContentItem;
use codex_anthropic_translator::openai::Reasoning;
use codex_anthropic_translator::openai::ReasoningEffort;
use codex_anthropic_translator::openai::ReasoningSummary;
use codex_anthropic_translator::openai::ResponseItem;
use codex_anthropic_translator::openai::ResponsesRequest;
use codex_anthropic_translator::openai::TextControls;
use codex_anthropic_translator::openai::TextFormat;
use codex_anthropic_translator::translate::TranslationError;
use codex_anthropic_translator::translate::translate_request;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;

fn baseline() -> ResponsesRequest {
    ResponsesRequest {
        model: "claude-opus-4-7".into(),
        instructions: "You are Codex.".into(),
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![ContentItem::InputText {
                text: "Refactor X.".into(),
            }],
            phase: None,
        }],
        tools: vec![],
        tool_choice: "auto".into(),
        parallel_tool_calls: true,
        reasoning: Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Auto),
        }),
        store: false,
        stream: true,
        include: vec![],
        service_tier: None,
        prompt_cache_key: Some("thread-1".into()),
        text: None,
        client_metadata: None,
    }
}

// ---------------------------------------------------------------------------
// Top-level shape for Opus 4.7
// ---------------------------------------------------------------------------

#[test]
fn opus_4_7_baseline_emits_correct_top_level_fields() {
    let result = translate_request(baseline()).expect("translation succeeds");
    assert_eq!(result.model, "claude-opus-4-7");
    assert_eq!(result.max_tokens, 128_000, "Opus 4.7 default ceiling");
    assert!(result.stream);
    assert_eq!(
        result.thinking,
        Some(ThinkingConfig::Adaptive {
            display: Some(ThinkingDisplay::Summarized),
        }),
        "Opus 4.7 must use adaptive + display:summarized for visible reasoning",
    );
    let output_config = result.output_config.expect("output_config present");
    assert_eq!(output_config.effort, Some(Effort::Xhigh));
    assert!(output_config.format.is_none());
}

#[test]
fn opus_4_7_baseline_attaches_cache_control_to_system_block() {
    let result = translate_request(baseline()).unwrap();
    assert_eq!(result.system.len(), 1);
    assert_eq!(
        result.system[0].cache_control,
        Some(CacheControl::ephemeral()),
    );
}

// ---------------------------------------------------------------------------
// Empty / minimal cases
// ---------------------------------------------------------------------------

#[test]
fn empty_instructions_produce_no_system_array() {
    let mut req = baseline();
    req.instructions = String::new();
    let result = translate_request(req).unwrap();
    assert!(result.system.is_empty());
}

#[test]
fn empty_tools_produce_no_tools_array_and_no_tool_choice() {
    // tools=[] AND tool_choice=auto from Codex side → translator
    // omits both (Anthropic requires tool_choice only when tools are
    // present).
    let result = translate_request(baseline()).unwrap();
    assert!(result.tools.is_empty());
    assert!(
        result.tool_choice.is_none(),
        "tool_choice omitted when no tools present",
    );
}

#[test]
fn tools_present_emits_anthropic_tool_choice_auto() {
    let mut req = baseline();
    req.tools = vec![json!({
        "type": "function",
        "name": "shell",
        "parameters": {"type": "object"},
    })];
    let result = translate_request(req).unwrap();
    assert_eq!(result.tools.len(), 1);
    assert!(
        result.tool_choice.is_some(),
        "tool_choice present with tools"
    );
}

// ---------------------------------------------------------------------------
// Cache plan integration
// ---------------------------------------------------------------------------

#[test]
fn cache_plan_pins_breakpoints_on_system_tools_and_assistant_turn_tail() {
    let mut req = baseline();
    req.tools = vec![json!({
        "type": "function",
        "name": "shell",
        "parameters": {"type": "object"},
    })];
    // Add a completed assistant turn so the planner places a
    // message-tail breakpoint.
    req.input.push(ResponseItem::Message {
        id: None,
        role: "assistant".into(),
        content: vec![ContentItem::OutputText {
            text: "Done.".into(),
        }],
        phase: None,
    });
    let result = translate_request(req).unwrap();
    assert_eq!(
        result.system[0].cache_control,
        Some(CacheControl::ephemeral()),
    );
    let codex_anthropic_translator::anthropic::Tool::Function(t) = &result.tools[0] else {
        panic!("expected function tool");
    };
    assert_eq!(t.cache_control, Some(CacheControl::ephemeral()));
    let assistant_idx = result
        .messages
        .iter()
        .rposition(|m| matches!(m.role, Role::Assistant))
        .expect("assistant message present");
    let ContentBlock::Text { cache_control, .. } = &result.messages[assistant_idx].content[0]
    else {
        panic!("expected text");
    };
    assert_eq!(*cache_control, Some(CacheControl::ephemeral()));
}

// ---------------------------------------------------------------------------
// Per-model behaviour
// ---------------------------------------------------------------------------

#[test]
fn haiku_4_5_uses_manual_thinking_and_64k_max_with_no_output_config_effort() {
    let mut req = baseline();
    req.model = "claude-haiku-4-5".into();
    let result = translate_request(req).unwrap();
    assert_eq!(result.max_tokens, 64_000);
    assert!(matches!(
        result.thinking,
        Some(ThinkingConfig::Enabled { .. })
    ));
    assert!(
        result
            .output_config
            .as_ref()
            .is_none_or(|cfg| cfg.effort.is_none()),
        "Haiku 4.5 doesn't accept effort",
    );
}

#[test]
fn opus_4_6_uses_adaptive_with_high_effort_no_xhigh_promotion() {
    let mut req = baseline();
    req.model = "claude-opus-4-6".into();
    let result = translate_request(req).unwrap();
    assert_eq!(result.max_tokens, 128_000);
    assert!(matches!(
        result.thinking,
        Some(ThinkingConfig::Adaptive { .. }),
    ));
    let cfg = result.output_config.expect("output_config present");
    assert_eq!(cfg.effort, Some(Effort::High), "no xhigh on 4.6");
}

#[test]
fn unknown_model_returns_translation_error() {
    let mut req = baseline();
    req.model = "gpt-5-codex".into();
    let err = translate_request(req).expect_err("non-Claude model should error");
    assert!(matches!(err, TranslationError::UnsupportedModel(_)));
}

// ---------------------------------------------------------------------------
// Structured output
// ---------------------------------------------------------------------------

#[test]
fn text_format_json_schema_translates_to_output_config_format() {
    let mut req = baseline();
    req.text = Some(TextControls {
        verbosity: None,
        format: Some(TextFormat::JsonSchema {
            schema: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"],
                "additionalProperties": false,
            }),
            strict: true,
            name: "out".into(),
        }),
    });
    let result = translate_request(req).unwrap();
    let cfg = result.output_config.expect("output_config present");
    let format = cfg.format.expect("format present");
    let JsonOutputFormat::JsonSchema { schema } = format;
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn text_verbosity_alone_does_not_emit_format_field() {
    let mut req = baseline();
    req.text = Some(TextControls {
        verbosity: Some(codex_anthropic_translator::openai::Verbosity::High),
        format: None,
    });
    let result = translate_request(req).unwrap();
    let cfg = result.output_config.expect("output_config present");
    assert!(
        cfg.format.is_none(),
        "verbosity dropped — no Anthropic equivalent",
    );
}

// ---------------------------------------------------------------------------
// Metadata + miscellaneous
// ---------------------------------------------------------------------------

#[test]
fn client_metadata_x_codex_installation_id_becomes_anthropic_metadata_user_id() {
    let mut req = baseline();
    let mut metadata = HashMap::new();
    metadata.insert(
        "x-codex-installation-id".to_string(),
        "install-abc".to_string(),
    );
    req.client_metadata = Some(metadata);
    let result = translate_request(req).unwrap();
    let metadata = result.metadata.expect("metadata present");
    assert_eq!(metadata.user_id.as_deref(), Some("install-abc"));
}

#[test]
fn no_client_metadata_omits_anthropic_metadata_field() {
    let result = translate_request(baseline()).unwrap();
    assert!(result.metadata.is_none());
}

#[test]
fn always_emits_stream_true_regardless_of_input_value() {
    // Codex always streams; if a client somehow sends stream:false we
    // still flip it. Anthropic non-streaming requests over agentic
    // workloads risk timeout.
    let mut req = baseline();
    req.stream = false;
    let result = translate_request(req).unwrap();
    assert!(result.stream);
}

#[test]
fn store_service_tier_parallel_tool_calls_include_are_dropped_silently() {
    // None of these have Anthropic equivalents — translation should
    // proceed without error and they should not appear in the output.
    let mut req = baseline();
    req.store = true;
    req.service_tier = Some("priority".into());
    req.parallel_tool_calls = true;
    req.include = vec!["reasoning.encrypted_content".into()];
    let result = translate_request(req).unwrap();
    // No assertion needed beyond "doesn't error" — these fields don't
    // exist on the Anthropic request type at all.
    assert_eq!(result.model, "claude-opus-4-7");
}
