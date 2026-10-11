# Workflow integration API

This document is the normative interface reference for a **workflow management
program** (scheduler, DAG runner, CI job, or agent orchestrator) that drives
Audit Alchemist as one bounded node. It standardizes the process contract, the
JSON payloads, the exit codes, and the configuration resource. Detailed feature
behavior stays in [Configuration](configuration.md), the
[Version 1 protocol](protocol.md), and [Monitoring](monitoring.md); this page
states the machine contract those documents implement.

The harness is **not a resident service**. There is no daemon, socket, or network
API to call. The integration contract is: spawn one process per node, hand it
configuration and (for `agent`) one JSON request, read exactly one JSON document
from stdout, and branch on the process exit code. The caller owns scheduling,
retries across nodes, and cross-node state.

- Request/response schema versions are explicit integers (`schema_version`).
- Unknown top-level fields and unsupported schema versions are rejected.
- Machine-readable output is the **only** content on stdout.
- All progress, streaming, retry, and error diagnostics go to stderr.

## Process contract

| Property | Contract |
| --- | --- |
| Invocation | `alchemist <command> [flags]`, arguments passed directly without a shell. |
| Working directory | Caller-selected; path flags follow their documented resolution rules (see [Configuration](configuration.md#standalone-toml)). |
| stdin | `agent` reads one request object plus newline, then EOF. Other commands do not consume stdin. |
| stdout | Exactly one JSON document plus trailing newline for every command that emits a report; nothing else. |
| stderr | Live console stream, `tracing` diagnostics, and error text. Never part of the machine contract. |
| Exit code | `0` success, `1` invalid input/internal error, `2` one or more agent invocations failed. See [Exit codes](#exit-codes). |
| Filesystem | Root-relative file tools stay inside the audit root; **Bash is not sandboxed** and runs with host permissions. Use OS isolation for untrusted agents. |
| Output files (`--output`) | Created with exclusive create; an existing file fails before execution and is never overwritten. A failed run may leave a partial file. |
| Cancellation | `SIGINT`/Ctrl-C aborts and emits no complete report; child processes are cleaned up (Unix process groups, Windows kill-on-close jobs). |

### Streams and atomicity

- The JSON document on stdout is the workflow's source of truth.
- The optional `--format jsonl` stream on stderr is **observability only**; a
  workflow must not parse it as the result.
- A non-zero exit code does not by itself mean stdout is empty. Always parse the
  exit code and any emitted document, and treat a missing document as failure.

## Command endpoints

| Command | Reads | Writes to stdout |
| --- | --- | --- |
| `agent` | One [agent request](#agent-request) on stdin | [Agent response](#agent-response) |
| `audit` | CLI flags / TOML | [Audit result](#audit-result) (`--dry-run` emits [dry-run result](#dry-run-result)) |
| `evaluate` | CLI flags / TOML + dataset | [Evaluation report](#evaluation-report) |
| `benchmark` | CLI flags / TOML + dataset | [Evaluation report](#evaluation-report) |
| `validate` | Dataset file | [Validation result](#validation-result) |
| `check-config` | TOML file | [Configuration check](#configuration-check) |
| `doctor` | Optional `--root` | [Doctor report](#doctor-report) |
| `skills` | Optional name/resource | [Skills catalog or resource](#skills-catalog-and-resource) |
| `inspect-trace` | JSONL journal path | [Trace summary](#trace-summary) |

`evaluate` and `benchmark` share the same report shape. `evaluate` runs the
built-in agent in isolated per-case workspaces; `benchmark` runs an external
executable implementing the [Version 1 protocol](protocol.md) verbatim.

## Shared types

### Finding

```jsonc
{
  "cwe": "CWE-78",              // string, canonical CWE-<positive integer>
  "path": "sources/x.py",       // string, normalized relative POSIX path
  "line": 5,                     // integer, >= 1
  "severity": "high",           // "info" | "low" | "medium" | "high" | "critical"
  "title": "Command injection", // string, 1..1024 bytes, non-whitespace
  "evidence": "..."              // string, 1..16384 bytes, non-whitespace
}
```

### Finding key

The identity used for scoring; `severity`, `title`, and `evidence` are ignored.

```json
{ "cwe": "CWE-78", "path": "sources/x.py", "line": 5 }
```

CWE is `CWE-` followed by a positive decimal identifier without leading zeros
(at most 6 digits). Paths reject absolute form, `..`, `.`, backslashes, colons,
empty components, and NUL bytes.

## Payload reference

### Agent request

Sent to the built-in `agent` command on stdin by an external supervisor. Ground
truth is deliberately absent.

```json
{
  "schema_version": 1,
  "case_id": "command-injection",
  "target": "sources/command_unsafe.py",
  "instruction": "The name argument is attacker-controlled. Audit command execution."
}
```

Bound: at most 128 KiB. `target` is relative to the audit root (the process
working directory for `agent`).

### Agent response

```json
{
  "schema_version": 1,
  "findings": [
    {
      "cwe": "CWE-78",
      "path": "sources/command_unsafe.py",
      "line": 5,
      "severity": "high",
      "title": "Command injection",
      "evidence": "Attacker-controlled name is concatenated into a command executed with shell=True."
    }
  ]
}
```

`findings` is required even when empty; at most 10,000 entries. The built-in
adapter additionally requires each cited line to have been observed (initial
context or `read_file`) with a still-matching content fingerprint. External
adapters are responsible for the correspondence between findings and source.

### Audit result

Emitted by `audit` (non-dry-run), and reused as `run` inside evaluation reports.

```jsonc
{
  "case_id": "audit",
  "outcome": "success",
  "elapsed_ms": 4213,
  "exit_code": 0,          // integer or null
  "error": null,           // string or null
  "findings": [ /* Finding */ ]
}
```

`outcome` is one of the [outcome enum](#outcome-enum).

### Evaluation report

Emitted by `evaluate` and `benchmark`.

```jsonc
{
  "schema_version": 1,
  "dataset": "example-v1",
  "executable": "target/debug/alchemist",
  "args": ["--config", "/tmp/....toml"],
  "jobs": 2,
  "timeout_ms": 60000,
  "max_output_bytes": 1048576,
  "metrics": {
    "true_positives": 3,
    "false_positives": 0,
    "false_negatives": 0,
    "precision": 1.0,          // number or null when the denominator is zero
    "recall": 1.0,             // number or null
    "f1": 1.0,                 // number or null
    "successful_cases": 6,
    "failed_cases": 0,
    "exact_match_cases": 6,
    "elapsed_ms": 5120
  },
  "cases": [
    {
      "run": { /* Audit result */ },
      "matched":    [ /* FindingKey */ ],
      "unexpected": [ /* FindingKey */ ],
      "missed":     [ /* FindingKey */ ],
      "duplicate_findings": 0
    }
  ]
}
```

`cases` preserves dataset order regardless of `--jobs`. `duplicate_findings`
counts findings that collapse to an already-seen key and therefore do not affect
scoring.

### Dataset

Consumed by `evaluate`, `benchmark`, and `validate`. Paths are relative to the
dataset file's directory.

```jsonc
{
  "schema_version": 1,
  "name": "example-v1",
  "cases": [
    {
      "id": "command-injection",
      "target": "sources/command_unsafe.py",
      "instruction": "The name argument is attacker-controlled. Audit command execution.",
      "expected": [
        { "cwe": "CWE-78", "path": "sources/command_unsafe.py", "line": 5 }
      ]
    }
  ]
}
```

Bounds: file at most 4 MiB; 1–10,000 cases; unique IDs of at most 128 ASCII
alphanumeric/underscore/hyphen characters; nonempty instructions at most 64 KiB;
all targets inside the canonical dataset root; expected paths must be files
inside their case target; labels unique with existing lines. An empty `expected`
array declares a clean target.

### Validation result

```json
{ "valid": true, "dataset": "example-v1", "cases": 6 }
```

### Dry-run result

Emitted by `audit --dry-run`. No credentials, network, or MCP startup.

```jsonc
{
  "dry_run": true,
  "target": "sources/x.py",
  "files": 1,
  "bytes": 2048,
  "estimated_tokens": 512,
  "estimated_prompt_tokens": 780,
  "tools": ["bash", "read_file", "..."],
  "skills": [ { "name": "...", "description": "..." } ],
  "mcp_servers": ["analysis"],
  "max_tool_calls": 32,
  "context_policy": "prune",
  "context_keep_turns": 2,
  "max_tool_output_bytes": 32768,
  "instruction_bytes": 74,
  "model": "YOUR-MODEL",
  "auth": "api-key",
  "wire_api": "chat-completions"
}
```

Token counts are estimates (`ceil(bytes / 4)` plus prompt text), not tokenizer
exactness.

### Configuration check

Emitted by `check-config --config FILE`. Validates local structure, resources,
and ranges without credentials, network, or MCP startup.

```jsonc
{
  "valid": true,
  "tools": ["bash", "read_file", "..."],
  "skills": [ { "name": "...", "description": "..." } ],
  "mcp_servers": ["analysis"]   // enabled servers only
}
```

### Doctor report

Emitted by `doctor`. Executes one fixed Bash probe; installs nothing and makes
no model request.

```jsonc
{
  "ready": true,
  "os": "linux",
  "arch": "x86_64",
  "bash": {
    "exit_code": 0,          // integer or null
    "stdout": "...",
    "stderr": "",
    "truncated": false,
    "timed_out": false,
    "error": null            // present only on failure
  },
  "optional_tools_required": false,
  "note": "Missing optional tools do not disable auditing. AUDIT_BASH can select a Bash executable."
}
```

### Skills catalog and resource

`skills` with no name emits an array of catalog entries. With a name it emits the
resource object. `--output` writes raw `content` to a new file instead.

```jsonc
// alchemist skills
[ { "name": "poc-validation", "description": "..." } ]
```

```jsonc
// alchemist skills poc-validation --resource references/x.md
{
  "name": "poc-validation",
  "resource": "references/x.md",
  "content": "...",
  "available_resources": [ "references/x.md", "..." ]
}
```

### Trace summary

Emitted by `inspect-trace PATH` over the [run journal](monitoring.md). Captured
payloads are never returned here.

```jsonc
{
  "records": 42,
  "partial_tail": false,
  "complete": true,
  "status": "finished",           // "finished" | "running_or_interrupted"
  "start": { /* run_start details */ },      // or null
  "summary": { /* run_end details */ },      // or null
  "last_heartbeat": { /* details */ },       // or null
  "last_event": { "type": "operation", "name": "turn_end", "elapsed_ms": 900 }
}
```

### Outcome enum

Stable lowercase labels used by audit results, journals, and progress:

| Value | Meaning |
| --- | --- |
| `success` | One valid response accepted. |
| `timeout` | Deadline exceeded. |
| `spawn_error` | External agent could not start (`benchmark`). |
| `io_error` | stdin/stdout I/O failure. |
| `output_limit` | Agent stdout exceeded `--max-output-bytes`. |
| `nonzero_exit` | Agent exited non-zero (external agents). |
| `invalid_response` | stdout was not a valid Version 1 response. |
| `provider_error` | Built-in model request failed. |

Failed invocations contribute no accepted findings. There are no retries or
hidden repairs after invalid JSON beyond the explicit, bounded recovery flags.

## Error envelope

Errors are human-readable and go to stderr as `error: <message>` with a trailing
context chain. They are **not** JSON and are not written to stdout. A workflow
should classify failures by exit code and by the emitted document's `outcome`,
not by parsing error text. Representative sources:

| Condition | Exit | Result |
| --- | --- | --- |
| Unknown flag/value, invalid TOML, bad dataset, missing credential, unknown skill/resource | `1` | No document. |
| One or more evaluation cases fail | `2` | Report with `metrics.failed_cases > 0`. |
| Built-in audit fails (timeout/provider error) | `2` | Audit result with non-`success` outcome. |
| `agent` receives an invalid request | `1` | No response; diagnostics on stderr. |
| Trace write failure | `1` | Error raised at run end. |

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Execution completed successfully. |
| `1` | Invalid configuration/input or internal error. |
| `2` | At least one agent invocation failed. |

Wrong findings lower scores but do not change a completed evaluation's exit code.
Ctrl-C aborts without a complete report.

## Configuration resource

Each node is configured by exactly one explicit TOML file. Precedence is
built-in defaults, then TOML, then explicit CLI flags. No implicit search,
includes, or environment-based config discovery. The complete field reference is
in [Configuration](configuration.md#standalone-toml); the machine contract is:

```toml
schema_version = 1          # required; other versions are rejected
[cli]                       # CLI flags by underscore name; only applied to the invoked command
[prompts]                   # system/system_file, append/append_file (mutually exclusive pairs)
[tools]                     # enabled names, bash_program, bash_timeout_ms
[skills]                    # enabled names
[[skills.custom]]           # name, description, content|file, [skills.custom.resources]
[mcp.NAME]                  # enabled, command, args, cwd, tools, timeout_ms, max_message_bytes, env_from
```

Bounds: the config file and its resolved representation are each limited to
1 MiB; prompt resources 64 KiB; custom skill bodies/resources 8 KiB; at most 64
custom skills, 32 resources per skill, and 16 MCP servers. Paths owned by the
file resolve relative to its directory; `target` stays relative to the audit
root. `check-config` is the offline validity probe a workflow should gate node
startup on.

## Environment variables

| Variable | Used by | Meaning |
| --- | --- | --- |
| `AUDIT_API_KEY` | `audit`, `evaluate`, `agent` | Default bearer-token variable (`--api-key-env` selects another). |
| `AUDIT_BASH` | Bash tool, `doctor` | Overrides Bash executable discovery. |
| `CODEX_HOME` | `--auth codex` | Directory holding `auth.json` (default `~/.codex`). |
| `RUST_LOG` | All commands | `tracing` filter; diagnostics on stderr only. |

The provider API key variable is also the redaction source for `--debug-trace`
payloads. Credentials are never placed in command arguments, stdout, or reports.

## Operational semantics

- **Deadlines.** `--timeout-ms` bounds the entire audit, including tools and
  retries. Evaluation applies it per case.
- **Concurrency.** `--jobs N` runs evaluation cases in parallel; report order is
  still dataset order. An external `benchmark` agent is invoked once per case.
- **Isolation.** `evaluate` gives each case a temporary writable copy of the
  dataset root without the manifest. This prevents routine writes from leaking
  between cases; it is not an OS sandbox.
- **Retries.** Transient provider failures retry with jittered backoff inside the
  deadline; retry counts and delays are observable in the journal. Invalid final
  JSON is repaired only through `--max-output-repairs`.
- **Budgets.** Tool-call, context-byte, token, and output limits are explicit and
  enforced in code. An over-budget tool batch executes nothing and enters
  read-only settlement.
- **Cleanup.** Child processes and MCP servers are torn down on completion,
  timeout, failure, and cancellation.

## Versioning guarantee

- `schema_version` values (`1` today) gate request, response, dataset, report,
  configuration, and journal compatibility. Unsupported versions are rejected,
  never coerced.
- The `outcome` labels, `severity` labels, JSON field names, and exit codes are
  the stable machine surface. Human-readable error strings and stderr rendering
  formats are **not** stable for parsing.
- Additive optional fields may appear; consumers should ignore unknown fields in
  documents they read, while senders must still send only schema-valid requests.

See the [change history](../CHANGELOG.md) for compatibility-relevant changes and
[Constraints](constraints.md) for the invariants this interface must preserve.
