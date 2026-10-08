# Configuration

All configuration is explicit and typed. Commands and flags are defined with
`clap` derive; run any command with `--help` for the authoritative list.

## Commands

| Command | Purpose |
| --- | --- |
| `audit` | Audit one file or directory with the built-in LLM agent. `--dry-run` reports the context without a request. |
| `evaluate` | Run the built-in agent over a labeled dataset and score it. |
| `benchmark` | Run another executable that implements the JSON stdin/stdout protocol. Repeat `--agent-arg=VALUE` for literal arguments. |
| `validate` | Check dataset schema, paths, labels, and source lines without running an agent. |
| `agent` | Internal protocol adapter: read one request from stdin, return one JSON response. The working directory is the source root. |

## Options

### Model and credentials (`audit`, `evaluate`, `agent`)

| Flag | Default | Notes |
| --- | --- | --- |
| `--model <ID>` | required | Provider model identifier. |
| `--auth <MODE>` | `api-key` | `api-key` uses an environment variable; `codex` uses a Codex ChatGPT login. |
| `--endpoint <URL>` | — | Full chat-completions URL. Required with `--auth api-key`; optional override for `codex`. |
| `--api-key-env <NAME>` | `AUDIT_API_KEY` | Environment variable holding the bearer token. |
| `--codex-auth-file <PATH>` | `$CODEX_HOME/auth.json` | Absolute path to a Codex credential file. |
| `--codex-base-url <URL>` | `https://chatgpt.com/backend-api/codex` | Codex Responses base URL. |

See [Authentication](authentication.md) for the credential details.

### Context and output bounds

| Flag | Default | Range |
| --- | --- | --- |
| `--max-source-bytes <N>` | `262144` (256 KiB) | 1–1048576 |
| `--max-source-tokens <N>` | disabled | 1–8388608; cap on the `ceil(bytes / 4)` estimate |
| `--max-tokens <N>` | `4096` | 1–32768 requested output tokens |
| `--timeout-ms <N>` | `60000` | 1–3600000 wall-clock deadline |
| `--max-output-bytes <N>` | `1048576` (1 MiB) | 1–16777216 stdout/stderr cap per run |
| `--jobs <N>` | `1` | 1–32 evaluation workers |

A byte bound is not a tokenizer estimate. `--max-source-tokens` optionally caps
the estimate used by `--dry-run` and the agent; the byte bound remains the hard
limit.

### Retries

| Flag | Default | Range |
| --- | --- | --- |
| `--max-attempts <N>` | `3` | 1–8; `1` disables retries |
| `--retry-base-ms <N>` | `500` | 0–60000 base exponential delay |
| `--retry-max-ms <N>` | `8000` | 0–600000 cap on backoff and `Retry-After` |

Transient failures (connection, TLS, timeout, and HTTP 408/425/429/500/502/503/504/529)
are retried with exponential backoff plus jitter, honoring `Retry-After`, and
always inside the run deadline. A successful response is never retried, even
when its content is invalid. A 401 from the Codex backend triggers one forced
re-authentication and retry.

### `audit` and `evaluate` specifics

| Flag | Default | Notes |
| --- | --- | --- |
| `--target <PATH>` | required (`audit`) | File or directory relative to `--root`. |
| `--root <PATH>` | `.` (`audit`) | Root the target must stay inside. |
| `--instruction <TEXT>` | generic audit instruction (`audit`) | Scope and threat model for the case. |
| `--dataset <PATH>` | required (`evaluate`, `benchmark`, `validate`) | Dataset file; paths resolve relative to its directory. |
| `--agent <PATH>` | required (`benchmark`) | Executable implementing the protocol. |
| `--dry-run` | off (`audit`) | Assemble and report the context without calling the model. |

## Reports and output

JSON goes to stdout or `--output`; tracing goes to stderr (`RUST_LOG=info`
enables progress). Output files are reserved before execution and are never
overwritten. An interrupted or failed run can leave an empty or partial output
file; remove it or choose another name before rerunning. Reports contain
findings, so handle them as source-derived sensitive material. Raw agent stderr
and API bodies are not persisted.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Execution completed successfully. |
| `1` | Invalid configuration/input or an internal error. |
| `2` | At least one agent invocation failed. |

Wrong findings lower the score but do not change a completed evaluation's exit
status. Ctrl-C aborts an audit or evaluation and emits no complete report.

## Execution supervision

The runner passes arguments directly without a shell. On Unix it creates a
process group and kills ordinary descendants on completion, failure, timeout, or
cancellation, then reaps the direct child. Other platforms terminate only the
direct child. Descendants that deliberately detach can escape the group. This is
execution supervision, not an OS security sandbox: external agents inherit the
environment and have filesystem and network access. Use a container or a
separate account when running untrusted agents.
