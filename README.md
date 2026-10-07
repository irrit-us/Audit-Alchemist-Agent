# Audit Alchemist

A lightweight, task-specific vulnerability discovery agent built on a plain LLM API, with a Rust CLI harness for bounded execution and small-dataset evaluation. Uses clap, serde, tracing, Tokio, and reqwest. It makes one model call per target, reads source without executing it, and emits evidence-backed JSON findings. No agent framework or terminal UI is required.

## Quick start

Install stable Rust, then:

```sh
cargo build --locked --bins
cargo run --locked -- validate --dataset datasets/smoke/dataset.json
cargo run --locked -- benchmark \
  --dataset datasets/smoke/dataset.json \
  --agent ./target/debug/demo-agent --jobs 2
```

The deterministic demo should produce 6 successful cases, 3 true positives, no false positives or misses, and F1 = 1. It is a plumbing fixture that recognizes three simple source patterns, **not evidence of LLM discovery quality**.

For a real audit, put your API bearer token in `AUDIT_API_KEY` using your normal secret management. Select a model and a full chat-completions compatible endpoint:

```sh
cargo run --locked -- audit \
  --root datasets/smoke --target sources/command_unsafe.py \
  --instruction 'The name argument is attacker-controlled. Audit command execution.' \
  --endpoint https://YOUR-PROVIDER/v1/chat/completions \
  --model YOUR-MODEL

cargo run --locked -- evaluate \
  --dataset datasets/smoke/dataset.json \
  --endpoint https://YOUR-PROVIDER/v1/chat/completions \
  --model YOUR-MODEL --jobs 2 --output evaluation.json
```

Loopback HTTP endpoints are supported for local models. Set `AUDIT_API_KEY` to a nonempty placeholder if the local server ignores authentication. `--api-key-env` selects another variable; tokens are never command-line arguments or report fields. A compatible server must accept `model`, `messages`, `temperature`, and `max_tokens`, and return one `choices[].message.content` string with `finish_reason: "stop"`. Models requiring a different API or sampling fields need a small adapter change in `src/llm.rs`.

`audit` sends a source snapshot to the selected provider. Directory snapshots include supported source extensions, skip dependency/build directories and nested symlinks, and fail when they exceed limits. Target files must be within `--root`. Source files must be UTF-8. Findings must reference an included file and an existing line. Narrow the target when context exceeds the configured bound; no silent source truncation occurs.

## Commands and limits

- `audit`: audit a source file or directory using the built-in plain LLM agent.
- `evaluate`: run that agent against every labeled case and score the results.
- `benchmark`: run another CLI agent against the same protocol and dataset; repeat `--agent-arg=VALUE` for literal arguments.
- `validate`: check the dataset without agent execution or API calls.
- `agent`: stdin/stdout JSON adapter used by the harness; the working directory is the source root.

`audit`, `evaluate`, and `benchmark` default to a 60-second deadline and 1 MiB each for stdout and stderr. Evaluations default to one job, configurable up to 32. LLM context defaults to 256 KiB of source across at most 128 files; responses are bounded to 2 MiB at the HTTP layer and 4,096 requested output tokens. A byte bound is not a tokenizer estimate. HTTP requests have a deadline, no redirects, and no automatic application retries. Each dataset case incurs at most one API request. `--max-tokens`, `--max-source-bytes`, `--timeout-ms`, and `--max-output-bytes` adjust these bounds.

JSON goes to stdout or `--output`; tracing goes to stderr (`RUST_LOG=info` enables progress). Output files are reserved before execution and never overwrite existing files. Interrupted/failed runs can leave an empty or partial output file; remove it or choose another filename before rerunning. Reports contain findings, so handle them as source-derived sensitive material. Raw agent stderr and API bodies are not persisted.

Exit codes: `0` means execution completed successfully, `1` means invalid configuration/input or an internal error, `2` means at least one agent invocation failed. Wrong findings lower the score but do not change a completed evaluation's exit status. Ctrl-C aborts an audit or evaluation and emits no complete report.

The runner passes arguments directly without a shell. On Unix it creates a process group and kills ordinary descendants on completion, failure, timeout, or cancellation, then reaps the direct child. Other platforms terminate only the direct child. Descendants that deliberately detach can escape the group. This is execution supervision, not an OS security sandbox: external agents inherit the environment and have filesystem/network access. Use a container or separate account when running untrusted agents.

## Protocol and evaluation

See [the protocol](docs/protocol.md) for request, response, and dataset examples. Ground-truth labels and case IDs are excluded from the LLM prompt. A source snapshot includes only the case target, with paths and source text; expected findings never enter model context.

Matching is exact on `(CWE, source-root-relative path, 1-based sink line)`. Duplicate predictions count once. Reports show matched, unexpected, and missed findings plus duplicate counts per case. Micro precision, recall, and F1 aggregate across cases. Undefined ratios are JSON `null`. Failed cases retain all expected findings as misses and cannot count as exact matches. Clean cases measure false-positive behavior through unexpected findings and exact matches. Exact line matching is intentionally strict; assess near misses separately before changing matching policy.

The six handcrafted samples cover CWE-78, CWE-89, and CWE-95 with paired mitigations. They validate basic evaluation behavior, not representative vulnerability coverage. A live `deepseek-flash` run using the local Codex DeepSeek profile completed all six cases with 3 true positives, no false positives or misses, and F1 = 1.0 in 10.702 seconds. See [validation results](docs/validation.md) and the [full report](deepseek-evaluation.json). Credentials were passed through a transient environment variable and were not saved in the project or report.

Record model, prompt, dataset revision, limits, and repeat runs when comparing changes. Temperature zero does not guarantee reproducibility across providers. API usage/cost and automatic source hashes are not collected by this initial version; pin code and dataset commits for comparisons.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Tests include protocol rejection, dataset/path validation, source size limits, exact scoring, subprocess failures/timeouts/output bounds, demo evaluation, and a local HTTP fixture for the plain LLM adapter. No real network API calls are needed. The architectural decisions and public constraints are recorded in [constraints.md](docs/constraints.md).
