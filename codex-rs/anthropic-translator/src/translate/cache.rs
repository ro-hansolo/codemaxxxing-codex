//! Apply a [`crate::CachePlan`] to a built [`crate::anthropic::MessageRequest`].
//!
//! Mutates the request in place to attach `cache_control: ephemeral`
//! to the matching system block, tool, or message tail. Each
//! breakpoint kind is independent; bad indices are silently ignored
//! (defensive, since the planner is the only intended caller).

use crate::CachePlan;
use crate::anthropic::CacheControl;
use crate::anthropic::ContentBlock;
use crate::anthropic::MessageRequest;
use crate::anthropic::Tool;
use crate::cache_state::Breakpoint;

/// Attach `cache_control` markers to the request per the supplied
/// plan. Operates in place.
pub fn apply_cache_plan(request: &mut MessageRequest, plan: &CachePlan) {
    for breakpoint in &plan.breakpoints {
        match breakpoint {
            Breakpoint::System => attach_to_last_system(request),
            Breakpoint::Tools => attach_to_last_tool(request),
            Breakpoint::Message { message_index } => {
                attach_to_message_tail(request, *message_index);
            }
        }
    }
}

fn attach_to_last_system(request: &mut MessageRequest) {
    if let Some(last) = request.system.last_mut() {
        last.cache_control = Some(CacheControl::ephemeral());
    }
}

fn attach_to_last_tool(request: &mut MessageRequest) {
    let Some(last) = request.tools.last_mut() else {
        return;
    };
    match last {
        Tool::Function(t) => t.cache_control = Some(CacheControl::ephemeral()),
        Tool::WebSearch(t) => t.cache_control = Some(CacheControl::ephemeral()),
    }
}

fn attach_to_message_tail(request: &mut MessageRequest, index: usize) {
    let Some(message) = request.messages.get_mut(index) else {
        return;
    };
    // Walk back from the end to find the first block that accepts a
    // cache_control marker. Anthropic forbids `cache_control` on
    // thinking / redacted_thinking blocks, so we skip them.
    for block in message.content.iter_mut().rev() {
        if attach_to_block(block) {
            return;
        }
    }
}

/// Returns `true` if the block accepted a cache_control marker.
fn attach_to_block(block: &mut ContentBlock) -> bool {
    match block {
        ContentBlock::Text { cache_control, .. }
        | ContentBlock::Image { cache_control, .. }
        | ContentBlock::ToolUse { cache_control, .. }
        | ContentBlock::ToolResult { cache_control, .. } => {
            *cache_control = Some(CacheControl::ephemeral());
            true
        }
        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => false,
    }
}
