# Project constraints and design decisions

This project starts from a plain LLM API rather than a specific vendor's agent
CLI. Its public integration contract is intentionally small: one audit request
and one versioned JSON findings response. Internally, a bounded tool loop supports
code exploration, editing, Bash, and local PoC execution.
The module map is in [Architecture](architecture.md).

## Required design invariants

Reviewed 2026-10-08. These are requirements for changes to this project, not a
claim that every requirement is mechanically enforced today. **Code** means an
existing deterministic guard; **review** means a contributor requirement;
**gap** identifies incomplete enforcement. Source rationale and prioritized
follow-ups are in [Harness design research](harness-design.md).

| ID | Requirement | Enforcement and evidence |
| --- | --- | --- |
| H01 | Preserve full investigation capability: Bash, reads, writes, edits, search, builds, local PoCs, and installed debuggers. Do not impose a read-only audit workflow. | Code: `src/tools.rs`; `tests/agent_tools.rs` exercises read/write/execute on all three wires. Review: new policies must preserve this capability. |
| H02 | Keep one small model/tool loop with distinct protocol, transport, tool, evidence, evaluation, and presentation responsibilities. Additional frameworks, agent orchestration, or persistent services need a concrete use case and validation plan. | Review: `src/provider/mod.rs` and the architecture module map. No structural dependency linter is claimed. |
| H03 | Validate input and output boundaries in code. Keep versioned strict JSON and exact tool argument schemas. Invalid or incomplete final output must fail explicitly; never silently repair it into a successful empty report. | Code: `src/protocol.rs`, `src/tools.rs`, conversation parsers; `tests/harness.rs`, `tests/llm_api.rs`, `tests/monitoring.rs`. |
| H04 | All requests, source scans, tool outputs, history, retries, and subprocesses must have finite limits. One run deadline includes model waits, backoff, and tool work. Expose truncation/paging and remaining tool calls. Reject an over-budget batch before any mutation. | Code: provider loop, file tools, runner; `tests/agent_tools.rs` and `tests/foundations.rs`. Review: a new execution path must use the same budgets. |
| H05 | Execute only complete validated tool turns, preserve call/result IDs and native continuation items, and retain mutation order. Never automatically replay a mutating tool to recover a provider failure. | Code: `src/provider/conversation.rs` and ordered dispatch; incomplete-stream and native replay fixtures in `tests/agent_tools.rs`. Review: parallelism requires demonstrated independence. Provider retries do not guarantee exactly-once remote billing. |
| H06 | Drain pipes concurrently, bound retained output, and clean up owned descendants on timeout/cancellation. Do not leave an unattended debugger waiting indefinitely for stdin or a client. | Code: `src/runner.rs`, Bash supervision, `tests/tool_lifecycle.rs`, and timeout fixtures. CI runs Rust checks on Linux and Windows; optional debugger smoke tests run on Linux. Review: skill controllers own target cleanup. |
| H07 | Source, comments, paths, and tool output are evidence, not instruction authority. Preserve audited-source identity; observed execution must support any PoC claim. | Prompt/review: `prompts/audit.txt`. Code checks observed lines and invalidates native writes. Gap: Bash/external mutations are not source-version tracked; citation validation alone proves neither exploitability nor injection resistance. |
| H08 | Load only skill metadata initially, then requested instructions/resources. Keep exact compiled resource allowlists and no-overwrite script export. A skill must state dependencies and tested compatibility; loading it must not install tools or grant permissions. | Code: `src/skills.rs`, `src/provider/prompt.rs`, `tests/debug_skills.rs`. Registry test requires each resource under 8 KiB. Review: `skills/UPSTREAM.md` records imported material and licenses. |
| H09 | Keep operational telemetry bounded, local, correlated, and separate from JSON stdout. Ordinary journals omit payloads; detailed capture is opt-in. Describe redaction and missing usage honestly. | Code: `src/monitor.rs`, `tests/monitoring.rs`. Gap: missing usage currently becomes zero; debug redaction covers the configured API key, not arbitrary secrets; journals are not lossless replay/checkpoints. |
| H10 | Keep labels and grading outside normal agent inputs; stage separate writable evaluation cases. Do not call this an OS sandbox or claim it hides all host data from Bash. | Code: `src/dataset.rs`, `src/main.rs`, `src/evaluate.rs`; workspace fixture in `tests/agent_tools.rs`. Review/deployment: strict held-out isolation needs an OS/container boundary with labels and unrelated credentials inaccessible. |
| H11 | Separate contract reliability, vulnerability quality, and efficiency. Quality claims require repeated comparable held-out runs including safe controls and execution failures. Do not silently relax exact scoring to improve results. | Code: current scoring and duplicate/failure tests in `tests/harness.rs`. Review: `docs/evaluation.md`; six smoke cases and mock-provider tests cannot establish general discovery gains. |
| H12 | Keep requirements discoverable and synchronized with code. Put only stable entry rules in AGENTS.md; keep specialized manuals in skills and research in docs. Add focused regression checks for new executable boundaries. | Review: `AGENTS.md`, this document, architecture and monitoring docs. Follow existing CI commands; report checks actually run and limitations. |
| H13 | Design primarily for a focused workflow node: one bounded task and one JSON result. Leave scheduling, DAG orchestration, persistent state, and cross-node coordination to the caller. CLI execution must remain independent of the optional TUI. | Code/review: `audit` and stdin/stdout `agent` lifecycle; existing CLI fixtures. No resident service is required. |
| H14 | Configure prompts, skill selection/resources, native tools, MCP servers, provider settings, and budgets through one explicit versioned TOML file. CLI flags override file defaults; reject unknown keys and invalid limits. No ambient config search or implicit MCP startup. | Code: `src/config.rs`, `src/main.rs`, `src/mcp.rs`; configuration and subprocess tests in `tests/config_cli.rs`. |
| H15 | Prioritize CLI integration tests for optimization: configuration precedence, process I/O, deadlines, cleanup, provider continuation, and evaluation forwarding. Optional UI tests complement these boundaries. | Linux/Windows CI runs real CLI subprocesses and local mock provider/MCP servers. Fixtures prove contracts, not live-server interoperability or audit-quality gains. |

## Extension constraints

Bash must remain a thin execution interface: a command string and optional
timeout, passed unchanged as one argument to Bash. Let Bash interpret quoting,
pipes, redirects, heredocs, expansions, and compound commands. Do not add a
command DSL, command rewriting, tool-specific subcommands, or implicit
`set -e`/`pipefail`. Keep timeout, bounded capture, and descendant cleanup in the
supervisor. Preserve stdout, stderr, and exit status in the tool result; file
helpers and skill scripts remain optional conveniences. The shell syntax and
failure-semantics fixture in `tests/agent_tools.rs` protects this boundary.

Default-loaded content must stay concise and decision-relevant: audit scope,
evidence/output rules, actual tool semantics, and short skill routing metadata.
Keep research, language manuals, examples, and implementation history on demand.
Load a skill when guidance is needed, not merely because a keyword matches.
Avoid duplicating tool definitions in the system prompt. Review prompt size when
adding default content; a smaller prompt alone does not prove better audit quality.

New tools must state path/cwd rules, input limits, output shape, mutation effects,
deadline and cleanup behavior, and recoverable errors. Prefer an independent
skill script when Bash already provides the capability; use a native tool when
it supplies a distinct contract or demonstrated reliability/efficiency benefit.
Do not remove useful capabilities merely to minimize the number of tools.

Keep version-sensitive Foundry, GDB, Node Inspector, and pwntools behavior in
their skills. Typed Foundry cheatcodes require compatible interfaces and runtime;
raw VM calls can bypass an import mismatch but cannot add unsupported cheatcodes.
VM cheatcode and console logging addresses have different roles. Preserve
runtime checks and report actual debug output, not just a successful call bit.

If compaction, caching, resume, or parallel execution is introduced, first define
source identity, evidence retention, provider-state preservation, budget
accounting, invalidation, and mutation recovery. A truncated debug journal is not
a checkpoint. Do not promise exactly-once shell side effects after an interruption.
No persistent session mechanism is required by the current bounded-audit scope.

Current [context pruning](context-management.md) changes only old result bodies,
preserves paired calls and provider continuation, and protects recent turns and
skills. Archives are bounded to one run and recover observed output through
native Bash. Do not treat recovered output as fresh source, replay mutations to
recover it, or relax the final request cap when protected context cannot fit.

## Existing implementation checks

| Constraint | Implementation choice | Validation |
| --- | --- | --- |
| CLI configuration must be explicit and typed | clap subcommands, ranges, literal process arguments | CLI integration checks |
| Inputs and outputs need stable contracts | serde structs, required fields, schema versions, unknown-field rejection | Roundtrip and rejection tests |
| Source context and model output are finite | Source/file bounds, requested token limit, HTTP response bound | Snapshot and HTTP fixture tests |
| Pipes can deadlock if input/output are sequential | Concurrent stdin write, stdout/stderr reads, and process wait | Process fixture tests |
| Dropping a Tokio child does not normally stop it | kill-on-drop, deadline, Unix process groups, Windows jobs, explicit direct-child wait | Timeout and descendant cleanup tests |
| Progress must not corrupt machine-readable results | tracing subscriber writes to stderr | End-to-end JSON parsing |
| Evaluation must separate detection quality from execution failures | Exact finding-key matching, failed-case misses, duplicate suppression | Scoring and demo tests |
| Evaluation labels must be excluded from normal agent inputs | Prompt omits labels; per-case writable copies exclude the manifest (not a host-access sandbox) | HTTP request and workspace inspection |
| Provider selection and credentials are deployment-specific | Full endpoint, model ID, named key environment variable | No credential required for local tests |
| Auditing requires exploration and executable PoCs | `tools` exposes Bash/read/write/edit/list/search with output and runtime caps | Native tool-loop and PoC fixtures for all wires |
| Model context must be finite and findings must cite inspected source | Hard request byte cap; initial context and read_file register observed lines | Context budget, paging, and finding tests |
| Batch progress must not corrupt results or affect scoring | Lock-free `progress` counters emitted through tracing on stderr | Concurrent progress tests |
| A ChatGPT subscription must be usable without a second stored secret | `provider::auth` reuses and refreshes `$CODEX_HOME/auth.json` through the public OAuth token endpoint | Credential parsing and selection tests |
| Concurrent refreshes must not corrupt the shared credential file | Advisory lock, atomic token-field write-back, redacting `Debug`, no tokens in reports | Lock and token-merge tests |
| Streaming model output must stay bounded and parseable | `provider::wire` and `provider::sse` decode each wire format into normalized events with a 2 MiB cap | Wire and SSE parser tests |
| Providers differ in request shape and stream events | `provider::wire` normalizes chat-completions, Responses, and Anthropic into one event stream | Wire decoder tests |
| Human output must not corrupt the machine report | `output` and `tui` render on stderr; stdout stays JSON | Renderer and CLI tests |
| A TUI must be testable without a terminal | `tui::App` and `tui::render` are terminal-free and validated with `TestBackend` | TUI frame test |
| Brief provider outages should not fail a run | `provider::retry` bounds attempts with exponential backoff, full jitter, and `Retry-After` | Retry policy tests |
| Context size should be visible before spending a request | `context::estimate_tokens` and `audit --dry-run` | Token estimate and dry-run tests |

These decisions follow the documented interfaces of [clap derive](https://docs.rs/clap/latest/clap/_derive/_tutorial/index.html),
[Serde container attributes](https://serde.rs/container-attrs.html),
[Tokio process management](https://docs.rs/tokio/latest/tokio/process/index.html),
[tracing-subscriber formatting](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/index.html),
and [reqwest client configuration](https://docs.rs/reqwest/0.12/reqwest/struct.ClientBuilder.html).
Serde flattening with unknown-field rejection is avoided in the final contract
implementation.

Historical provider-specific live-run settings belong in
[Validation](validation.md#historical-live-evaluation-2026-10-07); they are not
constraints on provider selection or the current tool loop.

The smoke labels use the public [CWE-78](https://cwe.mitre.org/data/definitions/78.html),
[CWE-89](https://cwe.mitre.org/data/definitions/89.html), and
[CWE-95](https://cwe.mitre.org/data/definitions/95.html) taxonomy. The samples
are original, synthetic source fixtures, not downloaded benchmarks. Corrected
variants use argument-vector process execution, SQL parameter binding, and
literal parsing respectively. The literal-parsing case explicitly limits input
upstream and asks about arbitrary code execution; it is not a claim that literal
parsing eliminates every denial-of-service risk.

Tokio supports concurrent pipe draining, deadlines, HTTP I/O, and bounded
evaluation workers. Ratatui provides an optional event-driven TUI; it must not
own report validation or scoring. There is no persistent service, agent
framework, database, or unbounded task queue.

Optimization should proceed from measured results: run the smoke set with a
chosen provider, inspect false positives and misses, update the focused prompt
or add representative cases, then rerun under the same model and budgets. Keep
changes isolated and compare execution failures separately from
precision/recall. Expand to a held-out dataset before making claims about
general discovery effectiveness. The model can run PoCs, but the harness has no
independent exploit-validity grader, automatic prompt search, or provider cost
accounting. Provider-reported usage is available in run monitoring with the
limitations described above.
