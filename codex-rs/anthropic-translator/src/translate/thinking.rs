//! Translate Codex's `reasoning` field + a model spec into an
//! Anthropic [`ThinkingConfig`] and (optionally) an
//! `output_config.effort` value.

use crate::anthropic::Effort as AnthropicEffort;
use crate::anthropic::ThinkingConfig;
use crate::anthropic::ThinkingDisplay;
use crate::openai::Reasoning;
use crate::openai::ReasoningEffort;
use crate::openai::ReasoningSummary;
use crate::translate::ModelSpec;
use crate::translate::ThinkingMode;

/// Result of mapping codex `reasoning` + a model spec into the two
/// Anthropic concepts that derive from them.
#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingTranslation {
    /// Always present for Claude models — the translator never emits
    /// `thinking: {type: "disabled"}` because Codex's reasoning model
    /// is opt-out via `summary: "none"`, not opt-out of thinking
    /// entirely.
    pub thinking: Option<ThinkingConfig>,
    /// `output_config.effort` value, or `None` for models that don't
    /// support the effort parameter (Sonnet 4.5, Haiku 4.5, …).
    pub output_config_effort: Option<AnthropicEffort>,
}

/// Map codex `reasoning` (effort + summary) plus the model rules into
/// the Anthropic `thinking` config and `output_config.effort`.
pub fn translate_thinking(spec: &ModelSpec, reasoning: Option<&Reasoning>) -> ThinkingTranslation {
    // Default codex effort is `high` — matches the OpenAI Responses
    // API default and Anthropic's default. The translator is
    // opinionated for Codex's agentic-coding workload: high is the
    // floor, never below.
    let codex_effort = reasoning
        .and_then(|r| r.effort)
        .unwrap_or(ReasoningEffort::High);
    let display = translate_display(reasoning.and_then(|r| r.summary));
    let effort = spec
        .allows_effort
        .then(|| translate_effort(codex_effort, spec));

    let thinking = match spec.thinking {
        ThinkingMode::AdaptiveOnly | ThinkingMode::Adaptive => ThinkingConfig::Adaptive { display },
        ThinkingMode::Manual => ThinkingConfig::Enabled {
            budget_tokens: budget_for_manual(codex_effort),
            display,
        },
    };

    ThinkingTranslation {
        thinking: Some(thinking),
        output_config_effort: effort,
    }
}

/// Codex effort → Anthropic effort. Promotes `high` → `xhigh` on
/// Opus 4.7 per the effort doc's recommendation for coding/agentic
/// workloads. `xhigh` falls back to `high` on models that don't
/// support it (Sonnet 4.6, Sonnet 4.5, Haiku 4.5). Anthropic has no
/// `none` or `minimal` levels, so both map down to `low`.
fn translate_effort(codex: ReasoningEffort, spec: &ModelSpec) -> AnthropicEffort {
    match codex {
        ReasoningEffort::None | ReasoningEffort::Minimal | ReasoningEffort::Low => {
            AnthropicEffort::Low
        }
        ReasoningEffort::Medium => AnthropicEffort::Medium,
        ReasoningEffort::High => {
            if spec.allows_xhigh_effort {
                AnthropicEffort::Xhigh
            } else {
                AnthropicEffort::High
            }
        }
        ReasoningEffort::XHigh => {
            if spec.allows_xhigh_effort {
                AnthropicEffort::Xhigh
            } else {
                AnthropicEffort::High
            }
        }
    }
}

/// Budget for manual `enabled` thinking mode. Calibrated for Codex's
/// agentic workload — high needs room for multi-tool reasoning,
/// minimal needs only a quick scratchpad. `XHigh` gets the largest
/// budget (Anthropic doesn't fail requests for budgets above the
/// model's effective ceiling — it just uses what it needs); `None`
/// pins to the floor.
fn budget_for_manual(codex: ReasoningEffort) -> u32 {
    match codex {
        ReasoningEffort::None | ReasoningEffort::Minimal => 2048,
        ReasoningEffort::Low => 4096,
        ReasoningEffort::Medium => 16_000,
        ReasoningEffort::High => 32_000,
        ReasoningEffort::XHigh => 64_000,
    }
}

/// Codex `summary` → Anthropic `display`. `None` means the user opted
/// out of seeing reasoning, which maps to `omitted` (faster TTFT).
/// Every other value (Auto, Concise, Detailed) collapses to
/// `summarized` because Anthropic doesn't distinguish between them.
fn translate_display(summary: Option<ReasoningSummary>) -> Option<ThinkingDisplay> {
    Some(match summary {
        Some(ReasoningSummary::None) => ThinkingDisplay::Omitted,
        _ => ThinkingDisplay::Summarized,
    })
}
