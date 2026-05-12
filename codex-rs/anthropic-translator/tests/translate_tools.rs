//! Translator rules for the OpenAI `tools` array → Anthropic `tools`.
//!
//! Codex emits OpenAI-shaped tool definitions with these `type`
//! values (per `codex-rs/tools/src/tool_spec.rs`):
//!
//!   * `function` — JSON-schema function tool (most tools).
//!   * `custom` — freeform/Lark-grammar tool (apply_patch,
//!     exec_command); needs schema synthesis.
//!   * `local_shell` — Codex executes locally; pass through as a
//!     regular function tool with a known input schema.
//!   * `web_search` — translator forwards to Anthropic's hosted
//!     `web_search_20250305` server tool (the version Vertex
//!     accepts; see `WEB_SEARCH_TOOL_TYPE` in `anthropic::request`).
//!   * `image_generation`, `tool_search`, `namespace` — drop (no
//!     Anthropic-on-Vertex equivalent).

use codex_anthropic_translator::anthropic::FunctionTool;
use codex_anthropic_translator::anthropic::Tool;
use codex_anthropic_translator::anthropic::WEB_SEARCH_TOOL_TYPE;
use codex_anthropic_translator::anthropic::WebSearchTool;
use codex_anthropic_translator::translate::translate_tools;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn translate(values: Vec<Value>) -> Vec<Tool> {
    translate_tools(&values)
}

// ---------------------------------------------------------------------------
// `function` tool
// ---------------------------------------------------------------------------

#[test]
fn function_tool_with_parameters_becomes_anthropic_function_with_input_schema() {
    let tools = translate(vec![json!({
        "type": "function",
        "name": "shell",
        "description": "Run a shell command.",
        "parameters": {
            "type": "object",
            "properties": {"cmd": {"type": "string"}},
            "required": ["cmd"],
        },
    })]);
    assert_eq!(tools.len(), 1);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function variant");
    };
    assert_eq!(t.name, "shell");
    assert_eq!(t.description, "Run a shell command.");
    assert_eq!(
        t.input_schema,
        json!({
            "type": "object",
            "properties": {"cmd": {"type": "string"}},
            "required": ["cmd"],
        }),
    );
    assert!(!t.eager_input_streaming);
}

#[test]
fn function_tool_marked_strict_round_trips_strict_field() {
    let tools = translate(vec![json!({
        "type": "function",
        "name": "shell",
        "description": "Run a shell command.",
        "parameters": {"type": "object"},
        "strict": true,
    })]);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function");
    };
    assert!(t.strict);
}

#[test]
fn function_tool_with_no_description_uses_empty_string() {
    // Anthropic requires `description`; if Codex emits a tool without
    // one we substitute an empty string rather than dropping the tool.
    let tools = translate(vec![json!({
        "type": "function",
        "name": "noop",
        "parameters": {"type": "object"},
    })]);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function");
    };
    assert_eq!(t.description, "");
}

#[test]
fn function_tool_without_parameters_uses_empty_object_schema() {
    // Anthropic requires `input_schema`; missing parameters → minimal
    // object schema (lets Claude call the tool with no input).
    let tools = translate(vec![json!({
        "type": "function",
        "name": "ping",
        "description": "Ping.",
    })]);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function");
    };
    assert_eq!(t.input_schema, json!({"type": "object"}));
}

// ---------------------------------------------------------------------------
// `custom` (freeform / Lark grammar) tool
// ---------------------------------------------------------------------------

#[test]
fn apply_patch_custom_tool_synthesizes_raw_string_schema() {
    // apply_patch's wire shape from Codex includes a Lark grammar
    // definition that Anthropic cannot accept. We synthesize a
    // minimal JSON schema with a single `raw` string field, set
    // `eager_input_streaming: true` so the body streams without
    // buffering, and embed the original grammar/format hints into
    // the description so Claude knows what to emit.
    let tools = translate(vec![json!({
        "type": "custom",
        "name": "apply_patch",
        "description": "Apply a unified diff to the workspace.",
        "format": {
            "syntax": "lark",
            "definition": "...lark grammar...",
        },
    })]);
    assert_eq!(tools.len(), 1);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function");
    };
    assert_eq!(t.name, "apply_patch");
    assert!(
        t.eager_input_streaming,
        "freeform tools must eagerly stream"
    );
    assert_eq!(
        t.input_schema,
        json!({
            "type": "object",
            "properties": {
                "raw": {"type": "string"},
            },
            "required": ["raw"],
            "additionalProperties": false,
        }),
    );
    // The grammar hint is appended to the description.
    assert!(
        t.description.contains("Apply a unified diff"),
        "preserved original description: {:?}",
        t.description,
    );
    assert!(
        t.description.contains("...lark grammar..."),
        "grammar hint embedded: {:?}",
        t.description,
    );
}

#[test]
fn custom_tool_without_format_synthesizes_schema_anyway() {
    // Some custom tools may not include a format field; we still
    // synthesize the raw-string schema and keep them callable.
    let tools = translate(vec![json!({
        "type": "custom",
        "name": "exec_command",
        "description": "Run a command.",
    })]);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function");
    };
    assert_eq!(t.name, "exec_command");
    assert!(t.eager_input_streaming);
}

// ---------------------------------------------------------------------------
// `local_shell` tool
// ---------------------------------------------------------------------------

#[test]
fn local_shell_tool_translated_to_function_with_known_schema() {
    // Codex's hosted `local_shell` tool is just a name/handle; Codex
    // executes it client-side. We surface it to Anthropic as a
    // regular function tool with the `local_shell` action schema so
    // Claude can call it.
    let tools = translate(vec![json!({"type": "local_shell"})]);
    assert_eq!(tools.len(), 1);
    let Tool::Function(t) = &tools[0] else {
        panic!("expected function");
    };
    assert_eq!(t.name, "local_shell");
    assert!(!t.description.is_empty(), "description must be non-empty");
    // Input schema must accept a `command` array (Codex's actual
    // exec_command shape) — minimal but strict.
    let props = t
        .input_schema
        .get("properties")
        .expect("schema has properties");
    assert!(props.get("command").is_some(), "schema declares command");
}

// ---------------------------------------------------------------------------
// `web_search` tool → Anthropic hosted server tool
// ---------------------------------------------------------------------------

#[test]
fn web_search_tool_translated_to_anthropic_hosted_web_search() {
    let tools = translate(vec![json!({
        "type": "web_search",
    })]);
    assert_eq!(tools.len(), 1);
    let Tool::WebSearch(_) = &tools[0] else {
        panic!("expected WebSearch variant");
    };
    // Round-tripping through serialization confirms the version
    // literal anthroproxy will see.
    let value = serde_json::to_value(&tools[0]).unwrap();
    assert_eq!(value["type"], json!(WEB_SEARCH_TOOL_TYPE));
    assert_eq!(value["name"], json!("web_search"));
}

#[test]
fn web_search_tool_passes_through_user_location_and_filters() {
    let tools = translate(vec![json!({
        "type": "web_search",
        "user_location": {
            "type": "approximate",
            "country": "US",
            "city": "San Francisco",
        },
        "filters": {"allowed_domains": ["docs.anthropic.com"]},
    })]);
    let Tool::WebSearch(t) = &tools[0] else {
        panic!("expected web_search");
    };
    assert_eq!(
        t.allowed_domains.as_deref(),
        Some(&["docs.anthropic.com".to_string()][..])
    );
    let location = t.user_location.as_ref().expect("user_location present");
    assert_eq!(location.country.as_deref(), Some("US"));
    assert_eq!(location.city.as_deref(), Some("San Francisco"));
}

// ---------------------------------------------------------------------------
// Unsupported / dropped tools
// ---------------------------------------------------------------------------

#[test]
fn image_generation_tool_is_dropped() {
    let tools = translate(vec![json!({
        "type": "image_generation",
        "output_format": "png",
    })]);
    assert!(
        tools.is_empty(),
        "image_generation has no Vertex equivalent"
    );
}

#[test]
fn tool_search_tool_is_dropped() {
    let tools = translate(vec![json!({
        "type": "tool_search",
        "execution": "...",
        "description": "...",
        "parameters": {"type": "object"},
    })]);
    assert!(tools.is_empty(), "tool_search not in our scope");
}

#[test]
fn namespace_tool_is_dropped_for_now() {
    // Codex's namespace tool wraps a list of children. Translator
    // could flatten with prefix; for v1 we drop with a known TODO.
    let tools = translate(vec![json!({
        "type": "namespace",
        "name": "github",
        "tools": [],
    })]);
    assert!(tools.is_empty());
}

#[test]
fn unknown_tool_type_is_dropped_silently() {
    let tools = translate(vec![json!({
        "type": "future_tool_type",
        "name": "x",
    })]);
    assert!(tools.is_empty());
}

// ---------------------------------------------------------------------------
// Mixed translation
// ---------------------------------------------------------------------------

#[test]
fn mixed_tool_array_preserves_order_and_drops_unsupported() {
    let tools = translate(vec![
        json!({"type": "function", "name": "shell", "parameters": {"type": "object"}}),
        json!({"type": "image_generation", "output_format": "png"}),
        json!({"type": "custom", "name": "apply_patch"}),
        json!({"type": "web_search"}),
        json!({"type": "local_shell"}),
    ]);
    assert_eq!(tools.len(), 4, "image_generation dropped, others kept");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| match tool {
            Tool::Function(FunctionTool { name, .. }) => name.as_str(),
            Tool::WebSearch(WebSearchTool { .. }) => "web_search",
        })
        .collect();
    assert_eq!(
        names,
        vec!["shell", "apply_patch", "web_search", "local_shell"]
    );
}
