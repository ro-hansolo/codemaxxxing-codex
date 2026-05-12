//! Per-thread cache breakpoint planner.
//!
//! Anthropic accepts up to 4 explicit `cache_control: ephemeral`
//! breakpoints per request. We assume Anthropic's automatic caching is
//! unavailable for our purposes, so every breakpoint is placed manually.
//!
//! Strategy (deterministic, pure function of the request):
//!
//! 1. **System** — last block of the `system` array. Pinned whenever a
//!    `system` array is sent.
//! 2. **Tools** — last entry of the `tools` array. Pinned whenever any
//!    tool is sent.
//! 3. **Message tail** — the most recent completed assistant turns,
//!    newest first, until all [`MAX_BREAKPOINTS`] slots are filled. Each
//!    completed turn is a stable prefix boundary that earlier requests
//!    will have written to the cache.
//!
//! There is no per-thread state: identical request structures always
//! produce identical plans. If 1-hour TTL promotion or hit-rate-aware
//! placement becomes necessary later, that logic lives in a separate
//! stateful wrapper rather than mutating this planner.

/// Maximum number of `cache_control` breakpoints permitted by Anthropic
/// in a single request. Pinned by the cache-state contract.
pub const MAX_BREAKPOINTS: usize = 4;

/// A single breakpoint placement decision the request translator must
/// apply when materializing the outgoing Anthropic payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Breakpoint {
    /// Tag the last text block of the `system` array.
    System,
    /// Tag the last entry of the `tools` array.
    Tools,
    /// Tag the last content block of `messages[message_index]`.
    Message { message_index: usize },
}

/// Inputs derived from the (in-progress) translated Anthropic request.
#[derive(Debug, Clone)]
pub struct PlanInput<'a> {
    /// `true` if the outgoing request will include a non-empty `system`
    /// array.
    pub has_system: bool,
    /// `true` if the outgoing request will include a non-empty `tools`
    /// array.
    pub has_tools: bool,
    /// Indices into the outgoing `messages` array that correspond to the
    /// last assistant message of a completed assistant turn — i.e. an
    /// assistant message that is followed by either nothing more or by a
    /// user message. Must be ordered oldest-first.
    pub assistant_turn_boundaries: &'a [usize],
}

/// The placement plan returned by [`CachePlan::compute`]. Always within
/// [`MAX_BREAKPOINTS`] entries and ordered to match the position the
/// breakpoints will occupy in the outgoing prompt (system → tools →
/// messages oldest-first).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[must_use]
pub struct CachePlan {
    pub breakpoints: Vec<Breakpoint>,
}

impl CachePlan {
    /// Compute the breakpoint plan for the next outgoing Anthropic
    /// request. Pure function: identical inputs always yield identical
    /// plans.
    pub fn compute(input: &PlanInput<'_>) -> Self {
        let mut breakpoints = Vec::with_capacity(MAX_BREAKPOINTS);
        if input.has_system {
            breakpoints.push(Breakpoint::System);
        }
        if input.has_tools {
            breakpoints.push(Breakpoint::Tools);
        }

        let remaining = MAX_BREAKPOINTS - breakpoints.len();
        let boundaries = input.assistant_turn_boundaries;
        let tail_start = boundaries.len().saturating_sub(remaining);
        breakpoints.extend(
            boundaries[tail_start..]
                .iter()
                .map(|&message_index| Breakpoint::Message { message_index }),
        );

        Self { breakpoints }
    }
}
