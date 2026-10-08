# Architecture

Audit Alchemist is a lightweight, task-specific vulnerability discovery agent. It
makes one model request per target, reads source without executing it, and emits
evidence-backed JSON findings. There is no agent framework, tool loop, persistent
service, or terminal UI.

## Layers

The library is layered so each concern stays auditable on its own. Models do not
invoke tools, and no module executes supplied source.

| Layer | Module | Responsibility |
| --- | --- | --- |
| Contract | `src/protocol.rs` | Versioned request/response types and path/CWE validation |
| Source access | `src/context/tools.rs` | Bounded, read-only filesystem operations inside a canonical root |
| Context | `src/context/mod.rs` | Deterministic snapshot assembly, token estimate, finding validation |
| Credentials | `src/provider/auth.rs`, `src/provider/auth/token.rs` | API keys and Sign In With ChatGPT/Codex login and refresh |
| Transport policy | `src/provider/retry.rs` | Transient-failure classification and jittered backoff |
| Wire formats | `src/provider/wire.rs` | Request bodies and stream decoders for chat-completions, Responses, and Anthropic |
| Streaming | `src/provider/sse.rs`, `src/provider/events.rs` | Incremental SSE reader and normalized events/usage |
| Adapters | `src/provider/mod.rs` | Credentialed clients for every wire format, with deadline and retry |
| Supervision | `src/runner.rs` | External agent process execution, deadlines, and cleanup |
| Evaluation | `src/dataset.rs`, `src/evaluate.rs`, `src/progress.rs` | Dataset loading, exact scoring, batch progress |
| Console | `src/output.rs` | `text`, `markdown`, `cot`, `body`, and `jsonl` renderers |
| TUI | `src/tui.rs` | Interactive `ratatui` view over the event stream |
| Entry points | `src/main.rs`, `src/bin/demo-agent.rs` | CLI orchestration and the deterministic fixture |

## Data flow

1. `main` parses the CLI and loads the dataset with `dataset::load`, which
   validates schema, targets, labels, and source lines.
2. For each case, `runner::run` spawns the built-in `agent` subcommand (or an
   external benchmark agent) with a `protocol::Request` on stdin and reads one
   JSON response from stdout.
3. Inside `provider::audit`, `context::build` is fed to a `provider::wire`
   request for the selected `--wire-api`, credentials come from `provider::auth`,
   the response streams through `provider::sse`, and `provider::wire` normalizes
   every delta into `provider::events::StreamEvent`s. `provider::retry` bounds
   transient failures. Findings are validated against the exact snapshot before
   being returned.
4. The `audit` command renders the events through `output` (or `tui`) on stderr
   and emits the JSON report on stdout. `evaluate` scores findings with
   `evaluate::score`; `progress` counts started, completed, and failed cases.
5. The report is written to stdout or `--output`; `tracing` progress goes to
   stderr only.

## Design principles

- **Bounded everywhere.** Source bytes, files, directory entries, HTTP bodies,
  output tokens, deadlines, and retry attempts all have explicit caps.
- **Fail closed.** Oversized context, malformed responses, and out-of-snapshot
  findings abort the case. Source is never silently truncated and invalid JSON
  is never repaired.
- **Source is untrusted data.** Prompts treat comments and strings as data, no
  module executes supplied source, and ground-truth labels never enter model
  context.
- **Read-only tooling.** `context::tools` is the only filesystem layer; it
  confines reads to a canonical root, prunes symlinks and dependency
  directories, and fails rather than truncating.
- **Machine-readable stdout, diagnostics on stderr.** Progress and retry
  warnings can never corrupt a report.
- **Credentials stay secret.** Tokens are never command-line arguments, log
  lines, or report fields.

See [Constraints](constraints.md) for the decision table and [Configuration](configuration.md)
for the resulting flags.
