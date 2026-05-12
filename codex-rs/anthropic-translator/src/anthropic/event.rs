//! Anthropic Messages SSE event types.
//!
//! These are the `data:` payloads carried by each `event:` line of the
//! `/v1/messages` SSE stream. The translator deserializes each into
//! the `StreamEvent` enum and re-emits the equivalent OpenAI Responses
//! event downstream (in the stream-translator slice).
//!
//! Reference: <https://docs.anthropic.com/en/docs/build-with-claude/streaming>

use serde::Deserialize;
use serde_json::Value;

use crate::anthropic::Role;

// ---------------------------------------------------------------------------
// Top-level event enum
// ---------------------------------------------------------------------------

/// One discriminator-tagged event from the SSE stream.
///
/// Per the streaming docs, the order of events on a successful turn
/// is: `MessageStart`, then a sequence of `ContentBlockStart` /
/// `ContentBlockDelta`* / `ContentBlockStop` triples (one per content
/// block), followed by one or more `MessageDelta` events, then
/// `MessageStop`. `Ping` may appear anywhere; `Error` may interrupt
/// the stream after the initial 200 OK.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: MessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: ContentBlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDelta,
        /// Per the [streaming
        /// reference](https://docs.anthropic.com/en/docs/build-with-claude/streaming),
        /// `usage` is the canonical shape of a `message_delta`, but
        /// the API's versioning policy explicitly allows new shapes
        /// and SDK error-recovery paths sometimes synthesize
        /// `message_delta` events without one. `Option` + `default`
        /// keeps stream parsing alive in those cases; the stream
        /// translator merges `Some(usage)` into the rolling
        /// cumulative total and treats `None` as "no new usage
        /// info".
        #[serde(default)]
        usage: Option<Usage>,
    },
    MessageStop,
    Ping,
    Error {
        error: ErrorPayload,
    },
}

// ---------------------------------------------------------------------------
// message_start payload
// ---------------------------------------------------------------------------

/// Initial assistant message envelope. `content` is always `[]` in
/// `message_start` (deltas populate it later) and is intentionally
/// dropped during deserialization.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageStart {
    pub id: String,
    pub model: String,
    pub role: Role,
    #[serde(default)]
    pub usage: Usage,
}

// ---------------------------------------------------------------------------
// Content blocks (as they appear in content_block_start)
// ---------------------------------------------------------------------------

/// Initial shape of a content block when it opens.
///
/// This is structurally similar to the request-side `ContentBlock`
/// but distinct: stream-side blocks carry no `cache_control` (caching
/// is request-only) and add server-tool variants we'd never emit
/// outbound.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    /// Safety-filtered thinking content. Per the
    /// [extended-thinking docs](https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking),
    /// the model emits these in place of regular `thinking` blocks
    /// when reasoning is redacted; the opaque `data` field is the
    /// signature equivalent and MUST be round-tripped on the next
    /// turn, otherwise Anthropic rejects the request for missing
    /// thinking-block validation.
    RedactedThinking {
        data: String,
    },
    /// User-defined or Anthropic-schema client tool call. The
    /// translator forwards this to Codex as a `FunctionCall` (or
    /// `LocalShellCall` for the synthesized `local_shell` tool).
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Anthropic-hosted server tool call (web_search). We do not emit
    /// these to Codex (Codex has no concept of server tools); the
    /// translator either drops them or surfaces them as an info note.
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Result of an Anthropic-hosted server tool. The `content`
    /// payload's structure depends on which server tool ran; we keep
    /// it as an opaque `Value` and forward verbatim.
    WebSearchToolResult {
        tool_use_id: String,
        content: Value,
    },
}

// ---------------------------------------------------------------------------
// Content block deltas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockDelta {
    /// Appended text fragment for a `text` block.
    TextDelta { text: String },
    /// Partial JSON fragment for a `tool_use` block. Concatenate
    /// `partial_json` across deltas; parse once `content_block_stop`
    /// arrives. Fragments may not be valid JSON on their own.
    InputJsonDelta { partial_json: String },
    /// Appended thinking text for a `thinking` block (only emitted
    /// when `thinking.display` is `"summarized"`).
    ThinkingDelta { thinking: String },
    /// Opaque signature for a `thinking` block. Arrives just before
    /// `content_block_stop`. The translator MUST round-trip this
    /// signature back to Anthropic on the next turn that includes
    /// thinking + tool use, otherwise the request is rejected.
    SignatureDelta { signature: String },
    /// Inline citation for a `text` block, emitted when web search
    /// is enabled and a result is referenced. See the [web search
    /// streaming docs](https://docs.anthropic.com/en/docs/build-with-claude/tool-use/web-search-tool#streaming).
    /// Codex has no inline-citation concept so the stream translator
    /// silently consumes these — they're modelled as a variant
    /// (rather than dropped via `#[serde(other)]`) so the surrounding
    /// `text_delta` events on the same block continue to flow.
    CitationDelta {
        #[serde(rename = "citation")]
        citation: Value,
    },
}

// ---------------------------------------------------------------------------
// message_delta payload
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelta {
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
}

/// Why Claude stopped generating. Maps onto Codex's `Completed` event
/// `end_turn` flag in the stream translator.
///
/// Forward-compat: any new variant Anthropic adds lands in `Unknown`
/// rather than failing to deserialize. The stream translator treats
/// `Unknown` as "end_turn" with a warning.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// Used by long-running agentic flows to signal the model paused
    /// rather than fully stopped.
    PauseTurn,
    /// Safety-triggered stop.
    Refusal,
    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Token-usage breakdown reported in `message_start.message.usage`
/// and (cumulatively) in each `message_delta.usage`.
///
/// All counts default to zero so a partial usage payload deserializes
/// without `Option` boilerplate at the consumer.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Per-TTL breakdown of cache writes when 5m and 1h breakpoints
    /// are mixed in the same request. Absent when only one TTL is
    /// used.
    pub cache_creation: Option<CacheCreationBreakdown>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheCreationBreakdown {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorPayload {
    #[serde(rename = "type")]
    pub kind: ErrorKind,
    pub message: String,
}

/// Documented Anthropic HTTP error types (per the
/// [errors reference](https://docs.anthropic.com/en/api/errors)),
/// plus a forward-compat `Unknown` arm.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidRequestError,
    AuthenticationError,
    BillingError,
    PermissionError,
    NotFoundError,
    RequestTooLarge,
    RateLimitError,
    ApiError,
    TimeoutError,
    OverloadedError,
    #[serde(other)]
    Unknown,
}

impl ErrorKind {
    /// Wire string used when reflecting the error back to Codex over
    /// SSE. Mirrors the snake_case wire identifiers Anthropic uses.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            ErrorKind::InvalidRequestError => "invalid_request_error",
            ErrorKind::AuthenticationError => "authentication_error",
            ErrorKind::BillingError => "billing_error",
            ErrorKind::PermissionError => "permission_error",
            ErrorKind::NotFoundError => "not_found_error",
            ErrorKind::RequestTooLarge => "request_too_large",
            ErrorKind::RateLimitError => "rate_limit_error",
            ErrorKind::ApiError => "api_error",
            ErrorKind::TimeoutError => "timeout_error",
            ErrorKind::OverloadedError => "overloaded_error",
            ErrorKind::Unknown => "api_error",
        }
    }
}
