//! Translate the OpenAI-shaped `tools` array Codex emits into
//! Anthropic-shaped tools.
//!
//! The tricky cases are Codex's freeform/Lark-grammar `custom` tools
//! (`apply_patch`, `exec_command`) which we synthesize as
//! `eager_input_streaming` function tools with a single `raw` string
//! property — Anthropic does not accept Lark grammars, so the raw
//! string is the only safe round-trip.

use crate::anthropic::FunctionTool;
use crate::anthropic::Tool;
use crate::anthropic::WebSearchTool;
use crate::anthropic::WebSearchUserLocation;
use serde_json::Value;
use serde_json::json;

/// Translate an OpenAI-shaped tool list into Anthropic-shaped tools,
/// dropping anything we can't represent on Vertex AI.
pub fn translate_tools(values: &[Value]) -> Vec<Tool> {
    values.iter().filter_map(translate_one).collect()
}

fn translate_one(value: &Value) -> Option<Tool> {
    let kind = value.get("type")?.as_str()?;
    match kind {
        "function" => Some(translate_function(value)),
        "custom" => Some(translate_custom(value)),
        "local_shell" => Some(translate_local_shell()),
        "web_search" => Some(translate_web_search(value)),
        // image_generation, tool_search, namespace, future variants:
        // no Vertex-supported equivalent → silently drop. Caller
        // (request translator) may emit a system-level note if it
        // matters.
        _ => None,
    }
}

fn translate_function(value: &Value) -> Tool {
    Tool::Function(FunctionTool {
        name: string_field(value, "name"),
        description: string_field(value, "description"),
        input_schema: value
            .get("parameters")
            .cloned()
            .unwrap_or_else(empty_object_schema),
        strict: bool_field(value, "strict"),
        eager_input_streaming: false,
        cache_control: None,
    })
}

fn translate_custom(value: &Value) -> Tool {
    let original_description = string_field(value, "description");
    let description = match value.get("format") {
        Some(format) if format.is_object() => append_grammar_hint(&original_description, format),
        _ => original_description,
    };
    Tool::Function(FunctionTool {
        name: string_field(value, "name"),
        description,
        input_schema: raw_string_schema(),
        // strict = false: Anthropic's strict mode requires a schema
        // that fully describes the output. A raw freeform string
        // can't be schema-validated at the structural level.
        strict: false,
        // Critical: stream the body without JSON validation so apply_patch
        // bodies arrive at Codex with the same UX as native custom-tool
        // streaming on the OpenAI side.
        eager_input_streaming: true,
        cache_control: None,
    })
}

fn translate_local_shell() -> Tool {
    Tool::Function(FunctionTool {
        name: "local_shell".into(),
        description: "Execute a command in the local shell. The input is a Codex \
            LocalShellAction: an object with `type: \"exec\"` and a `command` argv \
            array (e.g. [\"/bin/sh\", \"-c\", \"...\"])."
            .into(),
        // Schema mirrors Codex's `LocalShellAction` directly so the
        // reverse-translation in the stream layer can take Claude's
        // tool_use input verbatim and wrap it in a local_shell_call
        // ResponseItem with the action set to that object.
        input_schema: json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": ["exec"]},
                "command": {
                    "type": "array",
                    "items": {"type": "string"},
                },
            },
            "required": ["type", "command"],
        }),
        strict: false,
        eager_input_streaming: false,
        cache_control: None,
    })
}

fn translate_web_search(value: &Value) -> Tool {
    let max_uses = value
        .get("max_uses")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    let allowed_domains = value
        .get("filters")
        .and_then(|f| f.get("allowed_domains"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });
    let user_location = value
        .get("user_location")
        .and_then(|loc| serde_json::from_value::<WebSearchUserLocation>(loc.clone()).ok());

    Tool::WebSearch(WebSearchTool {
        max_uses,
        allowed_domains,
        user_location,
        cache_control: None,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn empty_object_schema() -> Value {
    json!({"type": "object"})
}

fn raw_string_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "raw": {"type": "string"},
        },
        "required": ["raw"],
        "additionalProperties": false,
    })
}

fn append_grammar_hint(description: &str, format: &Value) -> String {
    let syntax = format
        .get("syntax")
        .and_then(Value::as_str)
        .unwrap_or("text");
    let definition = format
        .get("definition")
        .and_then(Value::as_str)
        .unwrap_or("");
    if description.is_empty() {
        format!("The `raw` field must be a string in {syntax} format matching:\n{definition}")
    } else {
        format!(
            "{description}\n\nThe `raw` field must be a string in {syntax} format matching:\n{definition}"
        )
    }
}
