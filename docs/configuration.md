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
| `doctor` | Check Bash execution and optional local tools without credentials. |
| `skills [NAME]` | List/read built-in skills; `--resource` selects a script/reference and `--output` exports its raw contents to a new file. |
| `inspect-trace PATH` | Summarize a live or completed JSONL run journal. |
| `agent` | Internal protocol adapter: read one request from stdin, return one JSON response. The working directory is the source root. |

## Options

### Model and credentials (`audit`, `evaluate`, `agent`)

| Flag | Default | Notes |
| --- | --- | --- |
| `--model <ID>` | required | Provider model identifier. |
| `--wire-api <WIRE>` | `chat-completions` | `chat-completions`, `responses`, or `anthropic`; forced to `responses` with `--auth codex`. |
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
| `--max-tool-calls <N>` | `32` | 1–256 tool executions per audit; a batch counts each call |
| `--max-context-bytes <N>` | `2097152` (2 MiB) | 4096–16777216 bytes for each serialized request, including tools and history |
| `--max-tokens <N>` | `4096` | 1–32768 requested output tokens |
| `--timeout-ms <N>` | `60000` | 1–3600000 wall-clock deadline |
| `--max-output-bytes <N>` | `1048576` (1 MiB) | 1–16777216 stdout/stderr cap per run |
| `--jobs <N>` | `1` | 1–32 evaluation workers |

A byte bound is not a tokenizer estimate. `--max-source-bytes` controls initial
file preloading; directories and larger files are explored on demand.
`--max-source-tokens` caps the initial raw-source estimate. `--dry-run` also reports
`estimated_prompt_tokens` for system/user text with JSON and line labels (excluding
tool schemas/provider framing), and the available tools. Dry runs need a model
name but no credentials or endpoint. Estimates can undercount or overcount.
`--max-context-bytes` is the hard request bound. `--max-tokens` applies to each
model turn; the wall-clock deadline covers the entire audit, including tools.

### Agent tools

| Tool | Use and bounds |
| --- | --- |
| `bash` | Run commands, rg/grep, builds, tests, and local PoCs in the audit root. Defaults to a 30-second timeout, with `timeout_ms` capped by the remaining audit time. Both output pipes are drained with bounded storage; truncated output is explicitly marked. Timeouts return `timed_out: true`, an error, and captured partial stdout/stderr. |
| `read_file` | Stream UTF-8 source pages with 1-based `offset`/`limit` (default 200 lines), line labels, and `next_offset`. Supports files larger than 1 MiB without loading them entirely. Scans at most 64 MiB per call (including skipped lines); retains only the bounded page. `total_lines` is null until EOF is reached. |
| `write_file` | Create/overwrite UTF-8 files up to 1 MiB; parent directories must exist. |
| `edit_file` | Replace a unique, exact `old_text` match; missing/ambiguous matches return an actionable error. |
| `list_files` | List up to 128 supported source files; prune build/dependency directories. Use Bash for broader listings and other file types. |
| `search` | Match any of 1–64 literal needles in one scan; at most 128 files, 1 MiB scanned, and 200 matches. A budget overflow fails instead of silently skipping files. |

Tool results are bounded around 32 KiB plus truncation metadata. Calls execute in
the model's order so a PoC can be written and then run in one batch. Errors return
to the model for correction; malformed/incomplete provider streams never execute
tools. A final response must satisfy the versioned JSON contract and cite an
observed source line. Tool start/end events appear on stderr and in the TUI.

Bash is available by default and runs with host permissions, including filesystem
and network access; root-relative file helpers and process limits are not a sandbox.
Set `AUDIT_BASH` to a Bash executable path (no embedded flags). Windows uses Git
Bash under Program Files when available; otherwise Bash must be on PATH. Shell
variables and cwd changes reset per call; files persist. Unix process groups and
Windows kill-on-close jobs clean up ordinary child processes on timeout/cancellation.

### Console output (`audit`)

The machine-readable report always goes to stdout or `--output`; these flags
control the live stream and findings summary on stderr.

| Flag | Default | Values |
| --- | --- | --- |
| `--format <FORMAT>` | `text` | `quiet`, `text`, `markdown`, `cot`, `body`, `json`, `jsonl` |
| `--color <WHEN>` | `auto` | `auto`, `always`, `never` |
| `--tui` | off | Interactive terminal UI (requires a TTY) |

| Format | Behavior |
| --- | --- |
| `quiet` | No stderr output. |
| `text` | Findings summary after the run. |
| `markdown` | Findings as Markdown. |
| `cot` | Reasoning stream (dim) followed by the answer. |
| `body` | Answer body only, one timed line at a time with terminal control. |
| `json` | No stderr output (the JSON report is on stdout). |
| `jsonl` | One JSON stream event per line. |

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

## Monitoring flags

`audit`, `evaluate`, and `agent` accept `--trace-dir PATH` for bounded per-run
JSONL journals. `--debug-trace` requires that directory and adds bounded
request/response/tool payloads. See [Monitoring and skills](monitoring.md) for
privacy, event fields, recovery, and diagnostic commands.
