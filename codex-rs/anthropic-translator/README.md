# codex-anthropic-translator

My pure-Rust translator that exposes the **OpenAI Responses API** on
the inbound side and speaks the **Anthropic Messages API** on the
outbound side, so the unmodified Codex CLI (which only knows how to
speak Responses) can drive Claude through an Anthropic-shaped proxy.

This is the codexmaxxxing internal notebook for the crate — the
thing I read three months from now when I've forgotten why
`apply_patch` is a function tool with a `raw: string` schema, or why
`parse_output` dispatches on JSON value type instead of a struct, or
which Vertex limits we're working around. Read it end-to-end before
making changes.

---

## Why this crate exists

The Codex CLI is hard-locked to one wire format: the OpenAI Responses
API. See `codex-rs/model-provider-info/src/lib.rs:51` — `WireApi` only
declares `Responses`, and the older `chat` variant was explicitly
removed with a 400-equivalent error. There is no plumbing in Codex to
talk Anthropic Messages directly.

I want to drive `claude-opus-4-7` (and other Claude models) from Codex
without forking the Codex CLI itself. The translator is the bridge:
Codex POSTs to the translator's `/v1/responses` exactly as if it were
OpenAI; the translator emits the equivalent `POST /v1/messages` to a
downstream proxy called **anthroproxy**.

### What anthroproxy is

Anthroproxy is a local HTTP server (running on the user's machine,
default `127.0.0.1:6969`) that:

- Presents the standard Anthropic Messages API at a different base URL.
- Internally routes to **Anthropic-on-Vertex** (Claude models served
  through Google Cloud Vertex AI), handling GCP auth, the Vertex URL
  rewrite, the body-level `anthropic_version: "vertex-2023-10-16"`
  field, and the `:streamRawPredict` endpoint suffix.

From the translator's perspective, anthroproxy *is* the Anthropic API.
We never speak Vertex directly. **However, every feature we emit must
be supported on Anthropic-on-Vertex per the
[features overview](https://docs.anthropic.com/en/docs/build-with-claude/overview).**
This is the "Vertex compatibility floor" mentioned throughout the code.

### Architecture

```text
   codex CLI ── POST /v1/responses (OpenAI, SSE) ──► translator (this crate)
                                                          │
         ┌────────────────────────────────────────────────┘
         │  POST /v1/messages (Anthropic, SSE)
         ▼
     anthroproxy (port 6969)
         │
         ▼
     Anthropic-on-Vertex (api.anthropic.com shape, GCP transport)
```

The translator is a single axum HTTP server with one route
(`POST /v1/responses`). It does:

1. Deserialize the incoming Codex request body.
2. Translate it into an Anthropic `MessageRequest` (per-model rules,
   tool reshaping, cache plan, thinking config, structured output).
3. POST upstream to anthroproxy with optional `anthropic-beta` header.
4. Stream the SSE response back through a stateful translator that
   emits OpenAI Responses-shape SSE events Codex can parse.

---

## Vertex compatibility floor (read before adding features)

Every type and field in this crate is supported on Vertex AI per the
features overview (verified 2026-05-12). The translator deliberately
**does not** model features that aren't at least beta on Vertex:

| Anthropic feature | Vertex AI status | In this crate? |
|---|---|---|
| Adaptive thinking, effort, extended thinking | GA | ✓ |
| Manual extended thinking (`enabled` mode) | GA | ✓ (older models) |
| Structured outputs (`output_config.format`), strict tool use | GA | ✓ |
| Fine-grained tool streaming (`eager_input_streaming`) | GA | ✓ |
| Prompt caching 5m + 1h | GA | ✓ |
| Web search server tool (`web_search_20250305`) | GA on Vertex (basic only) | ✓ — newer `web_search_20260209` (dynamic filtering) is API-only and rejected by Vertex |
| Compaction, context editing | Beta on Vertex | Type system ready; opt in via `--beta` |
| **Automatic prompt caching** | ❌ Not on Vertex | ❌ — manual cache planner is the only path |
| Web fetch, code execution, MCP toolset, Files API, Agent Skills, programmatic tool calling | ❌ Not on Vertex | ❌ — out of scope |

When adding new request fields or content-block variants, **first**
check the Anthropic features overview and confirm Vertex support
before writing the type. Don't add speculative features that the
backend will reject.

---

## Onboarding map (in execution order)

When I (or anyone reading) come back to this cold, this is the order
to read in:

1. This README, top-to-bottom.
2. `tests/anthropic_request.rs` — what we send to Anthropic.
3. `tests/anthropic_event.rs` — what we receive over SSE.
4. `tests/openai_request.rs` — what Codex sends us.
5. `tests/openai_response_events.rs` — what we send back to Codex.
6. `tests/translate_request.rs` — end-to-end request translation
   contract (covers per-model behavior, cache placement, structured
   output).
7. `tests/translate_stream.rs` — end-to-end stream translation
   contract (covers text/thinking/tool_use, custom-tool streaming,
   web search results, cumulative usage, local_shell round-trip,
   redacted thinking, citation deltas).
8. `tests/server_integration.rs` — wiremock-backed end-to-end through
   the actual axum server.
9. Then read the `src/` modules in matching order.

Tests are the contract. If I change behavior, I change the test first
(see TDD discipline below — this is non-negotiable, and I learned
why the hard way; see "May 2026 hardening pass" below).

---

## May 2026 hardening pass

The first cut of this crate worked end-to-end on a single canonical
turn but had a class of bugs in common: types modeled against a
made-up wire shape, with `serde_json::from_value(..).unwrap_or(default)`
silently swallowing the mismatch, and tests that round-tripped against
the made-up shape so the bug was invisible to the suite. Symptoms
ranged from "shell tool returns nothing on every call" (because every
`function_call_output` deserialized to the empty-string fallback) to
"emoji and CJK supplementary characters silently disappear from
`apply_patch` payloads" (because the JSON `\uXXXX` decoder rejected
unpaired surrogates instead of pairing them).

A four-tier audit corrected all of it. The fixes worth knowing:

**Inbound from Codex** — every typed shape is now pinned against
either `protocol/src/models.rs` or the OpenAI Responses API reference,
not against a guess. Specifically:

- `function_call_output.output` and `custom_tool_call_output.output`
  are decoded as the documented `string | array of input_text /
  input_image content items` union rather than as the fictional
  `{content, success}` object. The struct-shaped fallback never
  appeared on the wire — `success` is internal metadata in
  `FunctionCallOutputPayload` and is intentionally not serialized.
- `ReasoningEffort` covers the full Codex enum (`none`, `minimal`,
  `low`, `medium`, `high`, `xhigh`). Any missing variant would crash
  the entire request deserialization since the enum has no
  `#[serde(other)]`. Translation maps `none` and `minimal` down to
  Anthropic `low`, `xhigh` to `xhigh` on Opus 4.7 (or `high` on
  models that don't support it).
- `ReasoningContentItem` covers both `reasoning_text` (legacy) and
  `text` (current) shapes per Codex's protocol.
- `FunctionCall.namespace`, `ContentItem::InputImage.detail`, and
  `Message.phase` are deserialized explicitly even though Anthropic
  has no equivalent and the translator drops them on the way out —
  this guards against future workspace-wide deny rules and makes the
  intent visible at the dropping site.

**Outbound to Codex** — `local_shell` round-trip is the headline.
Codex's `LocalShellHandler` crashes the turn with
`FunctionCallError::Fatal("LocalShellHandler expected ToolPayload::LocalShell")`
if it receives a generic `function_call`. The stream translator now
detects `name == "local_shell"` at `content_block_start` and emits
`OutputItem::LocalShellCall` (a new variant with the documented
`{type, id, call_id, status, action}` shape) for both
`output_item.added` and `output_item.done`, reshaping the streamed
JSON into a `LocalShellAction::Exec` value at stop. `RedactedThinking`
content blocks (the safety-redacted thinking shape) are now modeled
on the inbound event side and routed through Codex as
`Reasoning { encrypted_content: Some(data) }` so the next turn's
thinking-block validation succeeds.

**Inbound from Anthropic** — `ContentBlockDelta` gained
`CitationDelta` (web-search citations on text blocks; previously
killed the entire `StreamEvent` decode), the event-side `ContentBlock`
gained `RedactedThinking { data }` (same failure mode), and
`MessageDelta.usage` is now `Option<Usage>` so a partial delta event
from any future API rev doesn't crash the stream.

**Apply_patch streaming correctness** — `RawStringExtractor` was a
toy state machine that (a) called `char::from_u32` on each `\uXXXX`
escape, dropping every emoji / CJK supplementary char (UTF-16
surrogate pairs round-trip as `\uD83D\uDE00`-style pairs per RFC 8259
§7), and (b) extracted the value of the *first* JSON string after the
first colon, which silently dropped the actual patch body whenever
the model emitted a leading rationale field
(`{"explanation": "...", "raw": "..."}`). It's now a real streaming
JSON tokenizer: tracks top-level key matching, depth, string escapes,
and pairs UTF-16 surrogates across chunk boundaries. Lone surrogates
are dropped per spec without corrupting the rest of the stream.

This pass added 36 tests (213 → 249), all green.

---

## Key design decisions (and why)

These are the non-obvious calls. If you find yourself wanting to undo
one, re-read the linked docs first.

### 1. Translator is a separate process, not embedded in Codex

The user's primary goal was to drive Claude from Codex without forking
the Codex CLI source. This crate is a separate binary. The
`scripts/cmxcdx` wrapper auto-spawns it on demand so the user only
types one command. If/when in-process embedding is wanted, the
`server::serve` function is already designed to be called from inside
Codex's tokio runtime — but every embedded path increases the diff
against upstream Codex, which we want to keep at zero.

### 2. Manual cache planner, not Anthropic's automatic caching

Anthropic shipped automatic prompt caching (single top-level
`cache_control` field that auto-advances). It is **not available on
Vertex AI** per the features overview. Our `cache_state.rs` planner
emits up to 4 explicit `cache_control: ephemeral` breakpoints in a
deterministic positional layout (system / tools / two message-tail
markers). This is more code, but it's the only path that works on
Vertex.

### 3. Adaptive-only thinking on Opus 4.7

Opus 4.7 returns HTTP 400 on manual `thinking: {type: "enabled",
budget_tokens: N}`. The model *only* accepts `{type: "adaptive"}`.
`translate_thinking::translate_thinking` enforces this through the
`ThinkingMode::AdaptiveOnly` rule in `model_spec.rs`.

### 4. Codex `effort = high` → Anthropic `xhigh` on Opus 4.7

Per the Anthropic effort doc: *"Start with `xhigh` for coding and
agentic use cases"* on Opus 4.7. The user's workload (Codex) is
exactly that. We promote Codex's `high` to Anthropic `xhigh` only on
Opus 4.7 (where `xhigh` is supported); on other models `high` stays
`high`.

### 5. `display: "summarized"` set explicitly on every Opus 4.7 request

Opus 4.7's default is `display: "omitted"` (silent change from 4.6,
documented). Without an explicit override the Codex TUI would never
show reasoning. We always send `summarized` unless the user opted out
via `model_reasoning_summary = "none"` (mapped to `omitted`).

### 6. Apply_patch / exec_command synthesized as `eager_input_streaming` function tools

Codex emits these as `type: "custom"` tools with a Lark grammar
definition. Anthropic does not accept Lark grammars at all. We
synthesize a JSON schema with a single `raw: string` field, embed the
original grammar text in the tool description as a hint to Claude,
and set `eager_input_streaming: true` so the body streams without JSON
validation buffering. The stream side knows these tools are "custom"
(via the set passed to `StreamTranslator::new`) and routes their
tool_use deltas through the streaming raw-string extractor → real
`response.custom_tool_call_input.delta` events.

### 7. Web search results surface as synthetic assistant text

Anthropic's hosted web_search returns `web_search_tool_result` content
blocks with structured citations. Codex has no protocol-level concept
of server tools or hosted web search results. Rather than drop them,
we format them as assistant text (🔎 prefix for the call, bulleted
citations for results, ⚠ prefix for errors). This makes them visible
in Codex's TUI without requiring protocol changes.

### 8. `RawStringExtractor` is its own module with its own tests

Streaming JSON-string extraction with chunk-boundary handling
(boundaries on `:`, lone `\`, mid-`\uXXXX`) is fiddly. Isolating it
in `translate/raw_string_extractor.rs` with its own focused unit tests
keeps the stream translator readable and lets us TDD edge cases like
"chunk ends after `:`" without spinning up the full stream pipeline.

### 9. Forward-compat unknowns route to dedicated variants

Stop reasons, error kinds, and `ResponseItem` variants we don't
recognize all land in catch-all `Unknown` / `Unrecognized` arms with
`#[serde(other)]`. This means an Anthropic API addition (new stop
reason, new error type, new content block) can't break our parser
mid-stream. New variants surface explicitly when we add named arms.

### 10. Beta features are opt-in via `--beta` flag, not always-on

Even though some beta features (compaction, context editing) would be
nice-to-have, sending the `anthropic-beta` header unconditionally
risks unexpected behavior changes when Anthropic graduates a feature
or changes its semantics. Users explicitly opt in.

---

## Per-model routing

| Model | `max_tokens` default | Thinking mode | Effort range |
|---|---|---|---|
| `claude-opus-4-7` | 128_000 | Adaptive ONLY (manual rejected with 400) | low/medium/high/xhigh/max — codex `high` → `xhigh` |
| `claude-opus-4-6` | 128_000 | Adaptive (recommended) | low/medium/high/max |
| `claude-sonnet-4-6` | 64_000 | Adaptive (recommended) | low/medium/high/max |
| `claude-opus-4-5` | 64_000 | Manual (`enabled` + budget) | low/medium/high |
| `claude-sonnet-4-5` | 64_000 | Manual | none (no effort param) |
| `claude-haiku-4-5` | 64_000 | Manual | none |
| Unknown Claude prefix | 4096 (safe floor) | Manual | none |
| Non-Claude model | **400 `unsupported_model`** — translator refuses |

`@<date>` snapshots used by Vertex (`claude-opus-4-7@20260101`) are
stripped before the prefix match. Adding a new model means: add a
`ModelFamily` variant + a row in `model_spec.rs` + a regression test
in `tests/translate_model_spec.rs`.

---

## Translation cheat sheet

### Codex `reasoning` → Anthropic

Codex sends `reasoning: {effort, summary}`. Translator splits this:

- **`effort`** → `output_config.effort` (a *standalone* top-level
  Anthropic field, NOT inside `thinking`). Codex `minimal`/`low` →
  `low`, `medium` → `medium`, `high` → `xhigh` (Opus 4.7) or `high`
  (others). Models without effort support omit the field.
- **`summary`** → `thinking.display`. `none` → `omitted`, everything
  else (`auto`/`concise`/`detailed`) → `summarized`.

### Codex `tools[]` → Anthropic `tools[]`

| Codex tool `type` | Anthropic shape | Notes |
|---|---|---|
| `function` | `{name, description, input_schema, strict?, cache_control?}` | Direct; `parameters` → `input_schema`. |
| `custom` (apply_patch / exec_command) | Function tool with synthesized `{raw: string}` schema, `eager_input_streaming: true`, grammar hint embedded in description | Anthropic doesn't accept Lark; raw-string envelope is the only safe round-trip. |
| `local_shell` | Function tool with `LocalShellAction` schema (`type: "exec"`, `command: [string]`) | Codex executes locally; we expose it as a regular function tool. |
| `web_search` | Anthropic hosted `web_search_20250305` server tool | Filters and `user_location` pass through. Vertex doesn't accept the newer 20260209 dynamic-filtering shape. |
| `image_generation`, `tool_search`, `namespace`, future variants | Dropped silently | No Vertex-supported equivalent. |

### Codex `text.format` → Anthropic `output_config.format`

`text.format = {type: "json_schema", schema, strict, name}` becomes
`output_config.format = {type: "json_schema", schema}`. Verbosity is
dropped (no Anthropic equivalent). NO forced tool-call workaround;
structured outputs are GA on Vertex.

### Codex `prompt_cache_key` → manual cache plan

`prompt_cache_key` is a Codex-side hint. We don't pass it through —
instead the cache planner emits up to 4 explicit `cache_control:
ephemeral` breakpoints positionally:

1. **System** — last block of the `system` array. Pinned.
2. **Tools** — last entry of the `tools` array. Pinned.
3. **Message tail** — last content blocks of the most recent completed
   assistant turns, newest first, until the 4-slot cap is hit.

Thinking and redacted_thinking blocks are skipped when seeking the
message-tail attachment site (Anthropic forbids `cache_control` on
those).

### Codex `client_metadata.x-codex-installation-id` → `metadata.user_id`

Anthropic's `metadata` only accepts a single `user_id` string. We
extract the Codex installation ID (the only metadata Codex sends) and
emit it. All other metadata keys are dropped.

### Codex fields dropped silently (no Anthropic equivalent)

- `store: true`
- `service_tier`
- `parallel_tool_calls`
- `include`
- `text.verbosity`
- `previous_response_id`

### Anthropic stream events → Codex SSE

| Anthropic event | Codex output | Notes |
|---|---|---|
| `message_start` | `response.created` | Stash id + model. |
| `content_block_start` (text) | `response.output_item.added` (`AssistantMessage`) | New `output_index` assigned. |
| `content_block_start` (thinking) | `response.output_item.added` (`Reasoning`) + `response.reasoning_summary_part.added` | Empty thinking + signature initially. |
| `content_block_start` (tool_use) | `response.output_item.added` (`FunctionCall` or `CustomToolCall` per the custom-tool set) | Custom routing requires the set passed to `StreamTranslator::new`. |
| `content_block_start` (server_tool_use, web_search) | Synthetic assistant text "🔎 Web search: <query>" | Surfaced inline. |
| `content_block_start` (web_search_tool_result) | Synthetic assistant text with formatted citations | Or "⚠ Web search error: <code>" on failure. |
| `content_block_delta` (text_delta) | `response.output_text.delta` | Accumulates. |
| `content_block_delta` (thinking_delta) | `response.reasoning_summary_text.delta` (summary_index=0) | Buffers. |
| `content_block_delta` (signature_delta) | (buffered, no event) | Attached to closing Reasoning OutputItemDone. |
| `content_block_delta` (input_json_delta, custom tool) | `response.custom_tool_call_input.delta` per chunk | Via `RawStringExtractor` — true incremental. |
| `content_block_delta` (input_json_delta, function tool) | (buffered, no event) | Anthropic and OpenAI both emit args at done. |
| `content_block_stop` | `response.output_item.done` | Final item shape per kind. |
| `message_delta` | (buffered) | Stop reason → `end_turn` mapping; cumulative usage. |
| `message_stop` | `response.completed` | With usage + end_turn. |
| `error` | `response.failed` | With error type + message. |
| `ping` | (none) | Silently consumed. |

### Stop reason → `end_turn`

| Anthropic stop_reason | end_turn |
|---|---|
| `end_turn`, `max_tokens`, `stop_sequence`, `refusal`, `unknown` | `true` |
| `tool_use`, `pause_turn` | `false` (more turns coming) |

### Cumulative usage → Codex usage

Anthropic's `message_delta.usage` is cumulative. We merge across
events (latest non-zero wins) and on `message_stop` emit:

- `input_tokens` ← Anthropic `input_tokens`
- `output_tokens` ← Anthropic `output_tokens` (latest cumulative)
- `total_tokens` ← sum
- `input_tokens_details.cached_tokens` ← Anthropic
  `cache_read_input_tokens` (when > 0)
- `output_tokens_details.reasoning_tokens` ← 0 (Anthropic doesn't
  break this out per-block in our current scope)

### Custom tool input — true incremental streaming

Anthropic streams `input_json_delta` chunks like
`{"r`, `aw":"hel`, `lo\\n`, `wor`, `ld"}`. The translator runs each
chunk through `RawStringExtractor` — a stateful streaming parser that
emits the decoded contents of the `raw` field as soon as the bytes
are committed (decoding `\n`/`\t`/`\\`/`\"`/`\uXXXX` escapes, handling
chunk boundaries that land on `:`, lone `\`, or mid-`\uXXXX`). Codex
receives real `response.custom_tool_call_input.delta` events as the
patch body arrives, character by character.

### Reasoning round-trip

Anthropic `thinking` blocks include an opaque `signature`. The
translator buffers it across `signature_delta` events and attaches it
as `encrypted_content` on the closing `OutputItemDone(Reasoning)`.
Codex re-sends the reasoning item on the next turn via its
`include: ["reasoning.encrypted_content"]` request; the request
translator unwraps `encrypted_content` back into the Anthropic
`thinking.signature`. **Without this round-trip, Anthropic 400s on
subsequent thinking-with-tools turns.**

---

## Beta features

Pass beta-feature identifiers as a comma-separated list to the
`--beta` flag (or `TRANSLATOR_BETA` env var). They join into the
`anthropic-beta` header on every upstream request. Anthroproxy
forwards the header to Vertex unchanged.

```bash
codex-anthropic-translator --beta context-management-2025-06-27,interleaved-thinking-2025-05-14
TRANSLATOR_BETA="context-management-2025-06-27" codex-anthropic-translator
```

Empty list → no header sent.

Currently-relevant betas on Vertex:

- `context-management-2025-06-27` — context editing (auto-clearing of
  tool results when approaching window limits)
- `interleaved-thinking-2025-05-14` — explicitly enabling thinking
  between tool calls on Sonnet 4.6 manual mode (already auto-enabled
  on Opus 4.7 + adaptive)

Check the [Anthropic features overview](https://docs.anthropic.com/en/docs/build-with-claude/overview)
for the current beta header strings.

---

## Quick start (standalone)

```bash
# Build the binary.
cargo build --release -p codex-anthropic-translator

# Run pointing at anthroproxy.
./target/release/codex-anthropic-translator \
    --listen 127.0.0.1:7070 \
    --upstream http://127.0.0.1:6969

# In ~/.codex/config.toml:
#   [model_providers.anthroproxy]
#   name = "anthroproxy"
#   base_url = "http://127.0.0.1:7070/v1"
#   wire_api = "responses"
#
#   [profiles.opus]
#   model_provider = "anthroproxy"
#   model = "claude-opus-4-7"
#   model_reasoning_effort = "high"
#   model_reasoning_summary = "auto"

# Then:
codex -p opus
```

Or use the `cmxcdx` wrapper that auto-starts the translator —
see [`CMXCDX.md`](../../CMXCDX.md) at the repo root for the
end-user-facing guide.

## CLI reference

```text
codex-anthropic-translator [--listen <ADDR>] [--upstream <URL>] [--beta <FEATURE>...]
```

| Flag | Env var | Default | Notes |
|---|---|---|---|
| `--listen` | `TRANSLATOR_LISTEN` | `127.0.0.1:7070` | Bind address. |
| `--upstream` | `TRANSLATOR_UPSTREAM` | `http://127.0.0.1:6969` | Anthroproxy base URL. The translator appends `/v1/messages`. |
| `--beta` | `TRANSLATOR_BETA` | (none) | Repeat for multiple, or pass comma-separated. |

`RUST_LOG=codex_anthropic_translator=debug` enables tracing.

---

## Project layout

```
src/
├── lib.rs                          # private modules + explicit re-exports per AGENTS.md
├── main.rs                         # axum binary (clap CLI)
├── cache_state.rs                  # CachePlan::compute (4 explicit breakpoints)
├── server.rs                       # axum POST /v1/responses → upstream → SSE pipe
├── anthropic/
│   ├── mod.rs                      # re-exports
│   ├── request.rs                  # MessageRequest + everything we serialize
│   └── event.rs                    # StreamEvent + everything we deserialize
├── openai/
│   ├── mod.rs                      # re-exports
│   ├── request.rs                  # ResponsesRequest + ResponseItem (Codex shape)
│   └── response_events.rs          # ResponseStreamEvent (what Codex consumes)
└── translate/
    ├── mod.rs
    ├── model_spec.rs               # per-model rules (max_tokens, thinking, effort)
    ├── thinking.rs                 # codex.reasoning → thinking + effort
    ├── tools.rs                    # function/custom/local_shell/web_search xlation
    ├── messages.rs                 # ResponseItem array → Anthropic messages
    ├── cache.rs                    # apply CachePlan to MessageRequest
    ├── request.rs                  # top-level translate_request
    ├── stream.rs                   # StreamTranslator state machine
    └── raw_string_extractor.rs     # streaming JSON-string extractor + unit tests

tests/
├── cache_state.rs                  (13) breakpoint planner
├── anthropic_request.rs            (32) outgoing Anthropic Messages wire shape
├── anthropic_event.rs              (30) incoming Anthropic SSE deserialization
├── openai_request.rs               (30) Codex ResponsesRequest + ResponseItem
├── openai_response_events.rs       (13) outgoing Codex SSE wire shape
├── translate_model_spec.rs          (8) per-model rule table
├── translate_thinking.rs           (19) reasoning → thinking + effort, per model
├── translate_tools.rs              (14) tool array translation
├── translate_messages.rs           (21) ResponseItem[] → Anthropic messages[]
├── translate_cache.rs              (10) applying CachePlan
├── translate_request.rs            (15) end-to-end request translation
├── translate_stream.rs             (16) SSE event-by-event translation
└── server_integration.rs            (5) wiremock end-to-end through axum
```

Module sizes target <500 LoC each per the workspace AGENTS.md guidance.
Tests inline (in `src/translate/raw_string_extractor.rs`) are fine for
focused unit testing of internal helpers; integration tests in
`tests/` pin the public crate API.

---

## Development conventions

These are non-negotiable for this crate:

### TDD: tests first, always

Non-negotiable on this crate. The May 2026 hardening pass is the
empirical receipt: a class of bugs hid behind tests that round-tripped
against fictional shapes, and the test suite was happily green for
months while shell tool calls returned empty in production. The
workflow is:

1. Write the test that describes the new behavior (in
   `tests/<area>.rs` or as an inline `#[cfg(test)] mod tests` block),
   and pin it against the *real* source-of-truth — the Anthropic doc
   URL or the Codex `protocol/` types, not against a guess.
2. Run it; confirm it fails for the expected reason.
3. Write the minimal implementation to make it pass.
4. `cargo fmt -p codex-anthropic-translator` (or via the workspace
   recipe) and `cargo clippy -p codex-anthropic-translator --tests
   --all-targets -- -D warnings` clean.
5. Run the full crate's test suite.

If I find myself writing implementation first, stop. Re-read this
section.

### Read the docs before designing types

Anthropic ships fast, but Vertex lags. The wire format changes
meaningfully between quarterly releases (e.g. `output_config.format`
superseded the beta `output_format` field), and Anthropic's
"current" version on the direct API is often **not** what Vertex
accepts. A real example: as of writing, Anthropic-direct lists
`web_search_20260209` (with dynamic filtering) as current, but
Vertex rejects it and only accepts `web_search_20250305` — see
`WEB_SEARCH_TOOL_TYPE` and the test pinning that shape. Before
adding a new request field or stream event:

1. Fetch the relevant doc page from <https://docs.anthropic.com/>.
2. Cross-reference the Vertex compatibility floor in the features
   overview.
3. Add the test pinning the wire shape.
4. Cite the doc URL in the test's module docstring.

The cost of guessing wrong is hours of debugging in production. The
existing tests' module docstrings include doc URLs as the source of
truth for each pinned shape.

### Workspace lint rules (`codex-rs/Cargo.toml:431-466`)

The whole workspace denies a long list of clippy lints:
`unwrap_used`, `expect_used`, `redundant_clone`, `redundant_closure`,
`uninlined_format_args`, `manual_*`, `needless_*`, `trivially_copy_pass_by_ref`,
etc. `clippy.toml` allows `unwrap`/`expect` *only inside `#[test]`*
functions — module-level test helpers must use explicit `match`
returning a `panic!` instead.

The single workspace-rule exception in this crate is the
`#[allow(clippy::trivially_copy_pass_by_ref)]` on `is_false` in
`anthropic/request.rs` — required because serde's `skip_serializing_if`
demands a `fn(&T) -> bool` signature even for `bool`. Documented
inline.

### Module visibility

Per AGENTS.md: prefer private modules with explicit `pub use`
re-exports. The translate/ submodules are all `mod foo` (private)
with named re-exports in `translate/mod.rs`. Same pattern for
`anthropic/` and `openai/`. Don't `pub mod foo` unless tests need to
access nested submodule names directly (`anthropic::event` is the one
exception, because `event::ContentBlock` and `request::ContentBlock`
have legitimately different shapes).

### Module size cap

AGENTS.md targets <500 LoC per module excluding tests, hard cap ~800.
If a module is approaching that, split it before adding new code. The
translate/ module split was done preemptively for this reason.

---

## Tests (249 total, all green)

```bash
cargo test -p codex-anthropic-translator
cargo clippy -p codex-anthropic-translator --tests --all-targets -- -D warnings
cargo fmt -p codex-anthropic-translator -- --config imports_granularity=Item
```

The `imports_granularity=Item` config requires nightly; the workspace
recipe pipes that warning to /dev/null. Imports in this crate are
already one-per-line so the rule changes nothing in practice.

| Test file | What it pins |
|---|---|
| `tests/cache_state.rs` (13) | Breakpoint planner: order, cap, slot allocation. |
| `tests/anthropic_request.rs` (32) | Outgoing Anthropic Messages wire shape per the latest API docs. |
| `tests/anthropic_event.rs` (30) | Incoming Anthropic SSE event deserialization (incl. `redacted_thinking`, `citation_delta`, optional `MessageDelta.usage`, forward-compat catch-alls). |
| `tests/openai_request.rs` (30) | Codex `ResponsesRequest` + `ResponseItem` deserialization (incl. `ReasoningEffort` full enum, `ReasoningContentItem::Text`, `FunctionCall.namespace`, `InputImage.detail`, `Message.phase`). |
| `tests/openai_response_events.rs` (13) | Outgoing Codex SSE event wire shape. |
| `tests/translate_model_spec.rs` (8) | Per-model rule table. |
| `tests/translate_thinking.rs` (19) | Codex reasoning → Anthropic thinking + effort, per model (incl. `XHigh` and `None` mapping, manual budget tiers). |
| `tests/translate_tools.rs` (14) | Tool array translation (function/custom/local_shell/web_search). |
| `tests/translate_messages.rs` (21) | `ResponseItem[]` → Anthropic `messages[]` with role grouping; `function_call_output` real wire shape (string + content-items array + image passthrough). |
| `tests/translate_cache.rs` (10) | Applying `CachePlan` to a built `MessageRequest`. |
| `tests/translate_request.rs` (15) | End-to-end request translation per model + cache. |
| `tests/translate_stream.rs` (16) | SSE event-by-event translation incl. web search, custom-tool streaming, `local_shell` → `LocalShellCall` round-trip, `redacted_thinking` → `Reasoning`, `citation_delta` silently consumed. |
| `tests/server_integration.rs` (5) | Wiremock end-to-end: real HTTP, real upstream mock, real SSE pump. |
| `src/translate/raw_string_extractor.rs::tests` (23) | Chunk-boundary edge cases; UTF-16 surrogate pairs (split across chunk boundaries, lone surrogates dropped); key-aware extraction (only the value of the literal `"raw"` top-level key, regardless of position; nested `"raw"` keys ignored). |

---

## Known gaps and future work

Things deliberately left for follow-up. Each entry includes "what to
do" so I can pick one up cold.

### Compaction + context editing (beta) not auto-enabled

The translator accepts `--beta` flags but doesn't auto-detect when
compaction would help. For long sessions where the request payload
approaches the 30 MB Vertex limit, I have to opt in manually with
`--beta context-management-2025-06-27`. **What to do:** add a config
knob that auto-enables context editing when the request body exceeds
a threshold (say, 80% of 30 MB).

### Reasoning content blocks not modeled

We emit `ResponseItem::Reasoning { encrypted_content: Some(sig) }`
with empty `summary` because Anthropic's `display: "summarized"`
streams text via `thinking_delta` (which we route to
`response.reasoning_summary_text.delta`). The detailed reasoning
content (when Anthropic exposes the full chain in future models)
would route through `reasoning_text.delta`. **What to do:** when
Anthropic adds an Opus model that exposes full reasoning text rather
than summaries, plumb the `ReasoningTextDelta` variant which is
already declared on `ResponseStreamEvent`.

### Function-call arguments don't stream incrementally

Codex's CLI parser only handles `response.custom_tool_call_input.delta`
(per `codex-api/src/sse/responses.rs:314`); regular function tool
arguments arrive as one chunk in `OutputItemDone`. This matches the
OpenAI Responses API itself but means slow-thinking model thinking
shows up only at block_stop. **What to do:** if Codex adds a
`response.tool_call_arguments.delta` parser, plumb it here. Until
then, no action needed.

### Server-tool-use input not surfaced

When Claude calls hosted web search, we surface the *call* as a
synthetic assistant text "🔎 Web search: <query>" but don't stream the
query incrementally as it's being built. **What to do:** route the
`server_tool_use` block's `input_json_delta` through the
`RawStringExtractor` (with key `"query"` instead of `"raw"`) and
emit text deltas.

### No retries on upstream 5xx / network failure

The server returns 502 to Codex on any upstream connection error. For
production, you'd want bounded retries with exponential backoff (the
codex-api crate has utilities for this; we don't reach for them yet).
**What to do:** wrap the `state.http.post(...).send()` call in a
retry helper from `reqwest_retry` or implement a small one.

### `cmxcdx` wrapper assumes `nc` is available

The wrapper script uses `nc -z` to detect whether the translator is
already listening. macOS bundles BSD `nc`; older Linux distros may
need `netcat-openbsd`. **What to do:** swap for `bash`'s `</dev/tcp/`
readiness probe (no external dep) if portability matters.

### No automatic model migration when Anthropic deprecates IDs

We hard-code `claude-opus-4-7` etc. When Anthropic ships 4.8, the
user has to update both `model_spec.rs` and their `~/.codex/config.toml`.
**What to do:** the Anthropic Models API can list available models
programmatically; the translator could fetch this on startup and
maintain a fallback table.

---

## How to extend (concrete recipes)

### Adding a new Claude model

1. Add a `ModelFamily` variant in `src/translate/model_spec.rs`.
2. Add a row to the `match` in `model_spec()` mapping the wire ID
   prefix to the new family with its `max_tokens_default`,
   `ThinkingMode`, and effort gates.
3. Add a regression test in `tests/translate_model_spec.rs`.
4. If thinking/effort behavior is unusual, add per-model behavior tests
   to `tests/translate_thinking.rs`.

### Enabling a new beta feature

The `--beta` flag already supports arbitrary identifiers — no code
change needed for header transport. If the beta unlocks a new request
field:

1. Add the field to the relevant type in `src/anthropic/request.rs`
   (or `event.rs`) with a passing serde test.
2. Add translation logic in `src/translate/request.rs` if the field
   maps from a Codex request field.
3. Document the beta string in this README's beta features section.

### Supporting a new Anthropic server tool

Follow the web_search pattern:

1. Add the tool variant in `src/anthropic/request.rs::Tool` (or
   reuse `Tool::WebSearch`-style if it's a singleton).
2. Add the corresponding inbound event handling in
   `src/anthropic/event.rs::ContentBlock`.
3. In `src/translate/tools.rs`, map from Codex's tool spec to the
   new Anthropic shape.
4. In `src/translate/stream.rs`, decide how to surface the tool's
   call/result events to Codex. If there's no Codex protocol
   equivalent, follow the synthetic-assistant-text pattern used for
   web search.

### Supporting a new Codex content item type

1. Add the variant in `src/openai/request.rs::ResponseItem`.
2. Handle it in `src/translate/messages.rs::Builder::consume`.
3. Add a regression test in `tests/translate_messages.rs`.

### Adding a wire-format check from a new Anthropic doc

1. Fetch the doc URL.
2. Add the doc URL to the test module's docstring.
3. Write the test using `serde_json::json!` to materialize the wire
   shape from the doc, then assert serialization (for outgoing types)
   or deserialization (for incoming types) matches.
4. Implement.

---

## References

Source docs that define the wire contract pinned by tests in this crate
(every assertion ties back to one of these):

- Messages API: <https://docs.anthropic.com/en/api/messages>
- Streaming: <https://docs.anthropic.com/en/docs/build-with-claude/streaming>
- Prompt caching: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>
- Adaptive thinking: <https://docs.anthropic.com/en/docs/build-with-claude/adaptive-thinking>
- Effort parameter: <https://docs.anthropic.com/en/docs/build-with-claude/effort>
- Extended thinking: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking>
- Structured outputs: <https://docs.anthropic.com/en/docs/build-with-claude/structured-outputs>
- Tool use overview: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/overview>
- Define tools: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/define-tools>
- Handle tool calls: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/handle-tool-calls>
- Tool reference: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/tool-reference>
- Fine-grained tool streaming: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/fine-grained-tool-streaming>
- Errors: <https://docs.anthropic.com/en/api/errors>
- Models overview: <https://docs.anthropic.com/en/docs/about-claude/models/overview>
- Features overview (Vertex compatibility floor): <https://docs.anthropic.com/en/docs/build-with-claude/overview>
- Claude on Vertex AI: <https://docs.anthropic.com/en/docs/build-with-claude/claude-on-vertex-ai>

When Anthropic ships a breaking change to any of these, the corresponding
test fails loudly and points at the broken contract.
