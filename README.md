# codemaxxxing-codex

A personal playground fork of [`openai/codex`](https://github.com/openai/codex)
that drives Anthropic Claude (Opus 4.7 by default) through a near-stock
Codex CLI by routing every turn through a small local HTTP proxy
that speaks Codex's OpenAI Responses dialect on the inbound side and
the Anthropic Messages API on the outbound side.

This is **not** a product. It is a reference rig and exploration
space — a place to try ideas against the Codex codebase before
porting anything worth keeping into our main daily driver,
[`bb-deeplearning/codemaxxxing`](https://github.com/bb-deeplearning/codemaxxxing).

## Where this fits

There are two coding agents in this household:

- **[codemaxxxing](https://github.com/bb-deeplearning/codemaxxxing)** —
  the daily driver. An opinionated, heavily-customized fork of
  [OpenCode](https://github.com/anomalyco/opencode) used internally
  for all development on [Clauseo](https://clauseo.chat) and other
  [bbdeeplearning.systems](https://bbdeeplearning.systems) projects.
  Wave runner FSM, drafting-table TUI redesign, rewritten system
  prompts, custom agents — substantial divergence from upstream.

- **codemaxxxing-codex** (this repo) — the lab bench. A thin layer on
  top of `openai/codex` that lets the same Claude Opus 4.7 backend
  drive the Codex CLI/Desktop unmodified, so I can A/B agent
  behavior, prompt strategies, and sandboxing tradeoffs against a
  second harness without leaving my models or my routing setup.

The naming overlap is intentional — same backend persona
("codexmaxxxing by clauseo" in the rebranded TUI header), two
different harnesses.

## What's actually forked

The footprint is intentionally small so upstream merges keep
applying cleanly:

- `scripts/cmxcdx`, `scripts/cmxcdx-app` — wrappers that launch the
  forked Codex CLI / notarized Codex Desktop pointed at the local
  Anthropic-routing proxy on `127.0.0.1:6969`.
- `codex-rs/models-manager/models.json` — one new `claude-opus-4-7`
  entry so Codex's model registry recognises the slug.
- `codex-rs/tui/` — small rebrand of the session header and status
  card from "OpenAI Codex (vX.Y.Z)" to "codexmaxxxing by clauseo",
  plus regenerated `insta` snapshots. Codex `core`, `cli`, and
  `app-server` crates are untouched.
- `CMXCDX.md`, `AGENTS.md` — end-user setup and contributor notes
  for this fork.

No translator code lives in this repo any more. The OpenAI
Responses → Anthropic Messages translator that briefly lived at
`codex-rs/anthropic-translator/` has moved into a separate
(private) Vertex/Anthropic routing workspace and now ships
embedded inside the same proxy binary that already handles the
rest of the household's Anthropic traffic. One process, one port,
one bundle.

## Quickstart

End-user setup, env vars, config.toml blocks, and troubleshooting
all live in **[CMXCDX.md](./CMXCDX.md)**. The short version:

1. Have a local proxy running on `127.0.0.1:6969` that accepts
   Codex's OpenAI Responses requests on `/v1/responses` and forwards
   them to Anthropic (e.g. via Vertex). Any process that exposes
   that contract works; the wrappers just expect the port to be up.
2. `cd codex-rs && cargo build --release -p codex-cli`.
3. Register that proxy as a `model_provider` in `~/.codex/config.toml`
   with `wire_api = "responses"` and `base_url = "http://127.0.0.1:6969/v1"`,
   and add an `opus` profile pointing at it (full block in CMXCDX.md).
4. Symlink `scripts/cmxcdx` into your `PATH` and run `cmxcdx`.

## Upstream

Everything not listed under "What's actually forked" above is
`openai/codex` as-is. For Codex itself — installation against
OpenAI's hosted models, IDE integration, the desktop app, the
cloud Codex Web agent — see the upstream
[Codex Documentation](https://developers.openai.com/codex) and the
[upstream README](https://github.com/openai/codex#readme).

## License

Inherits the upstream Apache-2.0 license — see [LICENSE](./LICENSE).
