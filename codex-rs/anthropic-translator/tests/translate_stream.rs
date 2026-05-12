//! Stream translator: Anthropic SSE events → Codex `ResponseStreamEvent`s.
//!
//! Each test feeds a sequence of Anthropic events and asserts the
//! Codex events that come out. The translator is stateful (signature
//! buffering, tool-input accumulation, output-index tracking) so
//! tests run a sequence rather than translating events in isolation.

use codex_anthropic_translator::anthropic::Role;
use codex_anthropic_translator::anthropic::event::ContentBlock as InContent;
use codex_anthropic_translator::anthropic::event::ContentBlockDelta;
use codex_anthropic_translator::anthropic::event::ErrorKind;
use codex_anthropic_translator::anthropic::event::ErrorPayload;
use codex_anthropic_translator::anthropic::event::MessageDelta;
use codex_anthropic_translator::anthropic::event::MessageStart;
use codex_anthropic_translator::anthropic::event::StopReason;
use codex_anthropic_translator::anthropic::event::StreamEvent;
use codex_anthropic_translator::anthropic::event::Usage;
use codex_anthropic_translator::openai::OutputItem;
use codex_anthropic_translator::openai::ResponseStreamEvent;
use codex_anthropic_translator::translate::StreamTranslator;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashSet;

fn translator_with(custom_tools: &[&str]) -> StreamTranslator {
    let names: HashSet<String> = custom_tools.iter().map(|s| (*s).to_string()).collect();
    StreamTranslator::new(names)
}

fn run(translator: &mut StreamTranslator, events: Vec<StreamEvent>) -> Vec<ResponseStreamEvent> {
    let mut out = Vec::new();
    for event in events {
        out.extend(translator.consume(event));
    }
    out
}

fn message_start(model: &str, id: &str) -> StreamEvent {
    StreamEvent::MessageStart {
        message: MessageStart {
            id: id.into(),
            model: model.into(),
            role: Role::Assistant,
            usage: Usage::default(),
        },
    }
}

fn block_start_text(index: u32) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: InContent::Text {
            text: String::new(),
        },
    }
}

fn block_start_thinking(index: u32) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: InContent::Thinking {
            thinking: String::new(),
            signature: String::new(),
        },
    }
}

fn block_start_tool_use(index: u32, id: &str, name: &str) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: InContent::ToolUse {
            id: id.into(),
            name: name.into(),
            input: json!({}),
        },
    }
}

fn block_delta_text(index: u32, text: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentBlockDelta::TextDelta { text: text.into() },
    }
}

fn block_delta_thinking(index: u32, text: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentBlockDelta::ThinkingDelta {
            thinking: text.into(),
        },
    }
}

fn block_delta_signature(index: u32, sig: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentBlockDelta::SignatureDelta {
            signature: sig.into(),
        },
    }
}

fn block_delta_input_json(index: u32, partial: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentBlockDelta::InputJsonDelta {
            partial_json: partial.into(),
        },
    }
}

fn block_stop(index: u32) -> StreamEvent {
    StreamEvent::ContentBlockStop { index }
}

fn message_delta(stop_reason: StopReason) -> StreamEvent {
    StreamEvent::MessageDelta {
        delta: MessageDelta {
            stop_reason: Some(stop_reason),
            stop_sequence: None,
        },
        usage: Some(Usage {
            output_tokens: 10,
            ..Usage::default()
        }),
    }
}

// ---------------------------------------------------------------------------
// Text-only turn
// ---------------------------------------------------------------------------

#[test]
fn text_only_turn_emits_created_added_deltas_done_completed() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        block_start_text(0),
        block_delta_text(0, "Hello"),
        block_delta_text(0, " world"),
        block_stop(0),
        message_delta(StopReason::EndTurn),
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);

    // Expected sequence: created, output_item.added (msg), 2x text.delta,
    // output_item.done (msg), completed (with end_turn=true).
    assert!(matches!(&out[0], ResponseStreamEvent::Created { .. }));
    assert!(matches!(
        &out[1],
        ResponseStreamEvent::OutputItemAdded {
            item: OutputItem::AssistantMessage { .. },
            ..
        }
    ));
    assert!(matches!(
        &out[2],
        ResponseStreamEvent::OutputTextDelta { delta, .. } if delta == "Hello"
    ));
    assert!(matches!(
        &out[3],
        ResponseStreamEvent::OutputTextDelta { delta, .. } if delta == " world"
    ));
    assert!(matches!(
        &out[4],
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::AssistantMessage { text, .. },
            ..
        } if text == "Hello world"
    ));
    let ResponseStreamEvent::Completed { response } = &out[5] else {
        panic!("expected Completed");
    };
    assert_eq!(response.end_turn, Some(true));
}

// ---------------------------------------------------------------------------
// Thinking block (Opus 4.7 with display:summarized)
// ---------------------------------------------------------------------------

#[test]
fn thinking_block_emits_reasoning_summary_part_added_then_summary_text_deltas() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        block_start_thinking(0),
        block_delta_thinking(0, "Step one..."),
        block_delta_thinking(0, " step two."),
        block_delta_signature(0, "SIG_OPAQUE"),
        block_stop(0),
        message_delta(StopReason::EndTurn),
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);

    // After Created we expect:
    //   output_item.added (Reasoning),
    //   reasoning_summary_part.added (summary_index=0),
    //   reasoning_summary_text.delta x 2,
    //   output_item.done (Reasoning with encrypted_content=signature),
    //   completed.
    assert!(matches!(&out[0], ResponseStreamEvent::Created { .. }));
    assert!(matches!(
        &out[1],
        ResponseStreamEvent::OutputItemAdded {
            item: OutputItem::Reasoning { .. },
            ..
        }
    ));
    assert!(matches!(
        &out[2],
        ResponseStreamEvent::ReasoningSummaryPartAdded {
            summary_index: 0,
            ..
        }
    ));
    assert!(matches!(
        &out[3],
        ResponseStreamEvent::ReasoningSummaryTextDelta { delta, .. } if delta == "Step one..."
    ));
    assert!(matches!(
        &out[4],
        ResponseStreamEvent::ReasoningSummaryTextDelta { delta, .. } if delta == " step two."
    ));
    let ResponseStreamEvent::OutputItemDone {
        item: OutputItem::Reasoning {
            encrypted_content, ..
        },
        ..
    } = &out[5]
    else {
        panic!("expected Reasoning OutputItemDone");
    };
    assert_eq!(encrypted_content.as_deref(), Some("SIG_OPAQUE"));
}

// ---------------------------------------------------------------------------
// Function call (regular tool, not custom)
// ---------------------------------------------------------------------------

#[test]
fn function_tool_use_accumulates_input_json_and_emits_function_call_done() {
    let mut translator = translator_with(&[]); // no custom tools
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        block_start_tool_use(1, "toolu_1", "shell"),
        block_delta_input_json(1, "{\"cmd\":"),
        block_delta_input_json(1, "\"ls\"}"),
        block_stop(1),
        message_delta(StopReason::ToolUse),
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);

    let function_call_done = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::FunctionCall {
                call_id, arguments, ..
            },
            ..
        } if call_id == "toolu_1" => Some(arguments.clone()),
        _ => None,
    });
    assert_eq!(
        function_call_done.as_deref(),
        Some("{\"cmd\":\"ls\"}"),
        "function_call done must include accumulated arguments string",
    );

    // Completed must mark end_turn=false (tool_use → more turns coming).
    let completed = out.iter().find_map(|event| match event {
        ResponseStreamEvent::Completed { response } => Some(response.end_turn),
        _ => None,
    });
    assert_eq!(completed, Some(Some(false)));
}

// ---------------------------------------------------------------------------
// Custom tool (apply_patch) — eager-streamed deltas
// ---------------------------------------------------------------------------

#[test]
fn custom_tool_use_emits_custom_tool_call_input_delta_with_raw_payload() {
    // Translator was configured with apply_patch as a custom tool, so
    // when Claude calls it, the {"raw": "..."} accumulation must be
    // unwrapped and streamed as raw deltas to Codex.
    let mut translator = translator_with(&["apply_patch"]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        block_start_tool_use(1, "toolu_1", "apply_patch"),
        // Anthropic streams partial JSON for {"raw": "..."}
        block_delta_input_json(1, "{\"raw\":"),
        block_delta_input_json(1, "\"*** Begin Patch\\n*** End Patch\\n\"}"),
        block_stop(1),
        message_delta(StopReason::ToolUse),
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);

    // Expected: output_item.added (CustomToolCall), at least one
    // custom_tool_call_input.delta containing raw text, output_item.done.
    let added = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemAdded {
            item: OutputItem::CustomToolCall { name, .. },
            ..
        } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(added.as_deref(), Some("apply_patch"));

    let done = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::CustomToolCall { input, .. },
            ..
        } => Some(input.clone()),
        _ => None,
    });
    assert_eq!(done.as_deref(), Some("*** Begin Patch\n*** End Patch\n"));

    // Concatenating every CustomToolCallInputDelta must reconstruct the
    // full raw payload.
    let streamed: String = out
        .iter()
        .filter_map(|event| match event {
            ResponseStreamEvent::CustomToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(streamed, "*** Begin Patch\n*** End Patch\n");
}

#[test]
fn custom_tool_use_streams_raw_payload_incrementally_across_many_chunks() {
    // Anthropic frequently splits the raw string across many tiny
    // input_json_delta chunks, including mid-escape-sequence. The
    // extractor must surface raw bytes as they arrive — never buffer
    // the entire JSON payload before emitting.
    let mut translator = translator_with(&["apply_patch"]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        block_start_tool_use(1, "toolu_1", "apply_patch"),
        block_delta_input_json(1, "{\"r"),
        block_delta_input_json(1, "aw\":\"hello"),
        block_delta_input_json(1, ", "),
        block_delta_input_json(1, "world"),
        block_delta_input_json(1, "!\\n"),
        block_delta_input_json(1, "second line"),
        block_delta_input_json(1, "\"}"),
        block_stop(1),
        message_delta(StopReason::ToolUse),
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);

    // We expect MORE than one CustomToolCallInputDelta — the whole
    // point is incremental streaming.
    let delta_count = out
        .iter()
        .filter(|event| matches!(event, ResponseStreamEvent::CustomToolCallInputDelta { .. }))
        .count();
    assert!(
        delta_count >= 2,
        "expected incremental deltas, got {delta_count}",
    );

    // Concatenated deltas must reconstruct the full payload, with
    // JSON escapes (\n) decoded.
    let streamed: String = out
        .iter()
        .filter_map(|event| match event {
            ResponseStreamEvent::CustomToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(streamed, "hello, world!\nsecond line");

    // Final OutputItemDone input matches.
    let done = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::CustomToolCall { input, .. },
            ..
        } => Some(input.clone()),
        _ => None,
    });
    assert_eq!(done.as_deref(), Some("hello, world!\nsecond line"));
}

// ---------------------------------------------------------------------------
// Error event during stream
// ---------------------------------------------------------------------------

#[test]
fn anthropic_error_event_emits_response_failed_with_error_payload() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        StreamEvent::Error {
            error: ErrorPayload {
                kind: ErrorKind::OverloadedError,
                message: "Overloaded".into(),
            },
        },
    ];
    let out = run(&mut translator, events);
    let failed = out.iter().find_map(|event| match event {
        ResponseStreamEvent::Failed { response } => response.error.clone(),
        _ => None,
    });
    let Some(error) = failed else {
        panic!("expected Failed event");
    };
    assert_eq!(error["type"], json!("overloaded_error"));
    assert_eq!(error["message"], json!("Overloaded"));
}

// ---------------------------------------------------------------------------
// Usage extraction (cumulative across message_delta events)
// ---------------------------------------------------------------------------

#[test]
fn completed_event_includes_cumulative_usage_with_cache_breakdown() {
    let mut translator = translator_with(&[]);
    let events = vec![
        StreamEvent::MessageStart {
            message: MessageStart {
                id: "msg_1".into(),
                model: "claude-opus-4-7".into(),
                role: Role::Assistant,
                usage: Usage {
                    input_tokens: 200,
                    cache_creation_input_tokens: 50,
                    cache_read_input_tokens: 150,
                    output_tokens: 0,
                    cache_creation: None,
                },
            },
        },
        block_start_text(0),
        block_delta_text(0, "Hi"),
        block_stop(0),
        StreamEvent::MessageDelta {
            delta: MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
            },
            usage: Some(Usage {
                output_tokens: 25,
                ..Usage::default()
            }),
        },
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);
    let ResponseStreamEvent::Completed { response } = out.last().expect("event") else {
        panic!("expected Completed");
    };
    let usage = response.usage.as_ref().expect("usage present");
    assert_eq!(usage.input_tokens, 200);
    assert_eq!(usage.output_tokens, 25);
    assert_eq!(usage.total_tokens, 225);
    assert_eq!(
        usage.input_tokens_details.as_ref().map(|d| d.cached_tokens),
        Some(150),
        "cached_tokens = cache_read_input_tokens",
    );
}

// ---------------------------------------------------------------------------
// Multi-block sequence (text → tool_use)
// ---------------------------------------------------------------------------

#[test]
fn text_then_tool_use_assigns_distinct_output_indices() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        block_start_text(0),
        block_delta_text(0, "Calling tool now."),
        block_stop(0),
        block_start_tool_use(1, "toolu_1", "shell"),
        block_delta_input_json(1, "{}"),
        block_stop(1),
        message_delta(StopReason::ToolUse),
        StreamEvent::MessageStop,
    ];
    let out = run(&mut translator, events);
    let indices: Vec<usize> = out
        .iter()
        .filter_map(|event| match event {
            ResponseStreamEvent::OutputItemAdded { output_index, .. }
            | ResponseStreamEvent::OutputItemDone { output_index, .. } => Some(*output_index),
            _ => None,
        })
        .collect();
    // Expected: added(0), done(0), added(1), done(1).
    assert_eq!(indices, vec![0, 0, 1, 1]);
}

// ---------------------------------------------------------------------------
// Web search server tool — surface results to Codex
// ---------------------------------------------------------------------------

#[test]
fn server_tool_use_for_web_search_emits_synthetic_assistant_text_with_query() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: InContent::ServerToolUse {
                id: "srvtoolu_1".into(),
                name: "web_search".into(),
                input: json!({"query": "weather in NYC"}),
            },
        },
        block_stop(0),
    ];
    let out = run(&mut translator, events);
    let text = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::AssistantMessage { text, .. },
            ..
        } => Some(text.clone()),
        _ => None,
    });
    let text = text.expect("expected synthetic assistant text for the web_search call");
    assert!(text.contains("weather in NYC"), "missing query: {text}");
}

#[test]
fn web_search_tool_result_emits_synthetic_assistant_text_with_titles_and_urls() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: InContent::WebSearchToolResult {
                tool_use_id: "srvtoolu_1".into(),
                content: json!([
                    {
                        "type": "web_search_result",
                        "title": "Weather in New York City",
                        "url": "https://weather.com/nyc",
                        "encrypted_content": "ENC1",
                        "page_age": "2 hours ago",
                    },
                    {
                        "type": "web_search_result",
                        "title": "NYC forecast",
                        "url": "https://accuweather.com/nyc",
                        "encrypted_content": "ENC2",
                    },
                ]),
            },
        },
        block_stop(0),
    ];
    let out = run(&mut translator, events);
    let text = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::AssistantMessage { text, .. },
            ..
        } => Some(text.clone()),
        _ => None,
    });
    let text = text.expect("expected synthetic assistant text for web_search results");
    assert!(
        text.contains("Weather in New York City"),
        "missing title: {text}"
    );
    assert!(
        text.contains("https://weather.com/nyc"),
        "missing url: {text}"
    );
    assert!(
        text.contains("NYC forecast"),
        "missing second title: {text}"
    );
    assert!(
        text.contains("https://accuweather.com/nyc"),
        "missing second url: {text}"
    );
}

#[test]
fn web_search_tool_result_with_error_payload_emits_error_text() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_1"),
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: InContent::WebSearchToolResult {
                tool_use_id: "srvtoolu_1".into(),
                content: json!({
                    "type": "web_search_tool_result_error",
                    "error_code": "max_uses_exceeded",
                }),
            },
        },
        block_stop(0),
    ];
    let out = run(&mut translator, events);
    let text = out.iter().find_map(|event| match event {
        ResponseStreamEvent::OutputItemDone {
            item: OutputItem::AssistantMessage { text, .. },
            ..
        } => Some(text.clone()),
        _ => None,
    });
    let text = text.expect("expected synthetic text for web_search error");
    assert!(
        text.contains("max_uses_exceeded"),
        "missing error code: {text}"
    );
}

// ---------------------------------------------------------------------------
// Ping is silently consumed
// ---------------------------------------------------------------------------

#[test]
fn ping_event_emits_nothing() {
    let mut translator = translator_with(&[]);
    let out = translator.consume(StreamEvent::Ping);
    assert!(out.is_empty());
}

// ---------------------------------------------------------------------------
// local_shell tool calls round-trip as Codex `local_shell_call`
// items, NOT generic `function_call` items.
//
// Codex's tool router (`codex-rs/core/src/tools/handlers/shell/local_shell.rs`)
// expects `ToolPayload::LocalShell` and crashes the turn with
// `FunctionCallError::Fatal("LocalShellHandler expected
// ToolPayload::LocalShell")` when it receives a generic function
// call. Anthropic invokes our synthesized `local_shell` function
// tool as a regular `tool_use` block, so the stream translator must
// notice the tool name and rewrap the result as the local-shell-
// specific item type with the documented action shape (per
// `protocol/src/models.rs::LocalShellAction::Exec`).
// ---------------------------------------------------------------------------

#[test]
fn local_shell_tool_use_emits_local_shell_call_output_item() {
    use codex_anthropic_translator::anthropic::event::ContentBlock as InContent;
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_ls"),
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: InContent::ToolUse {
                id: "toolu_ls_1".into(),
                name: "local_shell".into(),
                input: json!({}),
            },
        },
        block_delta_input_json(0, "{\"action\":{\"type\":\"exec\","),
        block_delta_input_json(0, "\"command\":[\"/bin/sh\",\"-c\",\"echo hi\"]}}"),
        block_stop(0),
        message_delta(StopReason::ToolUse),
        StreamEvent::MessageStop,
    ];
    let events = run(&mut translator, events);
    // Pluck the OutputItemDone payload to inspect the synthesized
    // local_shell_call item.
    let done = events
        .iter()
        .find_map(|e| match e {
            ResponseStreamEvent::OutputItemDone { item, .. } => Some(item.clone()),
            _ => None,
        })
        .expect("expected an OutputItemDone");
    let OutputItem::LocalShellCall {
        call_id, action, ..
    } = done
    else {
        panic!(
            "expected OutputItem::LocalShellCall (Codex's LocalShellHandler crashes on \
             ToolPayload::Function), got {done:?}"
        );
    };
    assert_eq!(call_id, "toolu_ls_1");
    // The action must round-trip the documented LocalShellAction::Exec
    // shape (`{type:"exec", command:[...]}`) verbatim so Codex can
    // deserialize it without any post-processing.
    assert_eq!(action["type"], json!("exec"));
    assert_eq!(action["command"], json!(["/bin/sh", "-c", "echo hi"]));
}

#[test]
fn local_shell_tool_use_outputs_local_shell_call_in_added_event_too() {
    // The OutputItemAdded event (emitted at content_block_start) must
    // also be a LocalShellCall — Codex doesn't recover if the added
    // and done events disagree on item type.
    use codex_anthropic_translator::anthropic::event::ContentBlock as InContent;
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_ls2"),
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: InContent::ToolUse {
                id: "toolu_ls_2".into(),
                name: "local_shell".into(),
                input: json!({}),
            },
        },
    ];
    let events = run(&mut translator, events);
    let added = events
        .iter()
        .find_map(|e| match e {
            ResponseStreamEvent::OutputItemAdded { item, .. } => Some(item.clone()),
            _ => None,
        })
        .expect("expected an OutputItemAdded");
    assert!(
        matches!(added, OutputItem::LocalShellCall { .. }),
        "OutputItemAdded for local_shell must be LocalShellCall, got {added:?}",
    );
}

// ---------------------------------------------------------------------------
// redacted_thinking blocks survive the stream by being emitted as
// `Reasoning` items whose `encrypted_content` carries the opaque
// `data` field (analogous to a thinking block's signature). Without
// this, Codex loses the redaction on round-trip and Anthropic
// rejects the next turn for a missing thinking-block signature.
// ---------------------------------------------------------------------------

#[test]
fn redacted_thinking_block_emits_reasoning_with_encrypted_data() {
    use codex_anthropic_translator::anthropic::event::ContentBlock as InContent;
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_redacted"),
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: InContent::RedactedThinking {
                data: "EncryptedRedactedPayload==".into(),
            },
        },
        block_stop(0),
        message_delta(StopReason::EndTurn),
        StreamEvent::MessageStop,
    ];
    let events = run(&mut translator, events);
    let done = events
        .iter()
        .find_map(|e| match e {
            ResponseStreamEvent::OutputItemDone { item, .. } => Some(item.clone()),
            _ => None,
        })
        .expect("expected an OutputItemDone");
    let OutputItem::Reasoning {
        encrypted_content, ..
    } = done
    else {
        panic!("expected OutputItem::Reasoning, got {done:?}");
    };
    assert_eq!(
        encrypted_content.as_deref(),
        Some("EncryptedRedactedPayload=="),
    );
}

// ---------------------------------------------------------------------------
// citation_delta on a text block must not break the stream. The
// translator currently has no Codex-side equivalent for inline
// citations, so the safe behavior is to silently consume them — the
// surrounding text deltas continue to flow.
// ---------------------------------------------------------------------------

#[test]
fn citation_delta_inside_text_block_is_silently_consumed() {
    let mut translator = translator_with(&[]);
    let events = vec![
        message_start("claude-opus-4-7", "msg_cite"),
        block_start_text(0),
        block_delta_text(0, "Hello "),
        StreamEvent::ContentBlockDelta {
            index: 0,
            delta: ContentBlockDelta::CitationDelta {
                citation: serde_json::Value::Object(serde_json::Map::new()),
            },
        },
        block_delta_text(0, "world"),
        block_stop(0),
        message_delta(StopReason::EndTurn),
        StreamEvent::MessageStop,
    ];
    let events = run(&mut translator, events);
    let text_deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            ResponseStreamEvent::OutputTextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["Hello ", "world"]);
}
