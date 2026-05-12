//! Per-model rule table used by the request translator.
//!
//! Source-of-truth references:
//!
//!   * <https://docs.anthropic.com/en/docs/about-claude/models/overview>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/adaptive-thinking>
//!   * <https://docs.anthropic.com/en/docs/build-with-claude/effort>
//!
//! Constraints encoded in this table are enforced by the request
//! translator. Adding a new model means:
//!   1. Add a `model_spec` entry here.
//!   2. Add a regression test below pinning the entry.

use codex_anthropic_translator::translate::ModelFamily;
use codex_anthropic_translator::translate::ModelSpec;
use codex_anthropic_translator::translate::ThinkingMode;
use codex_anthropic_translator::translate::model_spec;
use pretty_assertions::assert_eq;

#[test]
fn opus_4_7_uses_adaptive_only_with_full_effort_range_and_128k_max() {
    // Opus 4.7 hard-rejects manual thinking (HTTP 400). Adaptive is
    // the only mode. xhigh is Opus-4.7-only and is the recommended
    // starting effort for coding/agentic workloads. Max output is
    // 128k tokens per the latest models overview.
    assert_eq!(
        model_spec("claude-opus-4-7"),
        ModelSpec {
            family: ModelFamily::Opus47,
            max_tokens_default: 128_000,
            thinking: ThinkingMode::AdaptiveOnly,
            allows_effort: true,
            allows_xhigh_effort: true,
            allows_max_effort: true,
        },
    );
}

#[test]
fn opus_4_6_uses_adaptive_with_max_effort_and_128k_max() {
    // Opus 4.6 supports both adaptive (recommended) and manual.
    // Translator picks adaptive. Max effort allowed; xhigh is not.
    assert_eq!(
        model_spec("claude-opus-4-6"),
        ModelSpec {
            family: ModelFamily::Opus46,
            max_tokens_default: 128_000,
            thinking: ThinkingMode::Adaptive,
            allows_effort: true,
            allows_xhigh_effort: false,
            allows_max_effort: true,
        },
    );
}

#[test]
fn sonnet_4_6_uses_adaptive_with_max_effort_and_64k_max() {
    assert_eq!(
        model_spec("claude-sonnet-4-6"),
        ModelSpec {
            family: ModelFamily::Sonnet46,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Adaptive,
            allows_effort: true,
            allows_xhigh_effort: false,
            allows_max_effort: true,
        },
    );
}

#[test]
fn haiku_4_5_uses_manual_thinking_with_no_effort_support() {
    // Haiku 4.5 does not support adaptive thinking and is not in the
    // effort parameter's supported model list — translator falls back
    // to manual budget_tokens.
    assert_eq!(
        model_spec("claude-haiku-4-5"),
        ModelSpec {
            family: ModelFamily::Haiku45,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Manual,
            allows_effort: false,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        },
    );
}

#[test]
fn opus_4_5_uses_manual_thinking_with_effort_support() {
    // Opus 4.5 is the only "older Opus" still in the effort doc's
    // supported list, with manual thinking only.
    assert_eq!(
        model_spec("claude-opus-4-5"),
        ModelSpec {
            family: ModelFamily::Opus45,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Manual,
            allows_effort: true,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        },
    );
}

#[test]
fn sonnet_4_5_uses_manual_thinking_without_effort_support() {
    assert_eq!(
        model_spec("claude-sonnet-4-5"),
        ModelSpec {
            family: ModelFamily::Sonnet45,
            max_tokens_default: 64_000,
            thinking: ThinkingMode::Manual,
            allows_effort: false,
            allows_xhigh_effort: false,
            allows_max_effort: false,
        },
    );
}

#[test]
fn opus_4_7_alias_with_dated_suffix_resolves_correctly() {
    // Vertex sometimes appends @date suffixes to model IDs (e.g.
    // `claude-opus-4-7@20260101`). Translator must accept those.
    assert_eq!(
        model_spec("claude-opus-4-7@20260101").family,
        ModelFamily::Opus47,
    );
}

#[test]
fn unknown_model_falls_back_to_safe_defaults() {
    // Unknown model defaults to manual thinking + 4096 max_tokens
    // (the Anthropic-wide minimum across active models). The
    // translator surfaces a soft warning but does not error — caller
    // can still steer to a known-good model via configuration.
    let spec = model_spec("claude-future-x");
    assert_eq!(spec.family, ModelFamily::Unknown);
    assert_eq!(spec.thinking, ThinkingMode::Manual);
    assert!(!spec.allows_effort);
    assert!(spec.max_tokens_default >= 4096);
}
