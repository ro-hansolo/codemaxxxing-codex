//! Anthropic Messages API wire types.
//!
//! Split into two submodules:
//!
//!   * [`request`] — types we serialize when calling
//!     `POST /v1/messages` against anthroproxy.
//!   * [`event`] — types we deserialize from the SSE stream returned
//!     by that POST.
//!
//! Request types are re-exported at the parent module level for
//! ergonomic access. Event types live behind the `event` submodule
//! path because their `ContentBlock` would collide with the
//! request-side `ContentBlock` (the two have different shapes — the
//! event one carries no `cache_control` and adds server-tool
//! variants).

mod request;

pub mod event;

pub use request::CacheControl;
pub use request::CacheTtl;
pub use request::ContentBlock;
pub use request::Effort;
pub use request::FunctionTool;
pub use request::ImageSource;
pub use request::JsonOutputFormat;
pub use request::Message;
pub use request::MessageRequest;
pub use request::Metadata;
pub use request::OutputConfig;
pub use request::Role;
pub use request::SystemBlock;
pub use request::ThinkingConfig;
pub use request::ThinkingDisplay;
pub use request::Tool;
pub use request::ToolChoice;
pub use request::ToolResultContent;
pub use request::WEB_SEARCH_TOOL_TYPE;
pub use request::WebSearchTool;
pub use request::WebSearchUserLocation;
