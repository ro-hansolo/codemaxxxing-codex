# cmxcdx — codemaxxxing-codex

My wrapper that lets me drive Claude (Opus 4.7 by default) from
Codex's CLI exactly as if it were an OpenAI model. It auto-starts a
local translator that converts OpenAI Responses API calls into
Anthropic Messages calls and forwards them to my local **anthroproxy**
(which routes to Anthropic-on-Vertex).

This is the daily-use guide. The technical deep-dive lives at
[`codex-rs/anthropic-translator/README.md`](codex-rs/anthropic-translator/README.md)
— go there if I'm changing the translator, fixing a bug, or
onboarding to the wire-format work.

---

## Prerequisites

- This repo cloned somewhere on disk.
- A working **anthroproxy** running locally (default `127.0.0.1:6969`)
  that presents the Anthropic Messages API and routes to
  Anthropic-on-Vertex under the hood.
- Rust toolchain (stable, pinned via `rust-toolchain.toml` to 1.93).

## One-time setup

### 1. Build the binaries

From the repo root:

```bash
cd codex-rs

# Translator (~5 minutes, ~5 MB output binary).
cargo build --release -p codex-anthropic-translator

# Codex CLI itself (~5–10 minutes first time; pulls a huge dep tree).
cargo build --release -p codex-cli

cd ..
```

Output binaries land at:

- `codex-rs/target/release/codex-anthropic-translator`
- `codex-rs/target/release/codex`

If you want faster builds for development iteration, use the
`dev-small` profile that the workspace already defines (about 3× faster
than `--release`, runtime is plenty fast for daily use):

```bash
cargo build --profile dev-small -p codex-anthropic-translator -p codex-cli
```

If you do that, edit the wrapper script's `TRANSLATOR_BIN` /
`CODEX_BIN` paths (`scripts/cmxcdx`) to point at `target/dev-small/`
instead of `target/release/`.

### 2. Configure Codex

Edit `~/.codex/config.toml` to register the translator as a model
provider and add an `opus` profile. The relevant blocks:

```toml
[model_providers.anthroproxy]
name = "anthroproxy"
base_url = "http://127.0.0.1:7070/v1"
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

Make sure `~/.local/bin` is on `PATH` — add this to `~/.zshrc` (or
your shell init) if it isn't:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify:

```bash
which cmxcdx
# /Users/you/.local/bin/cmxcdx
```

If you prefer a different alias, just symlink with that name. The
wrapper doesn't hardcode `cmxcdx` anywhere.

---

## Daily use

```bash
cmxcdx                              # interactive: runs `codex -p opus`
cmxcdx -- exec "fix the auth bug"   # passes everything after `--` to codex
cmxcdx -- --help                    # codex's own help
```

The wrapper:

1. Checks if `127.0.0.1:7070` is already in use.
   - If yes (translator already running from a previous invocation),
     reuses it. Subsequent `cmxcdx` invocations are essentially
     instant.
   - If no, spawns the translator in the background, pipes its logs
     to `/tmp/cmxcdx-translator.log`, waits up to 5 seconds for it
     to bind.
2. Execs the forked codex binary against the translator.
3. On exit, kills the translator only if this invocation started it.

---

## Customization

All configuration is via env vars on the wrapper:

| Variable | Default | Effect |
|---|---|---|
| `CMXCDX_REPO` | parent of script dir | Path to the codemaxxxing-codex checkout. |
| `CMXCDX_LISTEN` | `127.0.0.1:7070` | Translator listen address. Match it in `~/.codex/config.toml`'s `base_url`. |
| `CMXCDX_UPSTREAM` | `http://127.0.0.1:6969` | Anthroproxy base URL. |
| `CMXCDX_BETA` | (none) | Comma-separated `anthropic-beta` features. |
| `CMXCDX_LOG` | `/tmp/cmxcdx-translator.log` | Translator stderr/stdout file. |
| `CMXCDX_PROFILE` | `opus` | Codex profile to invoke when no `--` args are given. |

Examples:

```bash
# Enable context editing + interleaved thinking on this run.
CMXCDX_BETA="context-management-2025-06-27,interleaved-thinking-2025-05-14" cmxcdx

# Point at an anthroproxy on a different port.
CMXCDX_UPSTREAM="http://127.0.0.1:8888" cmxcdx

# Use a different profile (e.g. one you defined for Sonnet 4.6).
CMXCDX_PROFILE=sonnet cmxcdx

# Tail the translator's logs in another terminal.
tail -f /tmp/cmxcdx-translator.log
```

For long-running debug sessions, run the translator manually in a
separate terminal (the wrapper will detect the existing port and
reuse it):

```bash
codex-rs/target/release/codex-anthropic-translator \
    --listen 127.0.0.1:7070 \
    --upstream http://127.0.0.1:6969
```

Set `RUST_LOG=codex_anthropic_translator=debug` for verbose tracing.

---

## Desktop app

`codex app` (and therefore `cmxcdx -- app`) launches the OpenAI Codex
Desktop bundle at `/Applications/Codex.app`, which ships its own
`codex` binary inside `Contents/Resources/`. The bundled binary is
upstream OpenAI's, but the desktop reads `~/.codex/config.toml` on
startup and honors any `model_providers.*` block defined there. So
routing the desktop through our translator is purely a config + thin
wrapper job. No bundle modification, no codesign hacks.

There are three upstream gotchas to work around (tracked in
[openai/codex#10867](https://github.com/openai/codex/issues/10867)):

1. The desktop hides the model picker entirely for any custom
   `model_provider` unless `requires_openai_auth = true` is set on it.
2. With API-key auth + a custom provider, the desktop's models manager
   has `should_refresh_models() == false`, so it never calls the
   provider's `/models` endpoint and the picker only shows the bundled
   OpenAI models (none of which our translator can serve).
3. Pointing `model_catalog_json` at a JSON file in
   [`ModelsResponse`](https://developers.openai.com/codex/config-reference)
   shape forces the desktop into `StaticModelsManager`, which uses
   ONLY the catalog from that file. Our checked-in catalog at
   `codex-rs/anthropic-translator/data/models.json` contains just
   `claude-opus-4-7`, so the picker shows exactly one option and
   there's no way to misroute a GPT-5.x pick through the translator.

### Setup (one-time)

Add these to `~/.codex/config.toml`:

```toml
# Top-level: route every desktop turn through the translator. The TUI
# already opts in via `[profiles.opus]`; the desktop UI doesn't honor
# profiles, so the global default has to point at anthroproxy.
model_provider = "anthroproxy"

# Top-level: feed the desktop a static catalog containing only the
# models the translator can actually serve. Pin to your absolute path.
model_catalog_json = "/abs/path/to/codemaxxxing-codex/codex-rs/anthropic-translator/data/models.json"

[model_providers.anthroproxy]
# ...your existing keys, plus:
requires_openai_auth = true
```

On first launch the desktop app will prompt for an OpenAI login.
Choose **"API key"** and enter any character; the translator ignores
it and the anthroproxy upstream injects real auth itself.

### Daily use

```bash
cmxcdx-app                 # opens $PWD as the workspace
cmxcdx-app /path/to/repo   # opens a specific workspace
```

The script:

1. Verifies the three workaround keys above are present (refuses to
   launch and prints exact lines to add otherwise).
2. Starts the translator on `:7070` if nothing's listening.
3. `open -W -a /Applications/Codex.app <workspace>` and blocks until
   you quit the app, then tears the translator down.

The model picker should show **only Claude Opus 4.7** — the
`model_catalog_json` swap makes the desktop forget about its bundled
GPT-5.x list. That's by design: it makes routing unambiguous.

If a future translator gains support for more Claude models, add their
entries to `codex-rs/anthropic-translator/data/models.json` (same
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

This fork doesn't modify the Codex CLI source at all. The only delta
versus upstream is the new `codex-anthropic-translator` crate plus the
wrapper script. So:

- All of Codex's CLI features (TUI, slash commands, MCP servers,
  approval modes, sandbox profiles, etc.) work normally.
- Reasoning summaries appear in the TUI identically to OpenAI models
  (the translator emits matching `response.reasoning_summary_text.delta`
  events).
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
  chunk boundaries) and key-aware extraction (the streaming
  extractor finds the literal `"raw"` key in the streamed JSON
  envelope no matter where the model puts it — leading rationale
  fields like `{"explanation": "...", "raw": "..."}` no longer
  swallow the patch body).
- Anthropic's hosted web search results show up inline as assistant
  text with formatted citations (🔎 prefix for the call, bulleted
  result list for results, ⚠ for errors). Inline `citation_delta`
  events on text blocks are silently consumed without breaking the
  surrounding stream.
- Safety-redacted thinking blocks (`redacted_thinking`) round-trip
  end-to-end so the next turn's Anthropic-side validation succeeds.

What's not yet wired up (see the translator's
[Known gaps](codex-rs/anthropic-translator/README.md#known-gaps-and-future-work)
section for full details with "what to do" notes):

- Auto-enabling beta features based on request size — opt in via
  `CMXCDX_BETA`.
- Server-side web search query streaming — the call is announced as
  one synthetic message, not character-by-character.
- Upstream retry logic — single attempt, returns 502 to Codex on
  network failure.

---

## Troubleshooting

**`cmxcdx: translator binary not found`** — you haven't built the
release binary. Run `cd codex-rs && cargo build --release -p
codex-anthropic-translator`.

**`cmxcdx: codex binary not found`** — same, for the codex CLI:
`cd codex-rs && cargo build --release -p codex-cli`.

**Translator starts but Codex hangs / times out** — anthroproxy isn't
listening on the upstream URL, OR isn't routing to a working Vertex
deployment. Check anthroproxy is up:
`curl -sf http://127.0.0.1:6969/v1/models | head` (or whatever
endpoint anthroproxy exposes for health). Tail
`/tmp/cmxcdx-translator.log` to see what the translator received and
forwarded.

**`unsupported model: ...` 400 from translator** — your profile's
`model` field isn't a Claude model name we recognise. Valid prefixes
include `claude-opus-4-7`, `claude-opus-4-6`, `claude-sonnet-4-6`,
`claude-opus-4-5`, `claude-sonnet-4-5`, `claude-haiku-4-5`. Vertex
`@<date>` snapshots are accepted (e.g. `claude-opus-4-7@20260101`).

**Reasoning doesn't show in the TUI on Opus 4.7** — set
`model_reasoning_summary = "auto"` (or any value other than `"none"`)
in your profile. Opus 4.7's API default is `display: "omitted"` and
the translator only overrides to `summarized` when summary is non-none.

**Context window or 30 MB request size errors** — Vertex caps request
payloads at 30 MB. Long sessions with many tool results in scope can
hit this. Enable context editing:
`CMXCDX_BETA="context-management-2025-06-27" cmxcdx`.

**Different upstream port** — set `CMXCDX_UPSTREAM` (env) or edit the
wrapper. `~/.codex/config.toml` tracks the *translator* port (`:7070`),
not the upstream port — that's a translator implementation detail.
