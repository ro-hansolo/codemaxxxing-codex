//! Wire-format contract tests for incoming Anthropic Messages SSE
//! event types.
//!
//! Each test pins one Anthropic event shape against the latest
//! streaming reference. Sample JSON is taken from the official docs:
//!
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/streaming>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>
//!   * <https://docs.anthropic.com/en/api/errors>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/handle-tool-calls>

use codex_anthropic_translator::anthropic::event::ContentBlock;
use codex_anthropic_translator::anthropic::event::ContentBlockDelta;
use codex_anthropic_translator::anthropic::event::ErrorKind;
use codex_anthropic_translator::anthropic::event::StopReason;
use codex_anthropic_translator::anthropic::event::StreamEvent;
use codex_anthropic_translator::anthropic::event::Usage;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn parse(value: Value) -> StreamEvent {
    match serde_json::from_value::<StreamEvent>(value) {
        Ok(event) => event,
        Err(err) => panic!("StreamEvent must deserialize: {err}"),
    }
}

fn parse_usage(value: Value) -> Usage {
    match serde_json::from_value::<Usage>(value) {
        Ok(usage) => usage,
        Err(err) => panic!("Usage must deserialize: {err}"),
    }
}

// ---------------------------------------------------------------------------
// message_start
// ---------------------------------------------------------------------------

#[test]
fn message_start_parses_id_model_and_usage() {
    // Sample taken verbatim from the streaming reference.
    let event = parse(json!({
        "type": "message_start",
        "message": {
            "id": "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-opus-4-7",
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {"input_tokens": 25, "output_tokens": 1},
        },
    }));
    match event {
        StreamEvent::MessageStart { message } => {
            assert_eq!(message.id, "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY");
            assert_eq!(message.model, "claude-opus-4-7");
            assert_eq!(message.usage.input_tokens, 25);
            assert_eq!(message.usage.output_tokens, 1);
            assert_eq!(message.usage.cache_creation_input_tokens, 0);
            assert_eq!(message.usage.cache_read_input_tokens, 0);
        }
        other => panic!("expected MessageStart, got {other:?}"),
    }
}

#[test]
fn message_start_with_cache_usage_round_trips() {
    let event = parse(json!({
        "type": "message_start",
        "message": {
            "id": "msg_2",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-opus-4-7",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 5,
                "cache_creation_input_tokens": 248,
                "cache_read_input_tokens": 1800,
            },
        },
    }));
    let StreamEvent::MessageStart { message } = event else {
        panic!("wrong variant");
    };
    assert_eq!(message.usage.cache_creation_input_tokens, 248);
    assert_eq!(message.usage.cache_read_input_tokens, 1800);
}

// ---------------------------------------------------------------------------
// content_block_start
// ---------------------------------------------------------------------------

#[test]
fn content_block_start_text_block() {
    let event = parse(json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "text", "text": ""},
    }));
    match event {
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            assert_eq!(index, 0);
            match content_block {
                ContentBlock::Text { text } => assert_eq!(text, ""),
                other => panic!("expected Text, got {other:?}"),
            }
        }
        other => panic!("expected ContentBlockStart, got {other:?}"),
    }
}

#[test]
fn content_block_start_thinking_block_carries_empty_text_and_signature() {
    // For Opus 4.7 with display:"omitted", the thinking field stays
    // empty — but the signature is populated via signature_delta later.
    // The initial block_start always shows empty strings.
    let event = parse(json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "thinking", "thinking": "", "signature": ""},
    }));
    let StreamEvent::ContentBlockStart {
        index,
        content_block,
    } = event
    else {
        panic!("wrong variant");
    };
    assert_eq!(index, 0);
    match content_block {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "");
            assert_eq!(signature, "");
        }
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn content_block_start_tool_use_block() {
    let event = parse(json!({
        "type": "content_block_start",
        "index": 1,
        "content_block": {
            "type": "tool_use",
            "id": "toolu_01T1x1fJ34qAmk2tNTrN7Up6",
            "name": "get_weather",
            "input": {},
        },
    }));
    let StreamEvent::ContentBlockStart {
        index,
        content_block,
    } = event
    else {
        panic!("wrong variant");
    };
    assert_eq!(index, 1);
    match content_block {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "toolu_01T1x1fJ34qAmk2tNTrN7Up6");
            assert_eq!(name, "get_weather");
            assert_eq!(input, json!({}));
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

#[test]
fn content_block_start_server_tool_use_for_web_search() {
    // Sample matches the streaming-with-web-search example in the
    // streaming reference.
    let event = parse(json!({
        "type": "content_block_start",
        "index": 1,
        "content_block": {
            "type": "server_tool_use",
            "id": "srvtoolu_014hJH82Qum7Td6UV8gDXThB",
            "name": "web_search",
            "input": {},
        },
    }));
    let StreamEvent::ContentBlockStart { content_block, .. } = event else {
        panic!("wrong variant");
    };
    match content_block {
        ContentBlock::ServerToolUse { id, name, .. } => {
            assert_eq!(id, "srvtoolu_014hJH82Qum7Td6UV8gDXThB");
            assert_eq!(name, "web_search");
        }
        other => panic!("expected ServerToolUse, got {other:?}"),
    }
}

#[test]
fn content_block_start_web_search_tool_result_passes_through_content() {
    // The web search result content is structured but the translator
    // forwards it as opaque JSON (Codex's TUI doesn't render it
    // natively). We just round-trip the Value.
    let event = parse(json!({
        "type": "content_block_start",
        "index": 2,
        "content_block": {
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_014hJH82Qum7Td6UV8gDXThB",
            "content": [
                {
                    "type": "web_search_result",
                    "title": "Weather in NYC",
                    "url": "https://example.com",
                    "encrypted_content": "ENC..."
                }
            ],
        },
    }));
    let StreamEvent::ContentBlockStart { content_block, .. } = event else {
        panic!("wrong variant");
    };
    match content_block {
        ContentBlock::WebSearchToolResult {
            tool_use_id,
            content,
        } => {
            assert_eq!(tool_use_id, "srvtoolu_014hJH82Qum7Td6UV8gDXThB");
            assert_eq!(content[0]["title"], json!("Weather in NYC"));
        }
        other => panic!("expected WebSearchToolResult, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// content_block_delta
// ---------------------------------------------------------------------------

#[test]
fn content_block_delta_text_delta() {
    let event = parse(json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "text_delta", "text": "Hello"},
    }));
    let StreamEvent::ContentBlockDelta { index, delta } = event else {
        panic!("wrong variant");
    };
    assert_eq!(index, 0);
    match delta {
        ContentBlockDelta::TextDelta { text } => assert_eq!(text, "Hello"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

#[test]
fn content_block_delta_input_json_delta_carries_partial_json() {
    // Tool input streams as partial JSON fragments. The accumulator
    // contract: caller concatenates partial_json across deltas, parses
    // once on content_block_stop.
    let event = parse(json!({
        "type": "content_block_delta",
        "index": 1,
        "delta": {"type": "input_json_delta", "partial_json": "{\"location\":"},
    }));
    let StreamEvent::ContentBlockDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    match delta {
        ContentBlockDelta::InputJsonDelta { partial_json } => {
            assert_eq!(partial_json, "{\"location\":");
        }
        other => panic!("expected InputJsonDelta, got {other:?}"),
    }
}

#[test]
fn content_block_delta_thinking_delta() {
    let event = parse(json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "thinking_delta", "thinking": "Let me think..."},
    }));
    let StreamEvent::ContentBlockDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    match delta {
        ContentBlockDelta::ThinkingDelta { thinking } => {
            assert_eq!(thinking, "Let me think...");
        }
        other => panic!("expected ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn content_block_delta_signature_delta_carries_signature() {
    // Signature delta arrives just before content_block_stop on a
    // thinking block. We MUST round-trip it back to Anthropic on the
    // next turn (signature is required when thinking + tool_use).
    let event = parse(json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "signature_delta", "signature": "EqQBCgIYAhIM..."},
    }));
    let StreamEvent::ContentBlockDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    match delta {
        ContentBlockDelta::SignatureDelta { signature } => {
            assert_eq!(signature, "EqQBCgIYAhIM...");
        }
        other => panic!("expected SignatureDelta, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// content_block_stop
// ---------------------------------------------------------------------------

#[test]
fn content_block_stop_carries_index() {
    let event = parse(json!({"type": "content_block_stop", "index": 0}));
    match event {
        StreamEvent::ContentBlockStop { index } => assert_eq!(index, 0),
        other => panic!("expected ContentBlockStop, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// message_delta
// ---------------------------------------------------------------------------

#[test]
fn message_delta_with_end_turn_and_usage() {
    // Per the streaming docs, message_delta usage is cumulative output
    // tokens and may include cache fields.
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn", "stop_sequence": null},
        "usage": {"output_tokens": 15},
    }));
    let StreamEvent::MessageDelta { delta, usage } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(delta.stop_sequence, None);
    let usage = usage.expect("message_delta with `usage` field present");
    assert_eq!(usage.output_tokens, 15);
}

#[test]
fn message_delta_with_tool_use_stop_reason() {
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "tool_use", "stop_sequence": null},
        "usage": {"output_tokens": 89},
    }));
    let StreamEvent::MessageDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::ToolUse));
}

#[test]
fn message_delta_with_max_tokens_stop_reason() {
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "max_tokens"},
        "usage": {"output_tokens": 1024},
    }));
    let StreamEvent::MessageDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::MaxTokens));
}

#[test]
fn message_delta_with_stop_sequence_stop_reason() {
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "stop_sequence", "stop_sequence": "###"},
        "usage": {"output_tokens": 50},
    }));
    let StreamEvent::MessageDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::StopSequence));
    assert_eq!(delta.stop_sequence.as_deref(), Some("###"));
}

#[test]
fn message_delta_with_pause_turn_stop_reason() {
    // pause_turn appears in long-running agentic flows.
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "pause_turn"},
        "usage": {"output_tokens": 0},
    }));
    let StreamEvent::MessageDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::PauseTurn));
}

#[test]
fn message_delta_with_unknown_stop_reason_uses_unknown_variant() {
    // Forward-compat: any new stop_reason Anthropic adds must not
    // break us — we route unknowns through the catch-all variant.
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "some_future_reason"},
        "usage": {"output_tokens": 0},
    }));
    let StreamEvent::MessageDelta { delta, .. } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::Unknown));
}

// ---------------------------------------------------------------------------
// message_stop, ping
// ---------------------------------------------------------------------------

#[test]
fn message_stop_event() {
    let event = parse(json!({"type": "message_stop"}));
    assert!(matches!(event, StreamEvent::MessageStop));
}

#[test]
fn ping_event() {
    let event = parse(json!({"type": "ping"}));
    assert!(matches!(event, StreamEvent::Ping));
}

// ---------------------------------------------------------------------------
// error
// ---------------------------------------------------------------------------

#[test]
fn error_event_with_overloaded_error() {
    let event = parse(json!({
        "type": "error",
        "error": {"type": "overloaded_error", "message": "Overloaded"},
    }));
    let StreamEvent::Error { error } = event else {
        panic!("wrong variant");
    };
    assert_eq!(error.kind, ErrorKind::OverloadedError);
    assert_eq!(error.message, "Overloaded");
}

#[test]
fn error_event_with_rate_limit_error() {
    let event = parse(json!({
        "type": "error",
        "error": {"type": "rate_limit_error", "message": "Slow down"},
    }));
    let StreamEvent::Error { error } = event else {
        panic!("wrong variant");
    };
    assert_eq!(error.kind, ErrorKind::RateLimitError);
}

#[test]
fn error_event_with_invalid_request_error() {
    let event = parse(json!({
        "type": "error",
        "error": {"type": "invalid_request_error", "message": "bad"},
    }));
    let StreamEvent::Error { error } = event else {
        panic!("wrong variant");
    };
    assert_eq!(error.kind, ErrorKind::InvalidRequestError);
}

#[test]
fn error_event_with_unknown_kind_uses_unknown_variant() {
    let event = parse(json!({
        "type": "error",
        "error": {"type": "some_future_error", "message": "..."},
    }));
    let StreamEvent::Error { error } = event else {
        panic!("wrong variant");
    };
    assert_eq!(error.kind, ErrorKind::Unknown);
}

#[test]
fn every_documented_error_kind_round_trips() {
    // Pin the full set of documented HTTP error types from the errors
    // reference. New types added to Anthropic land in `Unknown` until
    // explicitly added here.
    for (wire, expected) in [
        ("invalid_request_error", ErrorKind::InvalidRequestError),
        ("authentication_error", ErrorKind::AuthenticationError),
        ("billing_error", ErrorKind::BillingError),
        ("permission_error", ErrorKind::PermissionError),
        ("not_found_error", ErrorKind::NotFoundError),
        ("request_too_large", ErrorKind::RequestTooLarge),
        ("rate_limit_error", ErrorKind::RateLimitError),
        ("api_error", ErrorKind::ApiError),
        ("timeout_error", ErrorKind::TimeoutError),
        ("overloaded_error", ErrorKind::OverloadedError),
    ] {
        let event = parse(json!({
            "type": "error",
            "error": {"type": wire, "message": "msg"},
        }));
        let StreamEvent::Error { error } = event else {
            panic!("wrong variant");
        };
        assert_eq!(error.kind, expected, "for wire kind {wire:?}");
    }
}

// ---------------------------------------------------------------------------
// Usage with mixed-TTL cache breakdown
// ---------------------------------------------------------------------------

#[test]
fn usage_with_mixed_ttl_cache_creation_breakdown() {
    // From the prompt-caching docs (1-hour cache duration section):
    // when mixing 5m + 1h breakpoints, usage gets a nested
    // `cache_creation` map with per-TTL token counts.
    let usage = parse_usage(json!({
        "input_tokens": 2048,
        "cache_read_input_tokens": 1800,
        "cache_creation_input_tokens": 248,
        "output_tokens": 503,
        "cache_creation": {
            "ephemeral_5m_input_tokens": 148,
            "ephemeral_1h_input_tokens": 100,
        },
    }));
    assert_eq!(usage.cache_read_input_tokens, 1800);
    assert_eq!(usage.cache_creation_input_tokens, 248);
    let breakdown = usage
        .cache_creation
        .expect("cache_creation should be present");
    assert_eq!(breakdown.ephemeral_5m_input_tokens, 148);
    assert_eq!(breakdown.ephemeral_1h_input_tokens, 100);
}

#[test]
fn usage_without_cache_creation_breakdown_omits_field() {
    let usage = parse_usage(json!({
        "input_tokens": 100,
        "output_tokens": 50,
    }));
    assert!(usage.cache_creation.is_none());
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
}

// ---------------------------------------------------------------------------
// Forward-compat / less-common variants the previous translator
// silently dropped (or failed to deserialize, which would silently
// abort the entire stream).
// ---------------------------------------------------------------------------

/// Per the [web search citations
/// docs](https://docs.anthropic.com/en/docs/build-with-claude/tool-use/web-search-tool#streaming),
/// citations on text blocks arrive as `citation_delta` deltas. The
/// strict serde tag on `ContentBlockDelta` previously had no arm for
/// `citation_delta`, so the entire `StreamEvent` failed to
/// deserialize, the stream layer logged a warning and dropped the
/// event, and citations silently never reached Codex.
#[test]
fn content_block_delta_citation_delta_deserializes() {
    let event = parse(json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {
            "type": "citation_delta",
            "citation": {
                "type": "web_search_result_location",
                "url": "https://example.com/article",
                "title": "Example Article",
                "encrypted_index": "EncIdx",
                "cited_text": "Some quoted text",
            },
        },
    }));
    let StreamEvent::ContentBlockDelta { index, delta } = event else {
        panic!("wrong variant");
    };
    assert_eq!(index, 0);
    let ContentBlockDelta::CitationDelta { citation } = delta else {
        panic!("expected citation_delta variant");
    };
    assert_eq!(citation["url"], json!("https://example.com/article"));
    assert_eq!(citation["title"], json!("Example Article"));
}

/// Per the [extended-thinking
/// docs](https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking),
/// the model emits `redacted_thinking` content blocks when its
/// reasoning is safety-filtered. The block carries an opaque `data`
/// field that must be round-tripped (signature-equivalent) to
/// preserve thinking-block validation on subsequent turns.
#[test]
fn content_block_redacted_thinking_deserializes() {
    let event = parse(json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {
            "type": "redacted_thinking",
            "data": "EncryptedRedactedPayload==",
        },
    }));
    let StreamEvent::ContentBlockStart {
        index,
        content_block,
    } = event
    else {
        panic!("wrong variant");
    };
    assert_eq!(index, 0);
    let ContentBlock::RedactedThinking { data } = content_block else {
        panic!("expected redacted_thinking content block");
    };
    assert_eq!(data, "EncryptedRedactedPayload==");
}

/// Per the [streaming
/// reference](https://docs.anthropic.com/en/docs/build-with-claude/streaming),
/// the canonical `message_delta` shape always includes a `usage`
/// field, but Anthropic's versioning policy explicitly allows new
/// shapes (and SDK error-recovery paths sometimes synthesize
/// `message_delta` events without one). With `usage: Usage` non-
/// optional, any such event would crash the `StreamEvent`
/// deserialization and abort the stream. Tolerate the omission with
/// a default-zero `Usage`.
#[test]
fn message_delta_without_usage_field_deserializes() {
    let event = parse(json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn", "stop_sequence": null},
    }));
    let StreamEvent::MessageDelta { delta, usage } = event else {
        panic!("wrong variant");
    };
    assert_eq!(delta.stop_reason, Some(StopReason::EndTurn));
    // Default-zero usage when omitted; the stream translator merges
    // this with any usage already captured from `message_start`.
    assert!(usage.is_none(), "missing `usage` must deserialize as None");
}
