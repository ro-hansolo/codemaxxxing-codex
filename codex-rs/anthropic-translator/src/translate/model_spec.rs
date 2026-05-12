//! Per-model translator rules.
//!
//! Source-of-truth references:
//!
//!   * Models overview:       <https://docs.anthropic.com/en/docs/about-claude/models/overview>
//!   * Adaptive thinking:     <https://docs.anthropic.com/en/docs/build-with-claude/adaptive-thinking>
//!   * Effort:                <https://docs.anthropic.com/en/docs/build-with-claude/effort>
//!
//! When Anthropic ships a new model, add a new [`ModelFamily`]
//! variant and a row in [`model_spec`].

/// Recognised Claude model families. Newer entries are listed first.
///
/// Vertex sometimes appends `@<date>` snapshots to model IDs; the
/// resolver strips the suffix before matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    Opus47,
    Opus46,
    Sonnet46,
    Haiku45,
    Opus45,
    Sonnet45,
    /// Anything we don't recognise — translator falls back to safe
    /// defaults rather than refusing to run.
    Unknown,
}

/// How a model exposes extended thinking.
///
/// `AdaptiveOnly` is reserved for Opus 4.7, where the API rejects
/// manual `enabled` mode with HTTP 400. `Adaptive` and `Manual`
/// describe models that accept either; the translator picks the one
/// the docs recommend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Adaptive is the only legal mode. (Opus 4.7.)
    AdaptiveOnly,
    /// Adaptive recommended; manual still functional but deprecated.
    Adaptive,
    /// Adaptive not supported; emit `enabled` mode with budget_tokens.
    Manual,
}

/// Per-model rule snapshot consumed by the translator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    pub family: ModelFamily,
    /// Default `max_tokens` to inject when Codex omits one (Codex
    /// never sends `max_tokens` because the OpenAI Responses API
    /// doesn't have it as a separate field).
    pub max_tokens_default: u64,
    pub thinking: ThinkingMode,
    pub allows_effort: bool,
    pub allows_xhigh_effort: bool,
    pub allows_max_effort: bool,
}

impl ModelSpec {
    const fn unknown() -> Self {
        Self {
            family: ModelFamily::Unknown,
            // 4096 is the universal minimum for cacheable prompts on
            // every active Claude model and a sensible floor for any
            // unrecognised model ID.
            max_tokens_default: 4096,
            thinking: ThinkingMode::Manual,
            allows_effort: false,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        }
    }
}

/// Resolve a wire-level model ID to its rule snapshot.
///
/// Strips `@<date>` snapshots used by Vertex AI before matching
/// against the prefix table.
pub fn model_spec(model: &str) -> ModelSpec {
    let base = model.split_once('@').map_or(model, |(prefix, _)| prefix);
    match base {
        "claude-opus-4-7" => ModelSpec {
            family: ModelFamily::Opus47,
            max_tokens_default: 128_000,
            thinking: ThinkingMode::AdaptiveOnly,
            allows_effort: true,
            allows_xhigh_effort: true,
            allows_max_effort: true,
        },
        "claude-opus-4-6" => ModelSpec {
            family: ModelFamily::Opus46,
            max_tokens_default: 128_000,
            thinking: ThinkingMode::Adaptive,
            allows_effort: true,
            allows_xhigh_effort: false,
            allows_max_effort: true,
        },
        "claude-sonnet-4-6" => ModelSpec {
            family: ModelFamily::Sonnet46,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Adaptive,
            allows_effort: true,
            allows_xhigh_effort: false,
            allows_max_effort: true,
        },
        "claude-haiku-4-5" => ModelSpec {
            family: ModelFamily::Haiku45,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Manual,
            allows_effort: false,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        },
        "claude-opus-4-5" => ModelSpec {
            family: ModelFamily::Opus45,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Manual,
            allows_effort: true,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        },
        "claude-sonnet-4-5" => ModelSpec {
            family: ModelFamily::Sonnet45,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Manual,
            allows_effort: false,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        },
        _ => ModelSpec::unknown(),
    }
}
