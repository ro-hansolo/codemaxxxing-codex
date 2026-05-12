//! Translator that exposes the OpenAI Responses API on the front and
//! speaks the Anthropic Messages API on the back.
//!
//! The Codex CLI is hard-locked to the Responses wire protocol
//! (`codex-rs/model-provider-info/src/lib.rs:51`), so the only way to
//! drive Anthropic Claude models is to translate. This crate is the
//! translator.
//!
//! ```text
//!   codex CLI ── POST /v1/responses (OpenAI, SSE) ──► translator
//!                                                          │
//!         ┌────────────────────────────────────────────────┘
//!         │  POST /v1/messages (Anthropic, SSE)
//!         ▼
//!     anthroproxy ──► Anthropic-on-Vertex (api.anthropic.com shape)
//! ```
//!
//! Anthroproxy presents the standard Anthropic Messages API with a
//! different base URL; we never speak Vertex directly. Every feature
//! the translator emits must therefore be supported on Vertex AI per
//! the Anthropic features overview.

mod cache_state;

pub mod anthropic;
pub mod openai;
pub mod server;
pub mod translate;

pub use cache_state::Breakpoint;
pub use cache_state::CachePlan;
pub use cache_state::MAX_BREAKPOINTS;
pub use cache_state::PlanInput;
