# Jev Context Compaction (Rust)

A typed Rust rewrite of
[`tamaratran/fast-jev-compaction`](https://github.com/tamaratran/fast-jev-compaction),
using the local `typesafe-ai` SDK. It removes stale tool calls or truncates their
results while leaving retained transcript content verbatim. Because some content
is removed, this is selective (not lossless) compaction.

The rewrite was based on upstream commit
[`e3f262a`](https://github.com/tamaratran/fast-jev-compaction/commit/e3f262a7f4d42bd8dd32ced30d26176f7cb545b0).

## Latency-oriented differences

- Uses TypeSafe's native question fan-out and the current 64k combined request
  budget, with a separate 32k state-plus-longest-question guard.
- Uses concise questions and normally evaluates hundreds of candidate tool calls
  in one request, avoiding repeated transmission of the full conversation.
- Shows Jev the first `truncateHeadChars` of each result: the newest heads keep a
  fifth of the state budget while old text shrinks, and the rest go oldest first.
  It then keeps the calls and full results Jev ranks highest until
  `keepBudgetRatio` (default 0.15) of the compactable tool-call characters is
  spent, rather than applying a fixed probability threshold; an item costing over
  a quarter of that budget ranks after all others.
- Reuses the SDK's pooled HTTP client.
- Disables retries and applies a three-second request deadline by default so the
  host can promptly fall back to ordinary compaction.
- Uses bounded concurrency only when multiple requests are unavoidable.
- Uses concrete Rust models and `typesafe_ai::Json`; no `Any`, trait objects, or
  `serde_json::Value` are exposed.

## Live result

On 2026-09-22 from Singapore, the release-mode integration test evaluated eight
tool calls as sixteen Noul questions in one production request: **724 ms** wall
time and 1,336 reported input tokens. This is a point-in-time Internet-path
sample, not an SLA; run the ignored live test in your deployment region:

```sh
cargo test -p jev-context-compaction --test live --release -- --ignored --nocapture
```

## CLI

Set `TYPESAFE_API_KEY`, then send a JSON object containing `messages` and optional
`options` on stdin:

```sh
cargo run -p jev-context-compaction --release < transcript.json
```

The output is a `CompactResult` JSON object. A host hook should fall back to its
built-in summary if the process exits unsuccessfully or the reduction is too small.

Library users can call `jev_context_compaction::compact` with an already shared
`typesafe_ai::Client`, avoiding client reconstruction and preserving pooled
connections across compactions.

The request budgets follow TypeSafe's current [model limits](https://docs.typesafe.ai/models).
The state representation follows the official [state guidance](https://docs.typesafe.ai/concepts/state),
and all candidate decisions sharing that state use native
[question fan-out](https://docs.typesafe.ai/patterns/fan-out).

## Claude Code plugin

Claude Code function hooks cannot start native processes. The included hook
therefore talks to an optional loopback-only Rust server. Keeping that server
alive also reuses the SDK client and its HTTP connection pool instead of paying
process and TLS setup on every compaction.

Build and start it with `TYPESAFE_API_KEY` in your environment, for example from a
`.env` file:

```sh
cargo build -p jev-context-compaction --release --features server \
  --bin jev-context-compaction-server
set -a; source .env; set +a
./target/release/jev-context-compaction-server
```

Then install the Claude Code plugin with
`claude plugin marketplace add PhiDung-hub/jev-context-compaction` and
`claude plugin install jev-context-compaction@jev-context-compaction`, or load
`claude/` from a local checkout. The
service binds only `127.0.0.1:8787` by default; override `FAST_JEV_LISTEN` if
needed. The hook contains no API key and falls through to Claude Code's built-in
compaction when the service or Jev fails, times out, or produces less than the
configured reduction.

When TypeSafe returns an exhausted-usage signal, the bridge opens a persistent
circuit and makes no further Jev calls. Host hooks disable their Jev path after
receiving the response and use built-in compaction. Remove the file configured by
`FAST_JEV_PAUSE_FILE` and restart the service only after replenishing usage.

The path is:

```text
Claude hook -> one loopback POST -> shared Rust Client -> one Jev fan-out request
```

Claude displays `jev-context-compaction started` while the Jev request is active,
then reports whether its result was applied or the built-in fallback was used.
With the plugin installed, compaction is automatic at 60% context usage after a
turn completes, not on every message. A manual Claude Code compaction also uses
the same hook. Restart an existing Claude Code session to load the renamed plugin.
Automatic compaction retries only after context usage grows by another ten
percentage points (configurable with `autoRetryDeltaPercent`), avoiding a Jev
request on every turn when compaction cannot reduce a transcript.

Codex uses its native compaction; this pack no longer installs Codex hooks or
monitors Codex sessions. The TypeSafe API key remains attached only to the
systemd bridge through its environment file. The Claude plugin does not store
or receive it.

After replenishing TypeSafe usage, re-enable the integration with:

```sh
rm ~/.local/state/fast-jev-compaction/usage-exhausted
systemctl --user restart jev-context-compaction.service
```

The old `fast-jev-compaction` CLI and `fast-jev-compaction.service` names remain
compatibility aliases. The pause-marker path keeps its old name so an exhausted
usage circuit is never silently reset by this rename. The obsolete plugin
package and marketplace entry have been removed.

See `NOTICE.md` for source attribution.
