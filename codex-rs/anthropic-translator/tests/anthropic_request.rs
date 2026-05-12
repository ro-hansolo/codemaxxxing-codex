//! Wire-format contract tests for the outgoing Anthropic Messages
//! request types.
//!
//! Each test pins one part of the JSON shape the translator must emit
//! against the Anthropic Messages API as documented at the URLs below
//! (verified 2026-05-12). Doc references are inline in the test bodies
//! so every assertion ties back to authoritative source:
//!
//!   * <https://docs.anthropic.com/en/api/messages>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/structured-outputs>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/adaptive-thinking>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/effort>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/define-tools>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/tool-reference>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/fine-grained-tool-streaming>
//!   * <https://docs.anthropic.com/en/docs/about-claude/models/overview>

use codex_anthropic_translator::anthropic::CacheControl;
use codex_anthropic_translator::anthropic::CacheTtl;
use codex_anthropic_translator::anthropic::ContentBlock;
use codex_anthropic_translator::anthropic::Effort;
use codex_anthropic_translator::anthropic::FunctionTool;
use codex_anthropic_translator::anthropic::ImageSource;
use codex_anthropic_translator::anthropic::JsonOutputFormat;
use codex_anthropic_translator::anthropic::Message;
use codex_anthropic_translator::anthropic::MessageRequest;
use codex_anthropic_translator::anthropic::Metadata;
use codex_anthropic_translator::anthropic::OutputConfig;
use codex_anthropic_translator::anthropic::Role;
use codex_anthropic_translator::anthropic::SystemBlock;
use codex_anthropic_translator::anthropic::ThinkingConfig;
use codex_anthropic_translator::anthropic::ThinkingDisplay;
use codex_anthropic_translator::anthropic::Tool;
use codex_anthropic_translator::anthropic::ToolChoice;
use codex_anthropic_translator::anthropic::ToolResultContent;
use codex_anthropic_translator::anthropic::WebSearchTool;
use codex_anthropic_translator::anthropic::WebSearchUserLocation;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn to_value(req: &MessageRequest) -> Value {
    match serde_json::to_value(req) {
        Ok(value) => value,
        Err(err) => panic!("MessageRequest must serialize to JSON: {err}"),
    }
}

// ---------------------------------------------------------------------------
// Top-level request shape
// ---------------------------------------------------------------------------

#[test]
fn minimal_request_omits_optional_fields() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text("Hello")],
        }],
        ..MessageRequest::default()
    };

    assert_eq!(
        to_value(&req),
        json!({
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "Hello"}],
                }
            ],
            "stream": false,
        }),
    );
}

#[test]
fn streaming_request_sets_stream_true() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 64,
        stream: true,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text("hi")],
        }],
        ..MessageRequest::default()
    };

    assert_eq!(to_value(&req)["stream"], json!(true));
}

#[test]
fn role_serializes_lowercase() {
    assert_eq!(serde_json::to_value(Role::User).unwrap(), json!("user"));
    assert_eq!(
        serde_json::to_value(Role::Assistant).unwrap(),
        json!("assistant"),
    );
}

// ---------------------------------------------------------------------------
// System blocks
// ---------------------------------------------------------------------------

#[test]
fn system_blocks_serialize_with_cache_control_when_present() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        system: vec![SystemBlock {
            text: "You are Codex.".into(),
            cache_control: Some(CacheControl::ephemeral()),
        }],
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text("hi")],
        }],
        ..MessageRequest::default()
    };

    assert_eq!(
        to_value(&req)["system"],
        json!([
            {
                "type": "text",
                "text": "You are Codex.",
                "cache_control": {"type": "ephemeral"},
            }
        ]),
    );
}

// ---------------------------------------------------------------------------
// Cache control
// ---------------------------------------------------------------------------

#[test]
fn cache_control_default_emits_ephemeral_only() {
    assert_eq!(
        serde_json::to_value(CacheControl::ephemeral()).unwrap(),
        json!({"type": "ephemeral"}),
    );
}

#[test]
fn cache_control_with_five_minute_ttl_round_trips() {
    assert_eq!(
        serde_json::to_value(&CacheControl {
            ttl: Some(CacheTtl::FiveMinutes),
        })
        .unwrap(),
        json!({"type": "ephemeral", "ttl": "5m"}),
    );
}

#[test]
fn cache_control_with_one_hour_ttl_round_trips() {
    // The 1h TTL is documented in the prompt-caching reference; longer
    // TTLs must come before shorter ones in mixed-TTL requests, but
    // that's the planner's job, not the type's.
    assert_eq!(
        serde_json::to_value(&CacheControl {
            ttl: Some(CacheTtl::OneHour),
        })
        .unwrap(),
        json!({"type": "ephemeral", "ttl": "1h"}),
    );
}

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

#[test]
fn assistant_message_with_thinking_and_tool_use_round_trips() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 4096,
        messages: vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::text("What's the weather?")],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    // Round-tripping a thinking block: thinking text
                    // may be empty when display:"omitted" was set on
                    // Opus 4.7; signature is required and opaque.
                    ContentBlock::Thinking {
                        thinking: String::new(),
                        signature: "sig-abc".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_1".into(),
                        name: "get_weather".into(),
                        input: json!({"location": "Paris"}),
                        cache_control: None,
                    },
                ],
            },
        ],
        ..MessageRequest::default()
    };

    assert_eq!(
        to_value(&req)["messages"][1]["content"],
        json!([
            {"type": "thinking", "thinking": "", "signature": "sig-abc"},
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "get_weather",
                "input": {"location": "Paris"},
            },
        ]),
    );
}

#[test]
fn tool_result_with_string_content_serializes_as_string() {
    // Per handle-tool-calls doc: tool_result content can be a plain
    // string OR an array of blocks. The string form is the simplest.
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: ToolResultContent::Text("OK".into()),
                is_error: false,
                cache_control: None,
            }],
        }],
        ..MessageRequest::default()
    };

    assert_eq!(
        to_value(&req)["messages"][0]["content"][0],
        json!({
            "type": "tool_result",
            "tool_use_id": "toolu_1",
            "content": "OK",
        }),
    );
}

#[test]
fn tool_result_with_blocks_serializes_as_array_and_marks_error() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_2".into(),
                content: ToolResultContent::Blocks(vec![
                    ContentBlock::text("partial output"),
                    ContentBlock::Image {
                        source: ImageSource::Url {
                            url: "https://example.com/x.png".into(),
                        },
                        cache_control: None,
                    },
                ]),
                is_error: true,
                cache_control: Some(CacheControl::ephemeral()),
            }],
        }],
        ..MessageRequest::default()
    };

    assert_eq!(
        to_value(&req)["messages"][0]["content"][0],
        json!({
            "type": "tool_result",
            "tool_use_id": "toolu_2",
            "content": [
                {"type": "text", "text": "partial output"},
                {"type": "image", "source": {"type": "url", "url": "https://example.com/x.png"}},
            ],
            "is_error": true,
            "cache_control": {"type": "ephemeral"},
        }),
    );
}

#[test]
fn image_block_with_base64_source_serializes_correctly() {
    let block = ContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
        },
        cache_control: None,
    };
    assert_eq!(
        serde_json::to_value(&block).unwrap(),
        json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": "iVBORw0KGgo=",
            },
        }),
    );
}

#[test]
fn redacted_thinking_round_trips_with_data_only() {
    let block = ContentBlock::RedactedThinking {
        data: "ENC...".into(),
    };
    assert_eq!(
        serde_json::to_value(&block).unwrap(),
        json!({"type": "redacted_thinking", "data": "ENC..."}),
    );
}

// ---------------------------------------------------------------------------
// User-defined function tools
// ---------------------------------------------------------------------------

#[test]
fn function_tool_minimal_emits_required_fields_only() {
    // Per define-tools doc: name (regex `^[a-zA-Z0-9_-]{1,64}$`),
    // description, input_schema. Everything else is optional.
    let tool = Tool::Function(FunctionTool {
        name: "shell".into(),
        description: "Run a shell command.".into(),
        input_schema: json!({"type": "object"}),
        ..FunctionTool::default()
    });
    assert_eq!(
        serde_json::to_value(&tool).unwrap(),
        json!({
            "name": "shell",
            "description": "Run a shell command.",
            "input_schema": {"type": "object"},
        }),
    );
}

#[test]
fn function_tool_with_cache_control_serializes_breakpoint() {
    let tool = Tool::Function(FunctionTool {
        name: "shell".into(),
        description: "Run a shell command.".into(),
        input_schema: json!({"type": "object"}),
        cache_control: Some(CacheControl::ephemeral()),
        ..FunctionTool::default()
    });
    assert_eq!(
        serde_json::to_value(&tool).unwrap()["cache_control"],
        json!({"type": "ephemeral"}),
    );
}

#[test]
fn function_tool_with_strict_true_emits_strict_field() {
    // Per the structured-outputs doc: `strict: true` guarantees the
    // tool input matches the schema (constrained decoding).
    let tool = Tool::Function(FunctionTool {
        name: "exec_command".into(),
        description: "Run an exec command.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {"cmd": {"type": "string"}},
            "required": ["cmd"],
            "additionalProperties": false,
        }),
        strict: true,
        ..FunctionTool::default()
    });
    assert_eq!(serde_json::to_value(&tool).unwrap()["strict"], json!(true),);
}

#[test]
fn function_tool_with_eager_input_streaming_emits_field() {
    // Per the fine-grained-tool-streaming doc: only user-defined tools
    // accept `eager_input_streaming: true`. We use this for apply_patch
    // so the freeform body streams to Codex as raw deltas.
    let tool = Tool::Function(FunctionTool {
        name: "apply_patch".into(),
        description: "Apply a unified diff.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {"raw": {"type": "string"}},
            "required": ["raw"],
        }),
        eager_input_streaming: true,
        ..FunctionTool::default()
    });
    assert_eq!(
        serde_json::to_value(&tool).unwrap()["eager_input_streaming"],
        json!(true),
    );
}

#[test]
fn function_tool_omits_strict_and_eager_streaming_when_default() {
    // Both flags must be omitted when false so we don't pollute the
    // tools system prompt with no-op fields.
    let tool = Tool::Function(FunctionTool {
        name: "shell".into(),
        description: "Run a shell command.".into(),
        input_schema: json!({"type": "object"}),
        ..FunctionTool::default()
    });
    let value = serde_json::to_value(&tool).unwrap();
    assert!(value.get("strict").is_none(), "got {value:?}");
    assert!(
        value.get("eager_input_streaming").is_none(),
        "got {value:?}"
    );
    assert!(value.get("cache_control").is_none(), "got {value:?}");
}

// ---------------------------------------------------------------------------
// Server tools (web search)
// ---------------------------------------------------------------------------

#[test]
fn web_search_hosted_tool_uses_vertex_supported_version_string() {
    // Per <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool>:
    // "On Vertex AI, only the basic web search tool (without dynamic
    // filtering) is available." The newer `web_search_20260209` adds
    // dynamic filtering (which requires `code_execution`, also not on
    // Vertex). Vertex's `/v1/messages` validator rejects 20260209 with
    // an `invalid_request_error` listing `web_search_20250305` as the
    // only accepted web_search variant. The translator targets the
    // Vertex compatibility floor, so we pin 20250305.
    let tool = Tool::WebSearch(WebSearchTool {
        max_uses: Some(5),
        allowed_domains: Some(vec!["docs.anthropic.com".into()]),
        user_location: Some(WebSearchUserLocation {
            country: Some("US".into()),
            region: Some("CA".into()),
            city: Some("San Francisco".into()),
            timezone: Some("America/Los_Angeles".into()),
        }),
        cache_control: None,
    });
    assert_eq!(
        serde_json::to_value(&tool).unwrap(),
        json!({
            "type": "web_search_20250305",
            "name": "web_search",
            "max_uses": 5,
            "allowed_domains": ["docs.anthropic.com"],
            "user_location": {
                "type": "approximate",
                "country": "US",
                "region": "CA",
                "city": "San Francisco",
                "timezone": "America/Los_Angeles",
            },
        }),
    );
}

#[test]
fn web_search_user_location_emits_approximate_type_even_with_no_fields() {
    let tool = Tool::WebSearch(WebSearchTool {
        max_uses: None,
        allowed_domains: None,
        user_location: Some(WebSearchUserLocation::default()),
        cache_control: None,
    });
    assert_eq!(
        serde_json::to_value(&tool).unwrap()["user_location"],
        json!({"type": "approximate"}),
    );
}

#[test]
fn web_search_minimal_emits_type_and_name_only() {
    let tool = Tool::WebSearch(WebSearchTool::default());
    assert_eq!(
        serde_json::to_value(&tool).unwrap(),
        json!({"type": "web_search_20250305", "name": "web_search"}),
    );
}

// ---------------------------------------------------------------------------
// Tool choice
// ---------------------------------------------------------------------------

#[test]
fn tool_choice_serializes_each_variant() {
    // Per define-tools doc, four variants exist. Codex always sends
    // `auto`; the other three are reserved for future use.
    assert_eq!(
        serde_json::to_value(ToolChoice::Auto).unwrap(),
        json!({"type": "auto"}),
    );
    assert_eq!(
        serde_json::to_value(ToolChoice::Any).unwrap(),
        json!({"type": "any"}),
    );
    assert_eq!(
        serde_json::to_value(ToolChoice::Tool {
            name: "respond".into(),
        })
        .unwrap(),
        json!({"type": "tool", "name": "respond"}),
    );
    assert_eq!(
        serde_json::to_value(ToolChoice::None).unwrap(),
        json!({"type": "none"}),
    );
}

// ---------------------------------------------------------------------------
// Thinking config
// ---------------------------------------------------------------------------

#[test]
fn thinking_config_adaptive_takes_only_display() {
    // Effort lives in `output_config`, NOT in `thinking`. Adaptive
    // accepts `display` and rejects extra fields. (adaptive-thinking
    // doc.)
    let cfg = ThinkingConfig::Adaptive {
        display: Some(ThinkingDisplay::Summarized),
    };
    assert_eq!(
        serde_json::to_value(&cfg).unwrap(),
        json!({"type": "adaptive", "display": "summarized"}),
    );
}

#[test]
fn thinking_config_adaptive_without_display_omits_field() {
    let cfg = ThinkingConfig::Adaptive { display: None };
    assert_eq!(
        serde_json::to_value(&cfg).unwrap(),
        json!({"type": "adaptive"}),
    );
}

#[test]
fn thinking_config_enabled_emits_budget_tokens() {
    // Manual mode for older / Haiku models that lack adaptive thinking.
    // **Rejected on Opus 4.7 with HTTP 400** — the translator must
    // never emit this for that model. Type system permits both because
    // we still target older models.
    let cfg = ThinkingConfig::Enabled {
        budget_tokens: 16000,
        display: Some(ThinkingDisplay::Omitted),
    };
    assert_eq!(
        serde_json::to_value(&cfg).unwrap(),
        json!({
            "type": "enabled",
            "budget_tokens": 16000,
            "display": "omitted",
        }),
    );
}

#[test]
fn thinking_display_serializes_lowercase() {
    assert_eq!(
        serde_json::to_value(ThinkingDisplay::Summarized).unwrap(),
        json!("summarized"),
    );
    assert_eq!(
        serde_json::to_value(ThinkingDisplay::Omitted).unwrap(),
        json!("omitted"),
    );
}

// ---------------------------------------------------------------------------
// Output config (effort + structured outputs)
// ---------------------------------------------------------------------------

#[test]
fn output_config_effort_serializes_each_variant() {
    // Effort doc enumerates five levels. Translator is responsible for
    // emitting only model-valid values; the type permits all five.
    //
    //   * xhigh → Opus 4.7 ONLY
    //   * max   → Opus 4.7, Opus 4.6, Sonnet 4.6, Mythos
    //   * low/medium/high → all models that accept effort
    for (effort, wire) in [
        (Effort::Low, "low"),
        (Effort::Medium, "medium"),
        (Effort::High, "high"),
        (Effort::Xhigh, "xhigh"),
        (Effort::Max, "max"),
    ] {
        assert_eq!(
            serde_json::to_value(OutputConfig {
                effort: Some(effort),
                ..OutputConfig::default()
            })
            .unwrap(),
            json!({"effort": wire}),
            "effort variant {effort:?} must serialize as {wire:?}",
        );
    }
}

#[test]
fn output_config_format_json_schema_emits_structured_output_shape() {
    // Per the structured-outputs doc: `output_config.format` with
    // `type: json_schema` and a schema is the GA mechanism for
    // constrained JSON outputs. NOT `text.format` (that's OpenAI
    // Responses), NOT a forced tool-call workaround.
    let cfg = OutputConfig {
        effort: None,
        format: Some(JsonOutputFormat::JsonSchema {
            schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "email": {"type": "string"},
                },
                "required": ["name", "email"],
                "additionalProperties": false,
            }),
        }),
    };
    assert_eq!(
        serde_json::to_value(&cfg).unwrap(),
        json!({
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "email": {"type": "string"},
                    },
                    "required": ["name", "email"],
                    "additionalProperties": false,
                },
            },
        }),
    );
}

#[test]
fn output_config_with_effort_and_format_emits_both_fields() {
    let cfg = OutputConfig {
        effort: Some(Effort::Xhigh),
        format: Some(JsonOutputFormat::JsonSchema {
            schema: json!({"type": "object"}),
        }),
    };
    assert_eq!(
        serde_json::to_value(&cfg).unwrap(),
        json!({
            "effort": "xhigh",
            "format": {"type": "json_schema", "schema": {"type": "object"}},
        }),
    );
}

#[test]
fn output_config_default_serializes_as_empty_object() {
    assert_eq!(
        serde_json::to_value(OutputConfig::default()).unwrap(),
        json!({}),
    );
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[test]
fn metadata_omits_user_id_when_absent() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text("hi")],
        }],
        metadata: Some(Metadata { user_id: None }),
        ..MessageRequest::default()
    };
    assert_eq!(to_value(&req)["metadata"], json!({}));
}

#[test]
fn metadata_includes_user_id_when_present() {
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text("hi")],
        }],
        metadata: Some(Metadata {
            user_id: Some("install-123".into()),
        }),
        ..MessageRequest::default()
    };
    assert_eq!(
        to_value(&req)["metadata"],
        json!({"user_id": "install-123"}),
    );
}

// ---------------------------------------------------------------------------
// End-to-end shape used by the request translator
// ---------------------------------------------------------------------------

#[test]
fn full_opus_4_7_request_has_correct_top_level_shape() {
    // End-to-end shape used by the request translator targeting
    // Opus 4.7:
    //
    //   * max_tokens at the model ceiling (128k per the latest models
    //     overview)
    //   * stream:true (Codex always streams)
    //   * adaptive thinking with display:"summarized" (must be
    //     explicit on Opus 4.7 — the default is "omitted")
    //   * output_config.effort:"xhigh" (recommended starting point for
    //     coding/agentic workloads per the effort doc)
    //   * cache_control on the last system block, last tool, and last
    //     content block of the most recent assistant turn (CachePlan)
    //   * metadata.user_id (Anthropic's only allowed metadata field)
    //   * tool_choice:"auto" (Codex always sends auto; required when
    //     thinking is active)
    //   * apply_patch synthesized as a strict + eager-streaming tool
    let req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 128_000,
        stream: true,
        system: vec![SystemBlock {
            text: "Codex base instructions".into(),
            cache_control: Some(CacheControl::ephemeral()),
        }],
        tools: vec![
            Tool::Function(FunctionTool {
                name: "shell".into(),
                description: "Run shell".into(),
                input_schema: json!({"type": "object"}),
                strict: true,
                ..FunctionTool::default()
            }),
            Tool::Function(FunctionTool {
                name: "apply_patch".into(),
                description: "Apply a unified diff".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"raw": {"type": "string"}},
                    "required": ["raw"],
                    "additionalProperties": false,
                }),
                strict: true,
                eager_input_streaming: true,
                cache_control: Some(CacheControl::ephemeral()),
            }),
        ],
        tool_choice: Some(ToolChoice::Auto),
        thinking: Some(ThinkingConfig::Adaptive {
            display: Some(ThinkingDisplay::Summarized),
        }),
        output_config: Some(OutputConfig {
            effort: Some(Effort::Xhigh),
            format: None,
        }),
        metadata: Some(Metadata {
            user_id: Some("install-abc".into()),
        }),
        messages: vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::text("Refactor X.")],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "Plan: ...".into(),
                    cache_control: Some(CacheControl::ephemeral()),
                }],
            },
        ],
    };

    let value = to_value(&req);
    assert_eq!(value["model"], json!("claude-opus-4-7"));
    assert_eq!(value["max_tokens"], json!(128_000));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(
        value["thinking"],
        json!({"type": "adaptive", "display": "summarized"}),
    );
    assert_eq!(value["output_config"], json!({"effort": "xhigh"}));
    assert_eq!(value["tool_choice"], json!({"type": "auto"}));
    assert_eq!(value["metadata"], json!({"user_id": "install-abc"}));
    assert_eq!(value["tools"][0]["strict"], json!(true));
    assert_eq!(value["tools"][1]["strict"], json!(true));
    assert_eq!(value["tools"][1]["eager_input_streaming"], json!(true));
    assert_eq!(
        value["tools"][1]["cache_control"],
        json!({"type": "ephemeral"}),
    );
    assert_eq!(
        value["messages"][1]["content"][0]["cache_control"],
        json!({"type": "ephemeral"}),
    );
}
