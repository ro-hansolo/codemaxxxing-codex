//! Outgoing Anthropic Messages request types.
//!
//! Every type in this module emits the latest Anthropic Messages API
//! wire format and is supported on Vertex AI per the Anthropic
//! [features overview](https://docs.anthropic.com/en/docs/build-with-claude/overview)
//! (verified 2026-05-12).
//!
//! Vertex compatibility floor: do not add types or fields here for
//! features that are not at least beta on Vertex AI. The translator
//! relies on this invariant to remain a pure Anthropic-shape emitter
//! that anthroproxy can forward without further translation. Beta
//! features (compaction, context editing) require the upstream client
//! to send the matching `anthropic-beta` header.
//!
//! Source-of-truth references:
//!   * <https://docs.anthropic.com/en/api/messages>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/structured-outputs>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/adaptive-thinking>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/effort>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/define-tools>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/tool-reference>
//!   * <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/fine-grained-tool-streaming>

use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use serde_json::Value;

/// Wire identifier for the `web_search` server tool, pinned to the
/// version Anthropic-on-Vertex accepts. Per the [web search tool
/// docs](https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool):
/// "On Vertex AI, only the basic web search tool (without dynamic
/// filtering) is available." The newer `web_search_20260209` adds
/// dynamic filtering and is rejected by Vertex's request validator.
/// Bump only after confirming Vertex support in the Anthropic
/// [features overview](https://docs.anthropic.com/en/docs/build-with-claude/overview).
pub const WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";

// ---------------------------------------------------------------------------
// Top-level request
// ---------------------------------------------------------------------------

/// Body of `POST /v1/messages` (or `:streamRawPredict` on Vertex behind
/// anthroproxy).
///
/// Construct with field literals; `..MessageRequest::default()` covers
/// every optional field. `model`, `max_tokens`, and at least one
/// `messages` entry are required by Anthropic and the type does not
/// enforce that — the request translator is responsible.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    pub messages: Vec<Message>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

// ---------------------------------------------------------------------------
// Message + role + content blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Content block carried inside `messages[i].content` (and inside
/// `tool_result` when the result is structured rather than a plain
/// string).
///
/// Thinking blocks intentionally have no `cache_control` field: per
/// the prompt-caching docs, `cache_control` cannot be applied to
/// thinking blocks directly. They get cached transparently when
/// covered by a breakpoint on a later block.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

impl ContentBlock {
    /// Build a plain text block with no cache breakpoint. Used heavily
    /// at every callsite that materializes user/assistant text so the
    /// `cache_control: None` boilerplate doesn't repeat.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            cache_control: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

/// Tool-result content can be either a flat string or a list of
/// structured blocks (text, image, document). We use the untagged
/// representation because Anthropic distinguishes them by JSON shape,
/// not a tag.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

/// A single block of the top-level `system` array. Always emits
/// `"type": "text"` on the wire.
#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub text: String,
    pub cache_control: Option<CacheControl>,
}

impl Serialize for SystemBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 2 + usize::from(self.cache_control.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("type", "text")?;
        map.serialize_entry("text", &self.text)?;
        if let Some(cc) = &self.cache_control {
            map.serialize_entry("cache_control", cc)?;
        }
        map.end()
    }
}

// ---------------------------------------------------------------------------
// Cache control
// ---------------------------------------------------------------------------

/// Ephemeral cache breakpoint marker.
///
/// `type` is always `"ephemeral"` (the only supported cache type
/// today). `ttl` defaults to 5 minutes and may be raised to 1 hour;
/// when mixing TTLs in a single request, longer TTLs must precede
/// shorter ones in prompt order — that ordering is the planner's
/// responsibility.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub ttl: Option<CacheTtl>,
}

impl CacheControl {
    /// 5-minute ephemeral breakpoint (the default).
    pub fn ephemeral() -> Self {
        Self::default()
    }
}

impl Serialize for CacheControl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 1 + usize::from(self.ttl.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("type", "ephemeral")?;
        if let Some(ttl) = &self.ttl {
            map.serialize_entry("ttl", ttl)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum CacheTtl {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Entry in the `tools` array. Function tools carry no `type` tag on
/// the wire; server tools (currently just web search in our target
/// surface) carry a date-versioned `type` literal.
#[derive(Debug, Clone)]
pub enum Tool {
    Function(FunctionTool),
    WebSearch(WebSearchTool),
}

impl Serialize for Tool {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Tool::Function(tool) => tool.serialize(serializer),
            Tool::WebSearch(tool) => tool.serialize(serializer),
        }
    }
}

/// User-defined function tool.
///
/// `strict: true` requests constrained decoding so Claude's `input`
/// is guaranteed to match `input_schema`. `eager_input_streaming:
/// true` opts the tool into fine-grained streaming — we use this for
/// `apply_patch` so its raw body streams to Codex without buffering.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FunctionTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub eager_input_streaming: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// Anthropic-hosted web search server tool. The `type` and `name`
/// fields are wire-format constants (see [`WEB_SEARCH_TOOL_TYPE`]).
#[derive(Debug, Clone, Default)]
pub struct WebSearchTool {
    pub max_uses: Option<u32>,
    pub allowed_domains: Option<Vec<String>>,
    pub user_location: Option<WebSearchUserLocation>,
    pub cache_control: Option<CacheControl>,
}

impl Serialize for WebSearchTool {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 2
            + usize::from(self.max_uses.is_some())
            + usize::from(self.allowed_domains.is_some())
            + usize::from(self.user_location.is_some())
            + usize::from(self.cache_control.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("type", WEB_SEARCH_TOOL_TYPE)?;
        map.serialize_entry("name", "web_search")?;
        if let Some(max_uses) = &self.max_uses {
            map.serialize_entry("max_uses", max_uses)?;
        }
        if let Some(allowed_domains) = &self.allowed_domains {
            map.serialize_entry("allowed_domains", allowed_domains)?;
        }
        if let Some(user_location) = &self.user_location {
            map.serialize_entry("user_location", user_location)?;
        }
        if let Some(cache_control) = &self.cache_control {
            map.serialize_entry("cache_control", cache_control)?;
        }
        map.end()
    }
}

/// `user_location` payload for the web search tool. The wire `type`
/// is always `"approximate"`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WebSearchUserLocation {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
}

impl Serialize for WebSearchUserLocation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 1
            + usize::from(self.country.is_some())
            + usize::from(self.region.is_some())
            + usize::from(self.city.is_some())
            + usize::from(self.timezone.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("type", "approximate")?;
        if let Some(country) = &self.country {
            map.serialize_entry("country", country)?;
        }
        if let Some(region) = &self.region {
            map.serialize_entry("region", region)?;
        }
        if let Some(city) = &self.city {
            map.serialize_entry("city", city)?;
        }
        if let Some(timezone) = &self.timezone {
            map.serialize_entry("timezone", timezone)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Tool { name: String },
    None,
}

// ---------------------------------------------------------------------------
// Thinking
// ---------------------------------------------------------------------------

/// Thinking configuration.
///
/// **`Adaptive`** is the only mode accepted by Opus 4.7 (the manual
/// `Enabled` mode returns 400). It is also the recommended mode for
/// Opus 4.6 and Sonnet 4.6. Thinking depth is controlled separately
/// via [`OutputConfig::effort`], not via this enum.
///
/// **`Enabled`** is for older models (Opus 4.5, Sonnet 4.5, Haiku
/// 4.5, …) that still require an explicit `budget_tokens`. The
/// request translator is responsible for picking the right variant
/// per model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Adaptive {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    Enabled {
        budget_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingDisplay {
    Summarized,
    Omitted,
}

// ---------------------------------------------------------------------------
// Output config (effort + structured outputs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<JsonOutputFormat>,
}

/// Effort levels documented in the [effort
/// reference](https://docs.anthropic.com/en/docs/build-with-claude/effort).
///
/// Model-availability constraints (enforced by the translator, not
/// the type):
///
///   * `Xhigh` — Opus 4.7 only.
///   * `Max`   — Opus 4.7, Opus 4.6, Sonnet 4.6, Mythos.
///   * `Low`/`Medium`/`High` — every model that accepts effort.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsonOutputFormat {
    JsonSchema { schema: Value },
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `serde` `skip_serializing_if` predicate — used by `is_error`,
/// `strict`, and `eager_input_streaming` so the wire payload stays
/// minimal (and so `tool_choice`-cache-invalidation doesn't kick in
/// over a no-op `false`).
///
/// The `&bool` signature is required by `skip_serializing_if`, which
/// only accepts `fn(&T) -> bool`; clippy's
/// `trivially_copy_pass_by_ref` lint does not apply to serde
/// callbacks.
#[inline]
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}
