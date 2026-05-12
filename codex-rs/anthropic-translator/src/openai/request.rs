//! Inbound (Codex → translator) request types.
//!
//! Field-for-field structural mirror of Codex's `ResponsesApiRequest`
//! and `ResponseItem`. The translator will *consume* most of these
//! fields, drop the ones with no Anthropic equivalent (`store`,
//! `service_tier`, `parallel_tool_calls`, `include`, `text.verbosity`)
//! and translate the rest. Modelling them all here means a Codex
//! payload never fails to deserialize because of a field we don't yet
//! use.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Top-level request body
// ---------------------------------------------------------------------------

/// Body of `POST /v1/responses` as Codex emits it. Mirrors
/// `codex_api::common::ResponsesApiRequest`.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    /// May be empty; serialized with `skip_serializing_if = "String::is_empty"`
    /// on Codex's side, so `default` here covers requests where the
    /// field is absent entirely.
    #[serde(default)]
    pub instructions: String,
    pub input: Vec<ResponseItem>,
    /// OpenAI-shaped tool definitions. Re-parsed by the request
    /// translator into Anthropic shapes; kept opaque here to avoid
    /// modelling a parallel tool universe.
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub tool_choice: String,
    #[serde(default)]
    pub parallel_tool_calls: bool,
    pub reasoning: Option<Reasoning>,
    #[serde(default)]
    pub store: bool,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub include: Vec<String>,
    pub service_tier: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub text: Option<TextControls>,
    pub client_metadata: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Reasoning / effort / summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Reasoning {
    pub effort: Option<ReasoningEffort>,
    pub summary: Option<ReasoningSummary>,
}

/// Effort levels Codex exposes per
/// `codex-rs/protocol/src/openai_models.rs::ReasoningEffort`.
///
/// `None` and `XHigh` flank the canonical `low/medium/high` range:
/// `None` requests minimum reasoning spend; `XHigh` is the
/// highest-effort tier (Opus 4.7 only on the Anthropic side, mapped
/// down to `High` for other models in `translate/thinking.rs`).
///
/// All variants must be modelled — there is no `#[serde(other)]`
/// fallback because an unknown effort would silently abort the
/// entire turn rather than fall back to a sensible default.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
    None,
}

// ---------------------------------------------------------------------------
// Text controls (verbosity + structured output)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TextControls {
    pub verbosity: Option<Verbosity>,
    pub format: Option<TextFormat>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextFormat {
    JsonSchema {
        schema: Value,
        #[serde(default)]
        strict: bool,
        #[serde(default)]
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Response items (entries in `input`)
// ---------------------------------------------------------------------------

/// One entry in the Codex `input` array. Mirrors
/// `codex_protocol::models::ResponseItem` with only the variants the
/// translator needs to read; unknown variants land in
/// [`ResponseItem::Unrecognized`] and are dropped during translation.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    Message {
        #[serde(default)]
        id: Option<String>,
        role: String,
        content: Vec<ContentItem>,
        /// `Commentary` vs `FinalAnswer` per
        /// `protocol/src/models.rs::MessagePhase`. Anthropic has no
        /// equivalent distinction, so the translator drops this on
        /// the way out — but we deserialize it explicitly so future
        /// behavior changes (e.g. annotating commentary with a
        /// different cache strategy) can pick it up without a
        /// re-plumbing pass. Kept as `Option<String>` (rather than
        /// modelling the enum) because the precise value is opaque
        /// to the translator.
        #[serde(default)]
        phase: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        summary: Vec<ReasoningSummaryItem>,
        #[serde(default)]
        content: Option<Vec<ReasoningContentItem>>,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    FunctionCall {
        #[serde(default)]
        id: Option<String>,
        name: String,
        /// Tool-routing namespace per
        /// `protocol/src/models.rs::ResponseItem::FunctionCall`.
        /// Anthropic has no equivalent so the translator drops it on
        /// translation, but we model it explicitly so the Codex turn
        /// always deserializes (serde's silent-ignore-of-unknown
        /// fields is fine, but explicit is clearer and protects
        /// against future workspace-wide deny rules).
        #[serde(default)]
        namespace: Option<String>,
        /// JSON-encoded *string* (Codex never pre-parses the args).
        arguments: String,
        call_id: String,
    },
    FunctionCallOutput {
        call_id: String,
        /// Documented as "string or list of output content" per the
        /// OpenAI Responses API reference; Codex's serializer at
        /// `protocol/src/models.rs:1459-1469` emits exactly those
        /// two shapes. Kept as `Value` so the translator's
        /// `parse_output` can dispatch on the variant.
        output: Value,
    },
    CustomToolCall {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        status: Option<String>,
        call_id: String,
        name: String,
        /// Raw string (e.g. an apply_patch body) — Codex emits no
        /// JSON wrapping for freeform tool inputs.
        input: String,
    },
    CustomToolCallOutput {
        call_id: String,
        #[serde(default)]
        name: Option<String>,
        output: Value,
    },
    LocalShellCall {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
        status: String,
        action: Value,
    },
    /// Forward-compat catch-all: web_search_call, image_generation_call,
    /// tool_search_call, future variants.
    #[serde(other)]
    Unrecognized,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentItem {
    InputText {
        text: String,
    },
    InputImage {
        image_url: String,
        /// `Auto` / `Low` / `High` / `Original` per
        /// `protocol/src/models.rs::ImageDetail`. Anthropic's URL
        /// image source has no detail concept, so the translator
        /// drops this on translation. Kept as `Option<String>` to
        /// avoid coupling to Codex's enum naming.
        #[serde(default)]
        detail: Option<String>,
    },
    OutputText {
        text: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningSummaryItem {
    SummaryText { text: String },
}

/// Items inside a `Reasoning.content` array per
/// `protocol/src/models.rs::ReasoningItemContent`. Both shapes are
/// emitted by Codex (`reasoning_text` is the historical name,
/// `text` is the current shape); modelling only one would crash the
/// entire request when the other appeared.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningContentItem {
    ReasoningText { text: String },
    Text { text: String },
}
