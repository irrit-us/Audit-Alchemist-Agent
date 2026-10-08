# Audit Alchemist

A lightweight, task-specific vulnerability discovery agent built on a plain LLM
API, with a Rust CLI harness for bounded execution and small-dataset evaluation.
It makes one model call per target, reads source without executing it, and emits
evidence-backed JSON findings. There is no agent framework or terminal UI.

Built with clap, serde, tracing, Tokio, and reqwest.

## Quick start

Install stable Rust, then build and run the deterministic fixture:

```sh
cargo build --locked --bins
cargo run --locked -- validate --dataset datasets/smoke/dataset.json
cargo run --locked -- benchmark \
  --dataset datasets/smoke/dataset.json \
  --agent ./target/debug/demo-agent --jobs 2
```

The demo should produce 6 successful cases, 3 true positives, no false positives
or misses, and F1 = 1. It recognizes three simple source patterns and is a
plumbing fixture, **not evidence of LLM discovery quality**.

## Run a real audit

With an API key in `AUDIT_API_KEY`:

```sh
cargo run --locked -- audit \
  --root datasets/smoke --target sources/command_unsafe.py \
  --instruction 'The name argument is attacker-controlled. Audit command execution.' \
  --endpoint https://YOUR-PROVIDER/v1/chat/completions \
  --model YOUR-MODEL
```

With a ChatGPT/Codex subscription after `codex login`:

```sh
cargo run --locked -- audit \
  --root datasets/smoke --target sources/command_unsafe.py \
  --auth codex --model YOUR-CODEX-MODEL \
  --instruction 'The name argument is attacker-controlled. Audit command execution.'
```

Preview the context and token estimate without spending a request:

```sh
cargo run --locked -- audit --dry-run \
  --root datasets/smoke --target sources/command_unsafe.py
```

Evaluate the built-in agent over a labeled dataset and score it:

```sh
cargo run --locked -- evaluate \
  --dataset datasets/smoke/dataset.json \
  --endpoint https://YOUR-PROVIDER/v1/chat/completions \
  --model YOUR-MODEL --jobs 2 --output evaluation.json
```

`audit` snapshots the target only, skips dependency/build directories and nested
symlinks, and fails rather than silently truncating source. Findings must
reference an included file and an existing line. See
[Configuration](docs/configuration.md) for every flag and limit, and
[Authentication](docs/authentication.md) for credential handling.

## Terminal output and providers

`audit` streams to stderr while the JSON report stays on stdout:

```sh
cargo run --locked -- audit --root datasets/smoke --target sources/command_unsafe.py \
  --endpoint https://YOUR-PROVIDER/v1/messages --wire-api anthropic --model YOUR-MODEL \
  --format cot --color always

cargo run --locked -- audit --root datasets/smoke --target sources/command_unsafe.py \
  --endpoint https://YOUR-PROVIDER/v1/chat/completions --model YOUR-MODEL --tui
```

`--format` selects `quiet`, `text`, `markdown`, `cot` (readable chain of thought),
`body` (per-line timed answer with terminal control), `json`, or `jsonl`.
`--wire-api` selects `chat-completions` (OpenAI-compatible, covering DeepSeek,
Groq, Azure, and local servers), `responses` (OpenAI Responses), or `anthropic`
(Anthropic Messages). `--tui` opens an interactive terminal view. See
[Configuration](docs/configuration.md).

## Documentation

| Document | Contents |
| --- | --- |
| [Architecture](docs/architecture.md) | Layers, modules, data flow, and design principles |
| [Configuration](docs/configuration.md) | Commands, flags, limits, exit codes, and reports |
| [Authentication](docs/authentication.md) | API-key and Sign In With ChatGPT (Codex) credentials |
| [Protocol](docs/protocol.md) | Version 1 agent request/response and dataset contract |
| [Evaluation](docs/evaluation.md) | Matching, scoring, metrics, and reproducibility |
| [Constraints](docs/constraints.md) | Public constraints and design-decision rationale |
| [Validation](docs/validation.md) | Recorded smoke-set run and local checks |

The full index is at [docs/index.md](docs/index.md) and the change history at
[CHANGELOG.md](CHANGELOG.md).

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Tests cover protocol rejection, dataset and path validation, source bounds,
exact scoring, subprocess failures/timeouts/output limits, context and token
budgets, credential parsing, retry policy, wire decoders for all three formats,
console renderers, TUI frames, concurrent progress, the deterministic demo, and
a local HTTP fixture. No real network API calls are needed.

## License

MIT. See [LICENSE](LICENSE).
