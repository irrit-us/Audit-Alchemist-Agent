# Configuration

All configuration is explicit and typed. Commands and flags are defined with
`clap` derive; run any command with `--help` for the authoritative list.

## Standalone TOML

The CLI is the primary workflow interface. Supply one file explicitly; the
harness does not search the working directory, user directory, or environment
for configuration. Copy [examples/node.toml](../examples/node.toml), then run:

```sh
alchemist check-config --config examples/node.toml
alchemist audit --config examples/node.toml --dry-run
alchemist audit --config examples/node.toml --model YOUR-MODEL
alchemist agent --config node.toml < request.json
```

`schema_version = 1` is required. Precedence is built-in defaults, then TOML,
then explicit CLI flags. Boolean overrides accept `--dry-run=false`,
`--tui=false`, and `--debug-trace=false`; bare flags still mean true.
Unknown keys, duplicate selections, invalid CLI values, and unknown schema
versions fail before model/server execution. CLI values in TOML are validated
even if a CLI flag overrides them. `check-config` validates local structure,
resources, selections, and value ranges without credentials, network access,
or MCP startup; it does not prove endpoint, executable, or server availability.
Dry runs also leave MCP servers stopped and report configured server names,
not undiscovered remote tool schemas.

| Section | Fields and behavior |
| --- | --- |
| `[cli]` | Named CLI flags with underscores: `model`, `endpoint`, `auth`, `wire_api`, `instruction`, `target`, context/tool/retry/time budgets, `trace_dir`, `debug_trace`, output flags, etc. Strings, integers, booleans, or string arrays for repeated `agent_arg`. Only options belonging to the invoked command are applied. Positional arguments and `config` itself are not file settings. |
| `[prompts]` | `system` or `system_file` replaces the built-in system prompt. `append` or `append_file` adds guidance. Each pair is mutually exclusive; each resource is nonempty UTF-8, at most 64 KiB. Rust output/evidence validation and budgets still apply. A replacement prompt must explain the response contract to the model. |
| `[tools]` | `enabled` selects exact native tool names; omission enables all seven, `[]` disables all. Dispatch rejects disabled tools even if a model invents a call. `bash_program` selects an executable path; `bash_timeout_ms` defaults to 30000, range 1–3600000. Bash still takes a raw command string and optional per-call timeout, capped by the run deadline. |
| `[skills]` | `enabled` selects exact built-in/custom names; omission exposes all, `[]` exposes none. Disabling `load_skill` also hides the catalog. |
| `[[skills.custom]]` | Unique `name` (ASCII letters/digits/underscore/hyphen, 1–48 bytes), `description` (1–256 bytes), and either `content` or `file` (UTF-8, at most 8 KiB). Built-in names cannot be shadowed. `[skills.custom.resources]` maps exact relative resource names to inline strings, each at most 8 KiB. Maximum 64 custom skills and 32 resources per skill. |
| `[mcp.NAME]` | A stdio server, described below. Disabled unless `enabled = true`. |

Skill bodies and resources are resolved locally when the file is loaded, but
only metadata enters initial model context. `load_skill` retrieves the requested
body/resource. `skills` listing, reading, and exporting honor the same config.
Resource export preserves the existing new-file-only behavior.

Paths owned by the file (`root`, `dataset`, `output`, `trace_dir`,
`codex_auth_file`, prompt/skill files, Bash executable, MCP cwd, and MCP commands
containing path components) resolve relative to its directory. A bare MCP command
uses PATH. `target` remains relative to the audit root; MCP `args` are literal
arguments with no path rewriting or shell expansion. Explicit CLI paths retain
their existing working-directory semantics. The config file and its resolved
representation are each limited to 1 MiB. No includes or implicit config layering.

`evaluate` freezes resolved agent settings in a temporary file and forwards it
to each child along with effective provider/budget flags. The snapshot survives
until workers finish and is then removed. An MCP server without `cwd` starts in
each staged audit workspace; an explicit `cwd` remains an intentional override.
`benchmark` runs the external agent exactly as configured through `agent_arg`;
it does not inject this harness's prompts/tools into another executable.

### MCP stdio servers

```toml
[mcp.analysis]
enabled = true
command = "/opt/analysis/bin/server"
args = ["--stdio"]
tools = ["lookup_symbol", "trace_callers"]
timeout_ms = 10000
max_message_bytes = 1048576

[mcp.analysis.env_from]
SERVER_TOKEN = "ANALYSIS_TOKEN"
```

`command` is required; `args` defaults to `[]`, `cwd` to the audit root, and
`tools` to all discovered tools (`[]` advertises none). `env_from` maps a child
environment name to an existing parent environment variable; a missing value
fails startup. The child also inherits the host environment and permissions.
Keep credential values out of the file and command arguments. Tool selection
controls dispatch, not OS access by Bash or server processes.

This implementation supports **stdio tools**, with newline-delimited JSON-RPC,
initialization/version negotiation, paginated `tools/list`, and `tools/call`.
It requests protocol `2025-11-25` and accepts `2024-11-05`, `2025-03-26`,
`2025-06-18`, or `2025-11-25`. It exposes no sampling, elicitation, roots,
resource, prompt, HTTP/SSE transport, or interactive authorization capability.
Server requests for unsupported client methods receive `-32601`; ping receives
an empty result. Server instructions are not injected into model prompts.

Tools appear as `mcp_NAME__REMOTE_NAME`, limited to 64 ASCII letters, digits,
underscores, and hyphens. Unrepresentable names, collisions, missing selected
tools, or non-object input schemas fail discovery. Limits are 16 servers,
128 discovered tools and 16 pages per server, 256 KiB of selected tool schemas
per server, and a 1–3600000 ms request timeout (default 10000). Message limits
are 1024–2097152 bytes (default 1048576). The complete model request still obeys
`max_context_bytes`; model-visible tool results obey `max_tool_output_bytes`.

MCP calls execute sequentially with native calls and count against the same
tool budget and overall deadline. Tool-level `isError` becomes a model-visible
error. Protocol/transport failures and timeouts close the affected session;
mutating calls are not retried. Stderr is drained without retention; it never
enters JSON stdout. On success, stdin closes and each child gets a brief exit
grace period; process groups/Windows jobs clean up descendants on teardown,
timeout, or cancellation. Operational journals record `mcp_start`, `mcp_ready`,
and the existing tool events. Payload capture remains opt-in; arbitrary server
secrets are not covered by the provider API-key redactor.

Protocol references: [stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
[initialization and shutdown](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle),
and [tool discovery/calls](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).

## Commands

| Command | Purpose |
| --- | --- |
| `check-config --config FILE` | Validate explicit local configuration and report selected capabilities without starting a model or MCP server. |
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
| `--reasoning-effort <LEVEL>` | — | `minimal`, `low`, `medium`, or `high`; sent with chat-completions requests to bound reasoning-model output. |

Chat-completions requests set `response_format = {"type":"json_object"}` so the
model returns one JSON object instead of prose followed by JSON. Providers that
reject JSON mode need the Responses or Anthropic wire, or an endpoint that
accepts the field. `--reasoning-effort` is ignored on those wires.

See [Authentication](authentication.md) for the credential details.

### Context and output bounds

| Flag | Default | Range |
| --- | --- | --- |
| `--max-source-bytes <N>` | `262144` (256 KiB) | 1–1048576 |
| `--max-source-tokens <N>` | disabled | 1–8388608; cap on the `ceil(bytes / 4)` estimate |
| `--max-tool-calls <N>` | `32` | 1–256 tool executions per audit; a batch counts each call |
| `--max-context-bytes <N>` | `2097152` (2 MiB) | 4096–16777216 bytes for each serialized request, including tools and history |
| `--context-policy <POLICY>` | `prune` | `prune` archives old results under pressure; `fail` retains history until the cap |
| `--context-keep-turns <N>` | `2` | 1–32 recent complete tool turns protected from history pruning |
| `--max-tool-output-bytes <N>` | `32768` | 1024–131072 bytes per model-visible JSON tool result, including metadata |
| `--max-tokens <N>` | `4096` | 1–32768 requested output tokens |
| `--max-output-repairs <N>` | `2` | 0–8 bounded retries after the final report fails JSON, schema, or evidence validation; `0` fails fast. A repair never fabricates a report |
| `--max-stream-bytes <N>` | `8388608` (8 MiB) | 1048576–33554432 streamed bytes per response, including SSE framing. Reasoning models can emit far more raw SSE than assembled text |
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
A tool batch that would exceed `--max-tool-calls` executes nothing; the harness
removes the tool schemas and asks for a final report so gathered evidence is not
discarded.
Dry runs report the context policy, protected-turn count, and result budget.
See [Context management](context-management.md) for pruning thresholds, protected
fields, temporary archives, exact accounting, and limitations.

### Agent tools

| Tool | Use and bounds |
| --- | --- |
| `bash` | Run commands, rg/grep, builds, tests, and local PoCs in the audit root. Defaults to a 30-second timeout, with `timeout_ms` capped by the remaining audit time. Both output pipes are drained with bounded storage; truncated output is explicitly marked. Timeouts return `timed_out: true`, an error, and captured partial stdout/stderr. |
| `read_file` | Stream UTF-8 source pages with 1-based `offset`/`limit` (default 200 lines), line labels, and `next_offset`. Supports files larger than 1 MiB without loading them entirely. Scans at most 64 MiB per call (including skipped lines); retains only the bounded page. `total_lines` is null until EOF is reached. |
| `write_file` | Create/overwrite UTF-8 files up to 1 MiB; parent directories must exist. |
| `edit_file` | Replace a unique, exact `old_text` match; missing/ambiguous matches return an actionable error. |
| `list_files` | List up to 128 supported source files; prune build/dependency directories. Use Bash for broader listings and other file types. |
| `search` | Match any of 1–64 literal needles in one scan; at most 128 files, 1 MiB scanned, and 200 matches. A budget overflow fails instead of silently skipping files. |

Model-visible tool results are bounded to 32 KiB by default, including JSON
escaping and truncation metadata. Calls execute in
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

The Bash interface accepts `command` and optional `timeout_ms` (default 30000,
capped by the remaining run deadline). The command is passed unchanged to
`bash --noprofile --norc -c`; Bash handles quoting, pipes, redirects, heredocs,
and expansions. The harness does not inject `set -e` or `pipefail`; commands can
select those options explicitly. Stdin is closed, so this is a noninteractive
shell call, not a persistent terminal. Results retain stdout, stderr, exit code,
and timeout/truncation metadata. Debuggers needing interactive control can use
the on-demand controller scripts or tmux guidance.

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
always inside the run deadline. A completed stream with no text and no tool calls
is also retried up to `--max-attempts` times without replaying any tool. A
response with text is never retried for transport reasons; if its final report
fails validation, `--max-output-repairs` returns the error to the model instead.
A 401 from the Codex backend triggers one forced re-authentication and retry.

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
cancellation, then reaps the direct child. Windows uses kill-on-close jobs to
clean up ordinary descendants; other platforms terminate only the direct child.
Descendants that deliberately detach on Unix can escape the group. This is
execution supervision, not an OS security sandbox: external agents inherit the
environment and have filesystem and network access. Use a container or a
separate account when running untrusted agents.

## Monitoring flags

`audit`, `evaluate`, and `agent` accept `--trace-dir PATH` for bounded per-run
JSONL journals. `--debug-trace` requires that directory and adds bounded
request/response/tool payloads. See [Monitoring and skills](monitoring.md) for
privacy, event fields, recovery, and diagnostic commands.
