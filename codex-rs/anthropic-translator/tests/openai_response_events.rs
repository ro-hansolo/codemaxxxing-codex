//! Wire-format contract tests for outgoing Codex SSE event types
//! (what the translator emits back to Codex over `/v1/responses`).
//!
//! These shapes are parsed on Codex's side at
//! `codex-rs/codex-api/src/sse/responses.rs:300-428`. Each test pins
//! one event type so a future refactor of the Codex parser can flag
//! shape drift loudly.

use codex_anthropic_translator::openai::OutputItem;
use codex_anthropic_translator::openai::ResponseObject;
use codex_anthropic_translator::openai::ResponseStreamEvent;
use codex_anthropic_translator::openai::ResponseUsage;
use codex_anthropic_translator::openai::ResponseUsageInputDetails;
use codex_anthropic_translator::openai::ResponseUsageOutputDetails;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn to_value(event: &ResponseStreamEvent) -> Value {
    match serde_json::to_value(event) {
        Ok(value) => value,
        Err(err) => panic!("event must serialize: {err}"),
    }
}

#[test]
fn response_created_event_has_typed_response_object() {
    let event = ResponseStreamEvent::Created {
        response: ResponseObject::new("resp_1", "claude-opus-4-7"),
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.created",
            "response": {
                "id": "resp_1",
                "object": "response",
                "model": "claude-opus-4-7",
            },
        }),
    );
}

#[test]
fn output_item_added_for_assistant_text_message() {
    let event = ResponseStreamEvent::OutputItemAdded {
        output_index: 0,
        item: OutputItem::AssistantMessage {
            id: "msg_1".into(),
            text: String::new(),
        },
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "content": [{"type": "output_text", "text": ""}],
            },
        }),
    );
}

#[test]
fn output_item_added_for_function_call() {
    let event = ResponseStreamEvent::OutputItemAdded {
        output_index: 1,
        item: OutputItem::FunctionCall {
            id: "fc_1".into(),
            call_id: "call_1".into(),
            name: "shell".into(),
            arguments: String::new(),
        },
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call_1",
                "name": "shell",
                "arguments": "",
            },
        }),
    );
}

#[test]
fn output_item_added_for_custom_tool_call() {
    let event = ResponseStreamEvent::OutputItemAdded {
        output_index: 1,
        item: OutputItem::CustomToolCall {
            id: "ctc_1".into(),
            call_id: "call_p".into(),
            name: "apply_patch".into(),
            input: String::new(),
        },
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {
                "type": "custom_tool_call",
                "id": "ctc_1",
                "call_id": "call_p",
                "name": "apply_patch",
                "input": "",
            },
        }),
    );
}

#[test]
fn output_item_added_for_reasoning_block() {
    let event = ResponseStreamEvent::OutputItemAdded {
        output_index: 0,
        item: OutputItem::Reasoning {
            id: "rs_1".into(),
            encrypted_content: None,
        },
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "reasoning",
                "id": "rs_1",
                "summary": [],
            },
        }),
    );
}

#[test]
fn output_item_done_for_reasoning_includes_encrypted_content() {
    let event = ResponseStreamEvent::OutputItemDone {
        output_index: 0,
        item: OutputItem::Reasoning {
            id: "rs_1".into(),
            encrypted_content: Some("ENC...".into()),
        },
    };
    assert_eq!(
        to_value(&event)["item"]["encrypted_content"],
        json!("ENC..."),
    );
}

#[test]
fn output_text_delta_carries_item_id_and_content_index() {
    let event = ResponseStreamEvent::OutputTextDelta {
        item_id: "msg_1".into(),
        content_index: 0,
        delta: "Hello".into(),
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1",
            "content_index": 0,
            "delta": "Hello",
        }),
    );
}

#[test]
fn custom_tool_call_input_delta_carries_call_id_and_delta() {
    let event = ResponseStreamEvent::CustomToolCallInputDelta {
        item_id: "ctc_1".into(),
        call_id: "call_p".into(),
        delta: "*** Begin Patch\n".into(),
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.custom_tool_call_input.delta",
            "item_id": "ctc_1",
            "call_id": "call_p",
            "delta": "*** Begin Patch\n",
        }),
    );
}

#[test]
fn reasoning_summary_text_delta_carries_summary_index() {
    let event = ResponseStreamEvent::ReasoningSummaryTextDelta {
        item_id: "rs_1".into(),
        summary_index: 0,
        delta: "Step one...".into(),
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1",
            "summary_index": 0,
            "delta": "Step one...",
        }),
    );
}

#[test]
fn reasoning_summary_part_added_carries_summary_index() {
    let event = ResponseStreamEvent::ReasoningSummaryPartAdded {
        item_id: "rs_1".into(),
        summary_index: 0,
    };
    assert_eq!(
        to_value(&event),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_1",
            "summary_index": 0,
        }),
    );
}

#[test]
fn response_completed_includes_usage_with_cached_input_tokens() {
    let event = ResponseStreamEvent::Completed {
        response: ResponseObject::new("resp_1", "claude-opus-4-7").with_usage(ResponseUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            input_tokens_details: Some(ResponseUsageInputDetails { cached_tokens: 80 }),
            output_tokens_details: Some(ResponseUsageOutputDetails {
                reasoning_tokens: 20,
            }),
        }),
    };
    let value = to_value(&event);
    assert_eq!(value["type"], json!("response.completed"));
    assert_eq!(
        value["response"]["usage"],
        json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "total_tokens": 150,
            "input_tokens_details": {"cached_tokens": 80},
            "output_tokens_details": {"reasoning_tokens": 20},
        }),
    );
}

#[test]
fn response_failed_carries_error_payload() {
    let event = ResponseStreamEvent::Failed {
        response: ResponseObject::new("resp_1", "claude-opus-4-7")
            .with_error(json!({"type": "overloaded_error", "message": "Overloaded"})),
    };
    assert_eq!(
        to_value(&event)["response"]["error"],
        json!({"type": "overloaded_error", "message": "Overloaded"}),
    );
}

#[test]
fn response_completed_marks_end_turn_when_set() {
    let event = ResponseStreamEvent::Completed {
        response: ResponseObject::new("resp_1", "claude-opus-4-7").with_end_turn(true),
    };
    assert_eq!(to_value(&event)["response"]["end_turn"], json!(true));
}
