//! Top-level request translator: codex `ResponsesRequest` →
//! Anthropic `MessageRequest`.
//!
//! This module composes the per-component translators in
//! [`crate::translate`] and applies the cache plan as the final
//! step. The resulting `MessageRequest` is ready to be serialized
//! and shipped at anthroproxy's `POST /v1/messages` endpoint.

use crate::CachePlan;
use crate::PlanInput;
use crate::anthropic::JsonOutputFormat;
use crate::anthropic::MessageRequest;
use crate::anthropic::Metadata;
use crate::anthropic::OutputConfig;
use crate::anthropic::SystemBlock;
use crate::anthropic::ToolChoice;
use crate::openai::ResponsesRequest;
use crate::openai::TextFormat;
use crate::translate::ModelFamily;
use crate::translate::apply_cache_plan;
use crate::translate::model_spec;
use crate::translate::translate_messages;
use crate::translate::translate_thinking;
use crate::translate::translate_tools;
use std::collections::HashMap;

/// Failure modes for [`translate_request`]. Translator-level errors
/// only — wire-level / network errors live in the upstream client.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TranslationError {
    /// Codex sent a model ID we don't recognise as a Claude model.
    /// Translator refuses rather than silently routing a non-Claude
    /// model into anthroproxy.
    #[error(
        "unsupported model: {0:?} — only Claude models routed through anthroproxy are supported"
    )]
    UnsupportedModel(String),
}

/// The header key Codex uses to ship its installation ID through
/// `client_metadata`. Lifted directly into Anthropic's
/// `metadata.user_id` (the only metadata field Anthropic accepts).
const INSTALLATION_ID_KEY: &str = "x-codex-installation-id";

/// Translate Codex's outgoing request body into the Anthropic Messages
/// payload anthroproxy will forward to Vertex.
pub fn translate_request(request: ResponsesRequest) -> Result<MessageRequest, TranslationError> {
    let spec = model_spec(&request.model);
    if matches!(spec.family, ModelFamily::Unknown) {
        return Err(TranslationError::UnsupportedModel(request.model));
    }

    let translated_messages = translate_messages(request.input);
    let tools = translate_tools(&request.tools);
    let thinking = translate_thinking(&spec, request.reasoning.as_ref());
    let format = request
        .text
        .and_then(|controls| controls.format)
        .map(|TextFormat::JsonSchema { schema, .. }| JsonOutputFormat::JsonSchema { schema });
    let output_config = build_output_config(thinking.output_config_effort, format);
    let system = build_system(request.instructions);
    let tool_choice = (!tools.is_empty()).then(|| translate_tool_choice(&request.tool_choice));
    let metadata = build_metadata(request.client_metadata);

    let mut built = MessageRequest {
        model: request.model,
        max_tokens: spec.max_tokens_default,
        system,
        tools,
        tool_choice,
        thinking: thinking.thinking,
        output_config,
        // Codex always streams; flip stream:false to stream:true
        // defensively. Long agentic turns over non-streaming requests
        // are rejected by Anthropic's 10-minute timeout.
        stream: true,
        messages: translated_messages.messages,
        metadata,
    };

    let plan = CachePlan::compute(&PlanInput {
        has_system: !built.system.is_empty(),
        has_tools: !built.tools.is_empty(),
        assistant_turn_boundaries: &translated_messages.assistant_turn_boundaries,
    });
    apply_cache_plan(&mut built, &plan);

    Ok(built)
}

fn build_system(instructions: String) -> Vec<SystemBlock> {
    if instructions.is_empty() {
        Vec::new()
    } else {
        vec![SystemBlock {
            text: instructions,
            cache_control: None,
        }]
    }
}

fn build_output_config(
    effort: Option<crate::anthropic::Effort>,
    format: Option<JsonOutputFormat>,
) -> Option<OutputConfig> {
    if effort.is_none() && format.is_none() {
        None
    } else {
        Some(OutputConfig { effort, format })
    }
}

fn build_metadata(client_metadata: Option<HashMap<String, String>>) -> Option<Metadata> {
    let user_id = client_metadata.and_then(|mut m| m.remove(INSTALLATION_ID_KEY));
    user_id.map(|id| Metadata { user_id: Some(id) })
}

fn translate_tool_choice(value: &str) -> ToolChoice {
    match value {
        "any" | "required" => ToolChoice::Any,
        "none" => ToolChoice::None,
        // Codex emits `tool_choice` as a flat string and currently
        // only sends "auto"; everything else (including the empty
        // default) collapses to Auto.
        _ => ToolChoice::Auto,
    }
}
