# Monitoring, debugging, and built-in skills

## Operational monitoring

Add `--trace-dir ./run-traces` to `audit`, `evaluate`, or `agent` to write a
unique JSONL journal per run. Evaluation resolves this path before entering its
temporary workspaces, so journals survive workspace cleanup. No credentials
are required to inspect an existing journal:

```sh
audit-harness audit --root . --target src --model MODEL --endpoint URL --trace-dir ./run-traces
audit-harness inspect-trace ./run-traces/audit-RUN_ID.jsonl
```

Each record has schema version 1, an increasing sequence, a timestamp, elapsed
milliseconds, and a typed event. Operational events cover run start/end, model
turns, HTTP status, retries and delay, stream arrival/size, tool start/end,
exit status, tool timeout/truncation, and a heartbeat every five seconds.
The terminal summary includes outcome, last phase, turns, tools, tool errors,
retries, duration, and cumulative provider-reported token usage. Repeated
partial usage updates are merged within a turn; separate turns are added.
Missing provider usage remains zero, so it is not a billing estimate.

The ordinary journal omits source text, prompts, commands, model text, and
tool payloads. It retains operational identifiers such as model and tool names,
call IDs, case ID, target, and the journal path. Text output shows lifecycle and heartbeat
updates; `--format jsonl` emits operational events alongside its existing
model text/reasoning stream on stderr. The TUI displays operation/retry status
and cumulative token usage. Final report JSON remains on stdout.

Journals use exclusive creation, flush each append, and stop capturing at
16 MiB with an explicit truncation marker and space reserved for the summary.
Creation or write failures are reported; a requested journal is not silently
discarded. Unix files are created with mode 0600; Windows inherits directory
permissions. Use a suitably protected directory for private projects.

`inspect-trace` shows the last heartbeat's phase and counters, omits captured
payloads, tolerates a partially written final
line, and rejects corrupt complete records. A journal without `run_end` is
reported as `running_or_interrupted`: it cannot distinguish a live process
from a forcibly killed one. Normal completion, errors, deadlines, and dropped
audit futures receive terminal outcomes. OS termination, forced evaluation
worker cleanup, power loss, or unwritable disks can prevent a final append.

## Detailed debugging

Use `--debug-trace` together with `--trace-dir` to capture bounded model request
bodies, assembled assistant text, tool arguments/results, and error chains.
Each payload retains at most approximately 32 KiB of head/tail text with an
explicit truncation marker. Debug payloads go only into the journal, not the
console event stream. This is a diagnostic capture, not a lossless wire replay;
raw HTTP headers, credential files, and encrypted reasoning state from
responses are not independently captured. A subsequent model request may
contain the provider's native continuation state.

The configured API key is redacted in raw and JSON-escaped forms before
payload truncation. This is not a general secret detector: source files,
commands, results, and error messages may contain other credentials or private
data. Review debug journals before sharing them. No traces are uploaded.

Start with the last phase and HTTP status. For `model`, check retries and
whether `stream_start` occurred; for `tool:NAME`, inspect timing and the
bounded tool result; for `validating`, use the debug response/error to diagnose
malformed JSON or unsupported citations. A nonzero Bash exit counts as a tool
error even when the tool successfully returned stdout/stderr. Missing
dependencies and failed PoCs remain available to the agent for recovery.

Run `audit-harness doctor --root .` without credentials to execute a fixed
Bash probe and discover optional rg, Git, Python, GDB, LLDB, and tmux commands.
Missing optional programs are reported without failing Bash readiness. The
command installs nothing and makes no model request. Use `AUDIT_BASH` to select
a Bash executable when automatic discovery is unsuitable. `RUST_LOG=info`
enables existing component diagnostics; tracing is written to stderr.

## Built-in skills

Five skills are compiled into the executable:

| Skill | Use |
| --- | --- |
| `code-audit` | Repository discovery and input-to-operation tracing |
| `poc-validation` | Minimal reproductions, controls, and failed-test diagnosis |
| `native-debugging` | Native crash analysis and focused GDB/pwndbg inspection |
| `tmux-debugging` | Explicit pane targeting and bounded interactive debugging |
| `finding-review` | Evidence, root-cause deduplication, and final JSON review |

Only names and descriptions enter the initial prompt. The agent consults a
matching skill through `load_skill {"name":"native-debugging"}` and can fetch
its focused reference with `{"name":"native-debugging","resource":"references/pwndbg.md"}`.
Skill loads count toward `--max-tool-calls`, like other tools. Bash, writes,
edits, searches, and source reads remain fully available. Loading a skill does
not install a debugger or change tool permissions.

For human inspection, use `audit-harness skills` or
`audit-harness skills native-debugging --resource references/pwndbg.md`.
These commands work from any directory without model credentials. Exact
registered resources are served from the binary, so workspace files cannot
replace built-in instructions and resource names cannot traverse the filesystem.

The debugger, tmux, and reporting guidance adapts selected MIT-licensed
[codex-auditor skills](https://github.com/0RAYS/codex-auditor/tree/5974e700bf5b44f10d885bb238dd8bcab8f42145/skills).
See [source attribution](../skills/UPSTREAM.md) and the retained
[license](../skills/LICENSE.codex-auditor). The upstream fixed report paths,
scoring schema, and large manuals were replaced with this harness's actual
JSON contract, process lifecycle, and on-demand references.
