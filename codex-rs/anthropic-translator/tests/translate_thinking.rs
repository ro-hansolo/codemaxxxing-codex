//! Translator rules for thinking + effort.
//!
//! Codex sends `reasoning: {effort, summary}`. Anthropic splits this:
//! `thinking` chooses the mode (adaptive vs enabled) and display
//! preference; `output_config.effort` controls token spend (and
//! thinking depth for adaptive). The mapping is per-model.

use codex_anthropic_translator::anthropic::Effort as AnthropicEffort;
use codex_anthropic_translator::anthropic::OutputConfig;
use codex_anthropic_translator::anthropic::ThinkingConfig;
use codex_anthropic_translator::anthropic::ThinkingDisplay;
use codex_anthropic_translator::openai::Reasoning;
use codex_anthropic_translator::openai::ReasoningEffort;
use codex_anthropic_translator::openai::ReasoningSummary;
use codex_anthropic_translator::translate::ThinkingTranslation;
use codex_anthropic_translator::translate::model_spec;
use codex_anthropic_translator::translate::translate_thinking;
use pretty_assertions::assert_eq;

fn translate(model: &str, reasoning: Option<Reasoning>) -> ThinkingTranslation {
    translate_thinking(&model_spec(model), reasoning.as_ref())
}

// ---------------------------------------------------------------------------
// Opus 4.7 (adaptive only, supports xhigh + max)
// ---------------------------------------------------------------------------

#[test]
fn opus_4_7_with_high_effort_emits_xhigh_and_summarized_display() {
    // Per the effort doc: "Start with `xhigh` for coding and agentic
    // use cases" on Opus 4.7. Codex's `high` is the highest user-
    // facing setting, so it maps to `xhigh` for this model.
    // `display: "summarized"` must be explicit — Opus 4.7 defaults to
    // omitted otherwise, and we want reasoning summaries to show.
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(
        result,
        ThinkingTranslation {
            thinking: Some(ThinkingConfig::Adaptive {
                display: Some(ThinkingDisplay::Summarized),
            }),
            output_config_effort: Some(AnthropicEffort::Xhigh),
        },
    );
}

#[test]
fn opus_4_7_with_medium_effort_emits_medium() {
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Medium),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::Medium));
}

#[test]
fn opus_4_7_with_low_effort_emits_low() {
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Low),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::Low));
}

#[test]
fn opus_4_7_with_minimal_effort_emits_low() {
    // Anthropic has no `minimal` level; map down to `low`.
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Minimal),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::Low));
}

#[test]
fn opus_4_7_with_summary_none_emits_omitted_display() {
    // Codex's `summary: "none"` means the user does not want
    // reasoning surfaced — use display:"omitted" so the stream skips
    // thinking_delta events entirely.
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::None),
        }),
    );
    assert_eq!(
        result.thinking,
        Some(ThinkingConfig::Adaptive {
            display: Some(ThinkingDisplay::Omitted),
        }),
    );
}

#[test]
fn opus_4_7_with_no_reasoning_field_still_enables_adaptive_thinking() {
    // Even when Codex omits `reasoning`, Opus 4.7 is best-served by
    // adaptive thinking (so reasoning appears in the stream). Default
    // effort is high (the API default) which we promote to xhigh per
    // the effort doc's recommendation.
    let result = translate("claude-opus-4-7", None);
    assert_eq!(
        result.thinking,
        Some(ThinkingConfig::Adaptive {
            display: Some(ThinkingDisplay::Summarized),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::Xhigh));
}

// ---------------------------------------------------------------------------
// Opus 4.6 / Sonnet 4.6 (adaptive recommended, max effort allowed but no xhigh)
// ---------------------------------------------------------------------------

#[test]
fn opus_4_6_high_effort_stays_high_no_xhigh_promotion() {
    let result = translate(
        "claude-opus-4-6",
        Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(
        result,
        ThinkingTranslation {
            thinking: Some(ThinkingConfig::Adaptive {
                display: Some(ThinkingDisplay::Summarized),
            }),
            output_config_effort: Some(AnthropicEffort::High),
        },
    );
}

#[test]
fn sonnet_4_6_high_effort_stays_high() {
    let result = translate(
        "claude-sonnet-4-6",
        Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::High));
}

// ---------------------------------------------------------------------------
// Older models (manual thinking with budget_tokens)
// ---------------------------------------------------------------------------

#[test]
fn opus_4_5_uses_manual_enabled_thinking_with_budget_from_effort() {
    // Opus 4.5 supports effort but does not support adaptive thinking.
    // Translator uses `enabled` mode with a budget derived from the
    // effort level, plus passes effort through (Opus 4.5 is in the
    // effort doc's supported list).
    let result = translate(
        "claude-opus-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(
        result,
        ThinkingTranslation {
            thinking: Some(ThinkingConfig::Enabled {
                budget_tokens: 32_000,
                display: Some(ThinkingDisplay::Summarized),
            }),
            output_config_effort: Some(AnthropicEffort::High),
        },
    );
}

#[test]
fn haiku_4_5_uses_manual_enabled_thinking_without_effort() {
    // Haiku 4.5 doesn't support effort or adaptive — manual budget
    // only, no output_config.effort field.
    let result = translate(
        "claude-haiku-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Medium),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(
        result,
        ThinkingTranslation {
            thinking: Some(ThinkingConfig::Enabled {
                budget_tokens: 16_000,
                display: Some(ThinkingDisplay::Summarized),
            }),
            output_config_effort: None,
        },
    );
}

#[test]
fn sonnet_4_5_uses_manual_enabled_thinking_without_effort() {
    let result = translate(
        "claude-sonnet-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(
        result,
        ThinkingTranslation {
            thinking: Some(ThinkingConfig::Enabled {
                budget_tokens: 32_000,
                display: Some(ThinkingDisplay::Summarized),
            }),
            output_config_effort: None,
        },
    );
}

#[test]
fn manual_thinking_minimal_effort_uses_smallest_budget() {
    let result = translate(
        "claude-haiku-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Minimal),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert!(matches!(
        result.thinking,
        Some(ThinkingConfig::Enabled {
            budget_tokens: 2048,
            ..
        })
    ));
}

#[test]
fn manual_thinking_with_summary_none_uses_omitted_display() {
    let result = translate(
        "claude-haiku-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Medium),
            summary: Some(ReasoningSummary::None),
        }),
    );
    let Some(ThinkingConfig::Enabled { display, .. }) = result.thinking else {
        panic!("expected manual enabled thinking");
    };
    assert_eq!(display, Some(ThinkingDisplay::Omitted));
}

// ---------------------------------------------------------------------------
// Output: OutputConfig assembly
// ---------------------------------------------------------------------------

#[test]
fn output_config_built_from_translation_only_emits_effort_when_present() {
    // The translator builds OutputConfig from
    // ThinkingTranslation.output_config_effort plus the format from
    // text.format. Here we focus on effort-only.
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::Medium),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    let cfg = OutputConfig {
        effort: result.output_config_effort,
        format: None,
    };
    assert_eq!(cfg.effort, Some(AnthropicEffort::Medium));
    assert!(cfg.format.is_none());
}

// ---------------------------------------------------------------------------
// XHigh and None efforts (added so Codex turns sending these don't
// fail to deserialize and so the translator maps them sensibly into
// the Anthropic effort/budget axes).
// ---------------------------------------------------------------------------

#[test]
fn opus_4_7_with_xhigh_effort_emits_xhigh() {
    // Codex's `xhigh` is the highest effort it exposes. For Opus 4.7
    // (which supports xhigh per the effort doc), pass it through as
    // Anthropic xhigh — never downgrade.
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::XHigh),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::Xhigh));
}

#[test]
fn xhigh_effort_on_model_without_xhigh_falls_back_to_high() {
    // For Sonnet 4.6 (no xhigh per the effort doc), xhigh maps down
    // to high so the request still validates on Vertex.
    let result = translate(
        "claude-sonnet-4-6",
        Some(Reasoning {
            effort: Some(ReasoningEffort::XHigh),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::High));
}

#[test]
fn opus_4_7_with_none_effort_still_enables_thinking_at_low() {
    // Codex's `effort: "none"` means the user wants minimum reasoning
    // spend. Anthropic doesn't expose a "no thinking" mode at the
    // effort axis (extended thinking is opt-out via thinking config,
    // not effort), so we keep thinking enabled but pin effort to the
    // floor (`low`).
    let result = translate(
        "claude-opus-4-7",
        Some(Reasoning {
            effort: Some(ReasoningEffort::None),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    assert_eq!(result.output_config_effort, Some(AnthropicEffort::Low));
}

#[test]
fn manual_thinking_with_xhigh_effort_uses_largest_budget() {
    // Manual mode (Sonnet 4.5, Haiku 4.5) needs an explicit
    // budget_tokens. xhigh budget is the largest tier (matches the
    // effort doc's recommendation for coding/agentic workloads).
    let result = translate(
        "claude-haiku-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::XHigh),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    let Some(ThinkingConfig::Enabled { budget_tokens, .. }) = result.thinking else {
        panic!("expected manual enabled thinking");
    };
    assert!(
        budget_tokens >= 32_000,
        "xhigh manual budget should be the largest tier, got {budget_tokens}"
    );
}

#[test]
fn manual_thinking_with_none_effort_uses_smallest_budget() {
    let result = translate(
        "claude-haiku-4-5",
        Some(Reasoning {
            effort: Some(ReasoningEffort::None),
            summary: Some(ReasoningSummary::Auto),
        }),
    );
    let Some(ThinkingConfig::Enabled { budget_tokens, .. }) = result.thinking else {
        panic!("expected manual enabled thinking");
    };
    assert!(
        budget_tokens <= 2048,
        "none manual budget should be at the floor, got {budget_tokens}"
    );
}
