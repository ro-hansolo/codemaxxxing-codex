# cmxcdx — codemaxxxing-codex

My wrapper that lets me drive Claude (Opus 4.7 by default) from
Codex's CLI exactly as if it were an OpenAI model.

Routing goes straight at the merged **anthroproxy** binary on
`127.0.0.1:6969`, which serves both the Anthropic Messages
passthrough (`/v1/messages`, used internally) and the OpenAI
Responses translator (`/v1/responses`, what Codex actually POSTs to)
from a single process. There is no separate translator binary to
launch any more — until the May 2026 merge that lived in
`codex-rs/anthropic-translator/`, but it has since been folded into
the [`rust-vertex-ai-proxy`](https://github.com/) workspace and
ships inside Anthroproxy.app. Translator deep-dive (wire contract,
per-model rules, tool reshaping, manual cache planner, stream
translator state machine, beta header support, known gaps, recipes
for extending) lives at:

- `~/Documents/Personal/VertexAI-Anthropic-Proxy/rust-vertex-ai-proxy/anthropic-translator/README.md`

---

## Prerequisites

- This repo cloned somewhere on disk.
- A working **anthroproxy** running locally (default
  `127.0.0.1:6969`). The easiest install is the menu bar app
  (`Anthroproxy.app`) packaged out of `rust-vertex-ai-proxy/` via
  `package.sh`. The bundled binary serves Anthropic Messages,
  Gemini passthroughs, GLM/MaaS, and the Codex translator routes on
  the same port.
- Rust toolchain (stable, pinned via `rust-toolchain.toml` to 1.93).

## One-time setup

### 1. Build the codex CLI

From the repo root:

```bash
cd codex-rs

# Codex CLI itself (~5–10 minutes first time; pulls a huge dep tree).
cargo build --release -p codex-cli

cd ..
```

Output binary lands at `codex-rs/target/release/codex`. Faster dev
builds via the workspace's `dev-small` profile:

```bash
cargo build --profile dev-small -p codex-cli
```

If you do that, edit the wrapper's `CODEX_BIN` path
(`scripts/cmxcdx`) to point at `target/dev-small/`.

### 2. Configure Codex

Edit `~/.codex/config.toml` to register anthroproxy as a model
provider and add an `opus` profile. The relevant blocks:

```toml
[model_providers.anthroproxy]
name = "anthroproxy"
base_url = "http://127.0.0.1:6969/v1"
wire_api = "responses"
stream_idle_timeout_ms = 600000

[profiles.opus]
model_provider = "anthroproxy"
model = "claude-opus-4-7"
model_reasoning_effort = "high"
model_reasoning_summary = "auto"
```

(If a previous session already set this up for you, skip this step.
Existing project trust levels and TUI settings stay untouched.)

### 3. Install the wrapper on your `PATH`

Symlink `scripts/cmxcdx` into a directory on your `PATH`:

```bash
mkdir -p ~/.local/bin
ln -sf "$PWD/scripts/cmxcdx" ~/.local/bin/cmxcdx
```

Make sure `~/.local/bin` is on `PATH` (add to `~/.zshrc` if needed):

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify:

```bash
which cmxcdx
# /Users/you/.local/bin/cmxcdx
```

If you prefer a different alias, just symlink with that name.

---

## Daily use

```bash
cmxcdx                              # interactive: runs `codex -p opus`
cmxcdx -- exec "fix the auth bug"   # passes everything after `--` to codex
cmxcdx -- --help                    # codex's own help
```

The wrapper just defaults the profile to `opus` and execs the forked
`codex` binary. Anthroproxy must already be listening on `:6969`
(the menu bar app handles this in the background; if it's not
running, just open it).

---

## Customization

| Variable | Default | Effect |
|---|---|---|
| `CMXCDX_REPO` | parent of script dir | Path to the codemaxxxing-codex checkout. |
| `CMXCDX_PROFILE` | `opus` | Codex profile to invoke when no `--` args are given. |

Translator-side knobs (beta features, listen address, upstream URL)
now live on the merged binary in `rust-vertex-ai-proxy/`:

- `LISTEN_ADDR` — anthroproxy bind address (default `127.0.0.1:6969`).
- `TRANSLATOR_BETA` — comma-separated `anthropic-beta` feature ids,
  e.g. `context-management-2025-06-27,interleaved-thinking-2025-05-14`.

Set those on the anthroproxy process (or via the menu bar app's
`Edit Config` action) rather than on `cmxcdx`. Examples:

```bash
# One-off: enable context editing on the next anthroproxy launch.
TRANSLATOR_BETA="context-management-2025-06-27" \
    /Applications/Anthroproxy.app/Contents/MacOS/vertex-proxy

# Use a different cmxcdx profile (e.g. one you defined for Sonnet 4.6).
CMXCDX_PROFILE=sonnet cmxcdx
```

Anthroproxy log lives wherever the menu bar app routes it (defaults
to `~/.config/vertex-proxy/proxy.log`); the standalone translator log
(`/tmp/cmxcdx-translator.log`) is no longer written to.

---

## Desktop app

`cmxcdx-app` launches the OpenAI Codex Desktop bundle at
`/Applications/Codex.app`, which ships its own `codex` binary inside
`Contents/Resources/`. The bundled binary is upstream OpenAI's, but
the desktop reads `~/.codex/config.toml` on startup and honors any
`model_providers.*` block defined there. So routing the desktop
through anthroproxy is purely a config + thin wrapper job. No bundle
modification, no codesign hacks.

There are three upstream gotchas to work around (tracked in
[openai/codex#10867](https://github.com/openai/codex/issues/10867)):

1. The desktop hides the model picker entirely for any custom
   `model_provider` unless `requires_openai_auth = true` is set on it.
2. With API-key auth + a custom provider, the desktop's models manager
   has `should_refresh_models() == false`, so it never calls the
   provider's `/models` endpoint and the picker only shows the bundled
   OpenAI models (none of which anthroproxy can serve).
3. Pointing `model_catalog_json` at a JSON file in
   [`ModelsResponse`](https://developers.openai.com/codex/config-reference)
   shape forces the desktop into `StaticModelsManager`, which uses
   ONLY the catalog from that file. The checked-in catalog at
   `~/Documents/Personal/VertexAI-Anthropic-Proxy/rust-vertex-ai-proxy/anthropic-translator/data/models.json`
   contains just `claude-opus-4-7`, so the picker shows exactly one
   option and there's no way to misroute a GPT-5.x pick through
   anthroproxy.

### Setup (one-time)

Add these to `~/.codex/config.toml`:

```toml
# Top-level: route every desktop turn through anthroproxy.
model_provider = "anthroproxy"
# Top-level: feed the desktop a static catalog of the models
# anthroproxy can actually serve. Use your own absolute path.
model_catalog_json = "/Users/you/Documents/Personal/VertexAI-Anthropic-Proxy/rust-vertex-ai-proxy/anthropic-translator/data/models.json"

[model_providers.anthroproxy]
# ...your existing keys, plus:
requires_openai_auth = true
```

On first launch the desktop app will prompt for an OpenAI login.
Choose **"API key"** and enter any character; anthroproxy ignores it
and the upstream injects real Vertex auth itself.

### Daily use

```bash
cmxcdx-app                 # opens $PWD as the workspace
cmxcdx-app /path/to/repo   # opens a specific workspace
```

The script verifies the three workaround keys above are present
(refuses to launch and prints exact lines to add otherwise), checks
that anthroproxy is listening on `:6969`, then opens the workspace
in Codex Desktop and waits for it to quit.

The model picker should show **only Claude Opus 4.7** — the
`model_catalog_json` swap makes the desktop forget about its bundled
GPT-5.x list. That's by design: it makes routing unambiguous.

If anthroproxy gains support for more Claude models, add their
entries to
`rust-vertex-ai-proxy/anthropic-translator/data/models.json` (same
`ModelInfo` shape used in `models-manager/models.json`) and they'll
show up in the picker on next launch.

### Cleanup of older attempts

Earlier iterations of this script tried two dead-end paths:

1. Ad-hoc resigning a copy of `Codex.app` at
   `~/Applications/Codex (cmxcdx).app`. macOS 26's TXM rejects ad-hoc
   bundles regardless of entitlements.
2. Pre-seeding `~/.codex/models_cache.json` with a fake far-future
   timestamp. The desktop loaded the cache but the `model_catalog_json`
   path is the documented one and replaces this entirely.

If you have leftovers from either, clean them up:

```bash
rm -rf "$HOME/Applications/Codex (cmxcdx).app"
rm -f  "$HOME/.codex/models_cache.json"
```

---

## What's actually different vs. stock Codex

This fork doesn't modify the Codex CLI source at all. It's a
routing-only setup: the wrapper script + an entry in
`~/.codex/config.toml` pointing at anthroproxy. So:

- All of Codex's CLI features (TUI, slash commands, MCP servers,
  approval modes, sandbox profiles, etc.) work normally.
- Reasoning summaries appear in the TUI identically to OpenAI models
  (anthroproxy's translator emits matching
  `response.reasoning_summary_text.delta` events).
- Tool calling works for the standard tools (shell, apply_patch,
  exec_command, local_shell, web_search). `apply_patch` and
  `exec_command` (Codex's freeform / Lark-grammar tools) are
  synthesized as Anthropic JSON-schema tools with the grammar
  embedded in the description. `local_shell` round-trips as
  `local_shell_call` so Codex's `LocalShellHandler` accepts it
  without crashing the turn.
- `apply_patch` body content streams to the TUI character-by-character
  as it arrives, with full UTF-16 surrogate-pair handling (emojis,
  CJK supplementary chars, math symbols round-trip cleanly across
  chunk boundaries) and key-aware extraction.
- Anthropic's hosted web search results show up inline as assistant
  text with formatted citations (🔎 prefix for the call, bulleted
  result list for results, ⚠ for errors). Inline `citation_delta`
  events on text blocks are silently consumed without breaking the
  surrounding stream.
- Safety-redacted thinking blocks (`redacted_thinking`) round-trip
  end-to-end so the next turn's Anthropic-side validation succeeds.

Known gaps live in the translator's README in the
`rust-vertex-ai-proxy` repo (search for "Known gaps and future
work").

---

## Troubleshooting

**`cmxcdx: codex binary not found`** — build it:
`cd codex-rs && cargo build --release -p codex-cli`.

**Codex hangs / connection refused** — anthroproxy isn't listening on
`:6969`. Open `Anthroproxy.app` (menu bar) or run
`/Applications/Anthroproxy.app/Contents/MacOS/vertex-proxy` directly.
Probe with `curl -sf http://127.0.0.1:6969/`.

**`unsupported model: ...` 400 from anthroproxy** — your profile's
`model` field isn't a Claude model name the translator recognises.
Valid prefixes include `claude-opus-4-7`, `claude-opus-4-6`,
`claude-sonnet-4-6`, `claude-opus-4-5`, `claude-sonnet-4-5`,
`claude-haiku-4-5`. Vertex `@<date>` snapshots are accepted (e.g.
`claude-opus-4-7@20260101`).

**Reasoning doesn't show in the TUI on Opus 4.7** — set
`model_reasoning_summary = "auto"` (or any value other than `"none"`)
in your profile. Opus 4.7's API default is `display: "omitted"` and
the translator only overrides to `summarized` when summary is
non-none.

**Context window or 30 MB request size errors** — Vertex caps
request payloads at 30 MB. Long sessions with many tool results in
scope can hit this. Restart anthroproxy with
`TRANSLATOR_BETA="context-management-2025-06-27"` set in its env.
