//! Apply a [`CachePlan`] to a freshly-built [`MessageRequest`] by
//! mutating the matching content blocks in place to attach
//! `cache_control: ephemeral`.
//!
//! The planner emits one of three breakpoint kinds; this layer maps
//! each to a concrete attachment site:
//!
//!   * [`Breakpoint::System`] — last block of the `system` array.
//!   * [`Breakpoint::Tools`]  — last entry of the `tools` array.
//!   * [`Breakpoint::Message`] — last content block of the message at
//!     the given index.

use codex_anthropic_translator::Breakpoint;
use codex_anthropic_translator::CachePlan;
use codex_anthropic_translator::anthropic::CacheControl;
use codex_anthropic_translator::anthropic::ContentBlock;
use codex_anthropic_translator::anthropic::FunctionTool;
use codex_anthropic_translator::anthropic::MessageRequest;
use codex_anthropic_translator::anthropic::Role;
use codex_anthropic_translator::anthropic::SystemBlock;
use codex_anthropic_translator::anthropic::Tool;
use codex_anthropic_translator::anthropic::WebSearchTool;
use codex_anthropic_translator::translate::apply_cache_plan;
use pretty_assertions::assert_eq;

fn user_msg(text: &str) -> codex_anthropic_translator::anthropic::Message {
    codex_anthropic_translator::anthropic::Message {
        role: Role::User,
        content: vec![ContentBlock::text(text)],
    }
}

fn assistant_msg(text: &str) -> codex_anthropic_translator::anthropic::Message {
    codex_anthropic_translator::anthropic::Message {
        role: Role::Assistant,
        content: vec![ContentBlock::text(text)],
    }
}

fn fn_tool(name: &str) -> Tool {
    Tool::Function(FunctionTool {
        name: name.into(),
        description: String::new(),
        input_schema: serde_json::json!({"type": "object"}),
        ..FunctionTool::default()
    })
}

// ---------------------------------------------------------------------------
// System breakpoint
// ---------------------------------------------------------------------------

#[test]
fn system_breakpoint_attaches_cache_control_to_last_system_block() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        system: vec![
            SystemBlock {
                text: "first".into(),
                cache_control: None,
            },
            SystemBlock {
                text: "last".into(),
                cache_control: None,
            },
        ],
        messages: vec![user_msg("hi")],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::System],
        },
    );
    assert!(req.system[0].cache_control.is_none());
    assert_eq!(req.system[1].cache_control, Some(CacheControl::ephemeral()));
}

// ---------------------------------------------------------------------------
// Tools breakpoint
// ---------------------------------------------------------------------------

#[test]
fn tools_breakpoint_attaches_cache_control_to_last_tool() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        tools: vec![fn_tool("a"), fn_tool("b"), fn_tool("c")],
        messages: vec![user_msg("hi")],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::Tools],
        },
    );
    let Tool::Function(first) = &req.tools[0] else {
        panic!("expected function");
    };
    assert!(first.cache_control.is_none());
    let Tool::Function(last) = &req.tools[2] else {
        panic!("expected function");
    };
    assert_eq!(last.cache_control, Some(CacheControl::ephemeral()));
}

#[test]
fn tools_breakpoint_attaches_cache_control_to_web_search_when_last() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        tools: vec![fn_tool("shell"), Tool::WebSearch(WebSearchTool::default())],
        messages: vec![user_msg("hi")],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::Tools],
        },
    );
    let Tool::WebSearch(t) = &req.tools[1] else {
        panic!("expected web_search");
    };
    assert_eq!(t.cache_control, Some(CacheControl::ephemeral()));
}

// ---------------------------------------------------------------------------
// Message breakpoint
// ---------------------------------------------------------------------------

#[test]
fn message_breakpoint_attaches_cache_control_to_last_content_block() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![
            user_msg("u1"),
            assistant_msg("a1"),
            user_msg("u2"),
            assistant_msg("a2"),
        ],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![
                Breakpoint::Message { message_index: 1 },
                Breakpoint::Message { message_index: 3 },
            ],
        },
    );
    let ContentBlock::Text { cache_control, .. } = &req.messages[1].content[0] else {
        panic!("expected text");
    };
    assert_eq!(*cache_control, Some(CacheControl::ephemeral()));
    let ContentBlock::Text { cache_control, .. } = &req.messages[3].content[0] else {
        panic!("expected text");
    };
    assert_eq!(*cache_control, Some(CacheControl::ephemeral()));
}

#[test]
fn message_breakpoint_attaches_to_last_block_of_multi_block_message() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![codex_anthropic_translator::anthropic::Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: "sig".into(),
                },
                ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "shell".into(),
                    input: serde_json::json!({}),
                    cache_control: None,
                },
            ],
        }],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::Message { message_index: 0 }],
        },
    );
    // Thinking block intentionally lacks cache_control (Anthropic
    // forbids it). The last block (ToolUse) should carry it.
    let ContentBlock::ToolUse { cache_control, .. } = &req.messages[0].content[1] else {
        panic!("expected tool_use");
    };
    assert_eq!(*cache_control, Some(CacheControl::ephemeral()));
}

#[test]
fn message_breakpoint_skips_thinking_blocks_when_seeking_target() {
    // Walks back from the end to find the first non-Thinking block,
    // since Anthropic rejects cache_control on thinking blocks.
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![codex_anthropic_translator::anthropic::Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("plan:"),
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: "sig".into(),
                },
            ],
        }],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::Message { message_index: 0 }],
        },
    );
    let ContentBlock::Text { cache_control, .. } = &req.messages[0].content[0] else {
        panic!("expected text");
    };
    assert_eq!(*cache_control, Some(CacheControl::ephemeral()));
}

// ---------------------------------------------------------------------------
// Composite plans + edge cases
// ---------------------------------------------------------------------------

#[test]
fn composite_plan_with_system_tools_and_message_breakpoints() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        system: vec![SystemBlock {
            text: "sys".into(),
            cache_control: None,
        }],
        tools: vec![fn_tool("shell")],
        messages: vec![user_msg("u1"), assistant_msg("a1")],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![
                Breakpoint::System,
                Breakpoint::Tools,
                Breakpoint::Message { message_index: 1 },
            ],
        },
    );
    assert_eq!(req.system[0].cache_control, Some(CacheControl::ephemeral()));
    let Tool::Function(t) = &req.tools[0] else {
        panic!("expected function");
    };
    assert_eq!(t.cache_control, Some(CacheControl::ephemeral()));
    let ContentBlock::Text { cache_control, .. } = &req.messages[1].content[0] else {
        panic!("expected text");
    };
    assert_eq!(*cache_control, Some(CacheControl::ephemeral()));
}

#[test]
fn empty_plan_leaves_request_unchanged() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        system: vec![SystemBlock {
            text: "sys".into(),
            cache_control: None,
        }],
        tools: vec![fn_tool("shell")],
        messages: vec![user_msg("u")],
        ..MessageRequest::default()
    };
    let snapshot = req.clone();
    apply_cache_plan(&mut req, &CachePlan::default());
    // Snapshot equality via JSON since MessageRequest doesn't derive PartialEq.
    assert_eq!(
        serde_json::to_value(&req).unwrap(),
        serde_json::to_value(&snapshot).unwrap(),
    );
}

#[test]
fn plan_with_message_index_out_of_range_is_silently_ignored() {
    // The planner emits indices it computed itself, but defensively
    // an out-of-range entry should be a no-op rather than a panic.
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![user_msg("u")],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::Message { message_index: 99 }],
        },
    );
    let ContentBlock::Text { cache_control, .. } = &req.messages[0].content[0] else {
        panic!("expected text");
    };
    assert_eq!(*cache_control, None);
}

#[test]
fn plan_with_system_breakpoint_but_no_system_blocks_is_silently_ignored() {
    let mut req = MessageRequest {
        model: "claude-opus-4-7".into(),
        max_tokens: 1024,
        messages: vec![user_msg("u")],
        ..MessageRequest::default()
    };
    apply_cache_plan(
        &mut req,
        &CachePlan {
            breakpoints: vec![Breakpoint::System],
        },
    );
    assert!(req.system.is_empty());
}
