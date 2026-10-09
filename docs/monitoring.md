# Monitoring, debugging, and built-in skills

## Operational monitoring

Add `--trace-dir ./run-traces` to `audit`, `evaluate`, or `agent` to write a
unique JSONL journal per run. Evaluation resolves this path before entering its
temporary workspaces, so journals survive workspace cleanup. No credentials
are required to inspect an existing journal:

```sh
alchemist audit --root . --target src --model MODEL --endpoint URL --trace-dir ./run-traces
alchemist inspect-trace ./run-traces/audit-RUN_ID.jsonl
```

Each record has schema version 1, an increasing sequence, a timestamp, elapsed
milliseconds, and a typed event. Operational events cover run start/end, model
turns, HTTP status, retries and delay, stream arrival/size, tool start/end,
exit status, tool timeout/truncation, and a heartbeat every five seconds.
The terminal summary includes outcome, last phase, turns, tools, tool errors,
retries, duration, and cumulative provider-reported token usage. Repeated
partial usage updates are merged within a turn; separate turns are added.
Missing provider usage remains zero, so it is not a billing estimate.

Context events report request size, the configured cap, approximate request
tokens, and before/after sizes for pruning or tool-result projection. See
[Context management](context-management.md) for retention and archive lifetime.
Archives are separate temporary tool-output files; ordinary journals do not
embed their contents.

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

Run `alchemist doctor --root .` without credentials to execute a fixed
Bash probe and discover optional rg, Git, Python, Forge, Cast, Node, GDB, LLDB,
and tmux commands.
Missing optional programs are reported without failing Bash readiness. The
command installs nothing and makes no model request. Use `AUDIT_BASH` to select
a Bash executable when automatic discovery is unsuitable. `RUST_LOG=info`
enables existing component diagnostics; tracing is written to stderr.

## Built-in skills

Ten skills are compiled into the executable:

| Skill | Use |
| --- | --- |
| `debugger-selection` | Preferred tools for 17 languages, with runtime/platform and unattended-execution guidance |
| `code-audit` | Repository discovery and input-to-operation tracing |
| `poc-validation` | Minimal reproductions, controls, and failed-test diagnosis |
| `native-debugging` | Native crash analysis and focused GDB/pwndbg inspection |
| `tmux-debugging` | Explicit pane targeting and bounded interactive debugging |
| `finding-review` | Evidence, root-cause deduplication, and final JSON review |
| `foundry-debugging` | Forge traces, typed cheatcodes, and import-free VM/console calls |
| `gdb-debugging` | A standalone GDB crash/breakpoint capture script |
| `node-inspector` | A separate Node Inspector controller with breakpoints and expression evaluation |
| `pwntools-debugging` | Byte-exact local process I/O with prompt checks and bounded receives |

Only names and descriptions enter the initial prompt. The agent consults a
skill when guidance is needed through `load_skill {"name":"native-debugging"}` and can fetch
its focused reference with `{"name":"native-debugging","resource":"references/pwndbg.md"}`.
Skill loads count toward `--max-tool-calls`, like other tools. Bash, writes,
edits, searches, and source reads remain fully available. Loading a skill does
not install a debugger or change tool permissions.

For human inspection, use `alchemist skills` or
`alchemist skills native-debugging --resource references/pwndbg.md`.
These commands work from any directory without model credentials. Exact
registered resources are served from the binary, so workspace files cannot
replace built-in instructions and resource names cannot traverse the filesystem.

### Choosing a debugger

Load `debugger-selection` for a tool-selection table covering Python, JavaScript,
TypeScript, C, C++, Rust, Go, Java, Kotlin, C#, Swift, Ruby, PHP, Dart, R, Bash,
and Solidity. Its five focused references supply launch examples, interactive
requirements, compatible alternatives, and links to upstream documentation.
The existing Foundry, GDB, Node Inspector, and pwntools skills supply executable
helpers where available. Recommendations do not imply those tools are installed.

```sh
alchemist skills debugger-selection
alchemist skills debugger-selection --resource references/managed.md
```

### On-demand script execution

Create a scratch directory through Bash, then export only the needed resource:

```json
{"name":"node-inspector","resource":"scripts/inspect.mjs","save_to":".audit-debug/inspect.mjs"}
```

`load_skill` with `save_to` writes the exact bundled resource to a new file
inside the audit root and returns its path and byte count. It does not repeat
the script source in the model context. Existing files and missing parent
directories are rejected. Without `save_to`, the resource is returned as text
for inspection. No script executes merely because a skill was loaded.

The equivalent CLI export is:

```sh
alchemist skills node-inspector --resource scripts/inspect.mjs --output .audit-debug/inspect.mjs
node .audit-debug/inspect.mjs --break app.js:42 --expression 'request.path' -- app.js
```

Each helper is independent and is run with the installed interpreter/debugger
through Bash. They inherit the existing tool timeout, output bounds, and
process cleanup. The binary bundles scripts, not third-party tool installations.
Node's helper requires global WebSocket support (Node 22.4+); GDB's capture
requires GDB Python support; the tube helper requires pwntools in its selected
Python environment. Forge uses the project's compiler and remappings.

Foundry compatibility is based on compiler/import and runtime-selector support,
not equal Forge/forge-std version numbers. The typed test uses `Test.vm` and
`console` when compatible. The fallback library supports Solidity 0.6–0.8 and
calls the cheatcode VM (`0x7109709ECfa91a80626fF3989D68f67F5b1DD12D`) and the
separate console address (`0x000000000000000000636F6e736F6c652e6c6f67`) directly.
It verifies a known VM return shape and preserves reverts from unsupported
selectors. Direct ABI calls avoid incompatible library imports; they do not
add missing runtime functionality. See the skill's compatibility reference.

Run `python -B tests/debug_scripts.py` for script tests. Missing optional runtimes
are reported as skips. Set `AUDIT_TEST_FOUNDRY=1` to run Forge compiler fixtures
(which may download solc), and `AUDIT_TEST_FORGE_STD` to a local forge-std checkout
for the typed test. Local validation covered Forge 1.8.1, solc 0.6.12 and 0.8.26,
Node 24.18.0, and forge-std commit `0258fe875e1d8e207c1eb7175e542ea32356773c`.
Linux CI installs GDB and pwntools for their native smoke tests and Node for
Inspector tests. `AUDIT_REQUIRE_DEBUG_TOOLS=1` makes missing dependencies fail
that job rather than silently skipping their tests. Local runs may still skip
unavailable optional tools; Foundry tests remain explicitly opt-in.

The debugger, tmux, and reporting guidance adapts selected MIT-licensed
[codex-auditor skills](https://github.com/0RAYS/codex-auditor/tree/5974e700bf5b44f10d885bb238dd8bcab8f42145/skills).
See [source attribution](../skills/UPSTREAM.md) and the retained
[license](../skills/LICENSE.codex-auditor). The upstream fixed report paths,
scoring schema, and large manuals were replaced with this harness's actual
JSON contract, process lifecycle, and on-demand references.
