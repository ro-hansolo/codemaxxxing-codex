//! Behavioural contract for the cache breakpoint planner.
//!
//! Every public branch in `CachePlan::compute` is exercised. The cap is
//! sourced from the crate's `MAX_BREAKPOINTS` constant rather than being
//! re-declared here so a future change to the cap fails loudly.

use codex_anthropic_translator::Breakpoint;
use codex_anthropic_translator::CachePlan;
use codex_anthropic_translator::MAX_BREAKPOINTS;
use codex_anthropic_translator::PlanInput;
use pretty_assertions::assert_eq;

fn input(has_system: bool, has_tools: bool, boundaries: &[usize]) -> PlanInput<'_> {
    PlanInput {
        has_system,
        has_tools,
        assistant_turn_boundaries: boundaries,
    }
}

#[test]
fn anthropic_breakpoint_cap_is_four() {
    // Pinned by the Anthropic Messages API contract; if Anthropic ever
    // raises the cap, the planner needs an explicit decision before that
    // change can flow through.
    assert_eq!(MAX_BREAKPOINTS, 4);
}

#[test]
fn empty_input_yields_empty_plan() {
    assert_eq!(
        CachePlan::compute(&input(false, false, &[])),
        CachePlan::default(),
    );
}

#[test]
fn system_only_emits_single_system_breakpoint() {
    assert_eq!(
        CachePlan::compute(&input(true, false, &[])),
        CachePlan {
            breakpoints: vec![Breakpoint::System],
        },
    );
}

#[test]
fn tools_only_emits_single_tools_breakpoint() {
    assert_eq!(
        CachePlan::compute(&input(false, true, &[])),
        CachePlan {
            breakpoints: vec![Breakpoint::Tools],
        },
    );
}

#[test]
fn system_and_tools_with_no_boundaries_skip_message_breakpoints() {
    assert_eq!(
        CachePlan::compute(&input(true, true, &[])),
        CachePlan {
            breakpoints: vec![Breakpoint::System, Breakpoint::Tools],
        },
    );
}

#[test]
fn one_boundary_appended_after_pinned_breakpoints() {
    assert_eq!(
        CachePlan::compute(&input(true, true, &[1])),
        CachePlan {
            breakpoints: vec![
                Breakpoint::System,
                Breakpoint::Tools,
                Breakpoint::Message { message_index: 1 },
            ],
        },
    );
}

#[test]
fn two_boundaries_fill_remaining_slots_in_natural_order() {
    assert_eq!(
        CachePlan::compute(&input(true, true, &[1, 3])),
        CachePlan {
            breakpoints: vec![
                Breakpoint::System,
                Breakpoint::Tools,
                Breakpoint::Message { message_index: 1 },
                Breakpoint::Message { message_index: 3 },
            ],
        },
    );
}

#[test]
fn extra_boundaries_drop_oldest_to_keep_newest_within_cap() {
    let plan = CachePlan::compute(&input(true, true, &[1, 3, 5, 7, 9]));
    assert_eq!(
        plan,
        CachePlan {
            breakpoints: vec![
                Breakpoint::System,
                Breakpoint::Tools,
                Breakpoint::Message { message_index: 7 },
                Breakpoint::Message { message_index: 9 },
            ],
        },
    );
    assert!(plan.breakpoints.len() <= MAX_BREAKPOINTS);
}

#[test]
fn without_pinned_breakpoints_all_four_slots_go_to_message_tail() {
    let plan = CachePlan::compute(&input(false, false, &[2, 4, 6, 8, 10, 12]));
    assert_eq!(
        plan,
        CachePlan {
            breakpoints: vec![
                Breakpoint::Message { message_index: 6 },
                Breakpoint::Message { message_index: 8 },
                Breakpoint::Message { message_index: 10 },
                Breakpoint::Message { message_index: 12 },
            ],
        },
    );
    assert_eq!(plan.breakpoints.len(), MAX_BREAKPOINTS);
}

#[test]
fn fewer_boundaries_than_slots_uses_all_boundaries() {
    assert_eq!(
        CachePlan::compute(&input(false, false, &[2])),
        CachePlan {
            breakpoints: vec![Breakpoint::Message { message_index: 2 }],
        },
    );
}

#[test]
fn breakpoints_are_emitted_in_request_payload_order() {
    // Order invariant: system precedes tools precedes message tail, and
    // message-tail entries are oldest-first. Anthropic's mixed-TTL rule
    // depends on this ordering even though we use a single TTL today.
    let plan = CachePlan::compute(&input(true, true, &[3, 7]));
    assert_eq!(
        plan.breakpoints,
        vec![
            Breakpoint::System,
            Breakpoint::Tools,
            Breakpoint::Message { message_index: 3 },
            Breakpoint::Message { message_index: 7 },
        ],
    );
}

#[test]
fn pure_function_returns_identical_plans_for_identical_input() {
    // No hidden state — pinning this prevents any future regression that
    // introduces global mutability.
    let i = input(true, true, &[1, 3, 5]);
    assert_eq!(CachePlan::compute(&i), CachePlan::compute(&i));
}

#[test]
fn breakpoint_message_variant_carries_index() {
    // Lock the public Breakpoint shape so the request translator can
    // pattern-match on it without surprises.
    match (Breakpoint::Message { message_index: 42 }) {
        Breakpoint::Message { message_index } => assert_eq!(message_index, 42),
        Breakpoint::System | Breakpoint::Tools => unreachable!(),
    }
}
