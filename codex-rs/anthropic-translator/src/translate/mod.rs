//! Translator: OpenAI Responses request → Anthropic Messages request.
//!
//! Pure functions, no IO. Composed of focused submodules:
//!
//!   * [`model_spec`] — per-model rules (max_tokens, thinking mode,
//!     effort gating).
//!
//! Subsequent slices add `messages`, `tools`, `thinking`, `cache`,
//! and a top-level `translate_request` entry point.

mod cache;
mod messages;
mod model_spec;
mod raw_string_extractor;
mod request;
mod stream;
mod thinking;
mod tools;

pub use cache::apply_cache_plan;
pub use messages::TranslatedMessages;
pub use messages::translate_messages;
pub use model_spec::ModelFamily;
pub use model_spec::ModelSpec;
pub use model_spec::ThinkingMode;
pub use model_spec::model_spec;
pub use request::TranslationError;
pub use request::translate_request;
pub use stream::StreamTranslator;
pub use thinking::ThinkingTranslation;
pub use thinking::translate_thinking;
pub use tools::translate_tools;
