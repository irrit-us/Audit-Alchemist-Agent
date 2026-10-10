# Harness design research

Reviewed 2026-10-08 against the current working tree. These are primary-source
design references and project-specific recommendations, not evidence that a
change improves audit quality. The resulting requirements are in
[Constraints](constraints.md); contributor entry points are in [AGENTS.md](../AGENTS.md).

## Findings and application

| Source | Relevant finding | Application here |
| --- | --- | --- |
| [Anthropic: Building effective agents](https://www.anthropic.com/engineering/building-effective-agents) | Start with simple, composable loops and add complexity when measured outcomes justify it. | Keep the direct model/tool loop. Require a concrete failure case before adding orchestration, another agent, or a framework. |
| [Anthropic: early SWE-bench scaffold](https://www.anthropic.com/engineering/swe-bench-sonnet) | A small prompt and general-purpose Bash/editing tools supported iterative reproduction and verification. Tool descriptions were part of the interface. | Preserve Bash, writes, builds, and PoCs. Specify actual cwd, stdin, persistence, timeout, and output semantics. This public scaffold is not private early Claude Code source. |
| [Anthropic: Writing effective tools](https://www.anthropic.com/engineering/writing-tools-for-agents) | Distinct tool purposes, relevant responses, and outcome-based evaluations help reduce wasted calls and context. | Prefer focused search and paged reads. Add a native tool only when its semantics or measured reliability improve on Bash plus a skill. Track tool failures and retries as well as final findings. |
| [Anthropic: Effective context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) | Context needs active selection; clear prompts and just-in-time retrieval avoid filling it with low-value material. | Keep the initial skill catalog small, load references on demand, and retain explicit request limits. Any future compaction must preserve evidence and provider continuation state. |
| [Agent Skills specification](https://agentskills.io/specification) | Metadata, instructions, and resources are separate loading stages; scripts should describe dependencies and errors. | Keep language/debugger details in independent resources. Export exact scripts without overwrites. The current loader is a compiled subset, not a general filesystem skill discovery engine. |
| [OpenAI: Harness engineering](https://openai.com/index/harness-engineering/) | Short repository entry points, discoverable documentation, inspectable execution, and mechanically checked boundaries help agents maintain a codebase. | Use a short AGENTS.md pointing to constraints and architecture; associate invariants with tests. Keep ordinary operational events useful without requiring full payload capture. |
| [Anthropic: Effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents) | Incremental tasks, persistent progress records, and end-to-end verification help work continue across sessions. | If resumable audits are added, persist versioned checkpoints with source identity, remaining budgets, evidence, and pending actions. Current bounded journals are diagnostics, not resumable sessions. |
| [Anthropic: Demystifying evals](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents) | Evaluate the model and harness together, repeat trials, inspect actual outcomes, and distinguish capability evaluations from regression checks. | Preserve deterministic contract tests, add held-out vulnerability/control pairs for quality claims, and report execution failures separately from detection quality. Do not prescribe a single successful tool trajectory. |

The Pi, OpenCode, Goose, and Codex implementation links in
[Architecture](architecture.md#implementation-references) remain useful examples
of focused tools and prompts. Those links are historical implementation
references; this review does not claim to re-audit their current source or copy
their permission model. The constraints below are chosen for this project's
audit workload and existing contracts.

## Well-known harness strategies

Reusable patterns from widely used coding/agent harnesses, mapped to this
project. **Adopted** means code, a deterministic test, and (where stated) a live
round; **candidate** means documented with an acceptance criterion but not
implemented; **rejected** means it conflicts with an invariant or has no observed
failure to justify it. Live evidence is in [Validation](validation.md).

| Strategy | Source harnesses | Status | Evidence |
| --- | --- | --- | --- |
| Force a tool-free final answer when the action budget is spent | Codex, Claude Code | Adopted | `Conversation::disable_tools` + `final_turn_forced`; `exhausted_budget_forces_a_final_report_without_tools`; round 5 |
| Bounded final-output repair: return JSON/schema/evidence errors to the model | OpenAI strict schemas, Codex, Claude Code | Adopted | `--max-output-repairs` (default 2); `invalid_final_output_is_repaired_within_budget`; round 9 repaired two live malformed reports |
| Retry empty/no-content completions as transient provider responses | OpenAI/Anthropic SDK retries, Codex | Adopted | typed `EmptyCompletion` + `empty_completion_retry`; `empty_completion_is_retried_before_failing`; round 7 failure class absent in round 8 |
| Surface a failed child's stderr tail without putting it in the report | Codex/Claude Code verbose modes | Adopted | `runner::stderr_tail` warn; captured and redacted into evaluation `stderr.log` |
| Plan/todo tool (`update_plan`/`TodoWrite`) | Codex, Claude Code | Candidate | Bounded single-case audits; add only if a measured trajectory shows lost task state |
| Diff/patch editing (`apply_patch`) | Codex, Aider | Candidate | Audits are read-heavy; add on a measured edit-failure rate, not by default |
| Read-result cache/dedup | Cursor, Aider | Candidate | Measure duplicate-read tokens first; pruning already bounds old results |
| LLM context compaction/summarization | Claude Code | Rejected for now | Lossy evidence and extra model calls; deterministic pruning preserves raw replay |
| Auto-retry read-only tools | Goose | Candidate | Observed failures are model/provider-side; add on a measured idempotent-tool failure rate |
| Native `glob`/`grep` tools | Codex, OpenCode | Partial | `search` already provides batched, bounded search; no measured need for more tools |

## Recommended boundaries

The model chooses investigation steps. Rust owns parsing, budgets, process
lifecycle, provider replay, and final validation. Skills teach workflows;
they do not bypass these mechanisms. Providers normalize wire details without
owning file access or scoring. Evaluation labels stay in the evaluator. Console
and TUI consume events without determining findings. These boundaries follow
the existing [module map](architecture.md), rather than introducing another layer.

Full local execution is a product requirement. A small native tool surface is
compatible with broad capability because Bash can invoke installed tooling.
Reliability limits must constrain resource use, not turn the agent into a
read-only reviewer. For runs requiring separation from host files or credentials,
an explicit OS/container boundary is needed; a prompt, root-relative path check,
or temporary workspace cannot provide that guarantee.

Keep the audit prompt focused on scope, evidence, investigation, and the final
contract. Do not copy this research report, repository maintenance rules, or all
debugger manuals into every model request. A failed tool call should provide
enough information to recover; repeated unchanged attempts consume a finite
budget and should not be mistaken for progress.

## Prioritized gaps

These are follow-up recommendations, not features implemented by this review.

| Priority | Current gap and consequence | Acceptance criterion for future work |
| --- | --- | --- |
| P1 | `WorkspaceTools` records observed line numbers. Native writes invalidate them, but Bash or another process can change a previously read file without invalidation. Citation validation does not establish source-version identity. | Bind evidence to a source snapshot or content fingerprint, check it before accepting a finding, and test Bash/external edits as well as native writes. Preserve unrestricted PoC execution. |
| P1 | Usage defaults to zero when absent; journals lack a complete reproducibility manifest. A zero cannot establish a free run or a cost reduction. | Represent unavailable usage explicitly; record relevant harness/prompt/skill/dataset revisions and run settings without credentials. Define how partial streams and retried attempts affect accounting. |
| P1 | Six synthetic smoke cases test plumbing, not representative discovery or robustness to injected repository instructions. | Add held-out cross-file cases, safe controls, misleading comments/output, dependency failures, and repeated trials. Hide grading data behind OS isolation when claiming strict label separation. |
| Addressed in CI configuration | Rust checks now have a Linux/Windows matrix; lifecycle fixtures cover cancellation, background cleanup, and pipe capture. | Keep both platforms required for regression assessment. Linux debugger checks require their declared dependencies; Foundry remains opt-in. Actual run results belong in Validation, not this design table. |
| Conditional | There is no resume/compaction protocol. Replaying a truncated debug trace could lose evidence or repeat a mutating action. | Before adding resume, version checkpoints, verify source identity, preserve native continuation items, persist action completion and remaining budgets, and test interrupted mutations. |

## How to assess an optimization

Write down the failing behavior and success criterion before changing the
prompt, tool, or skill. Compare baseline and candidate with the same provider,
model/settings, dataset revision, source state, and budgets; record both harness
revisions and repeat trials. Keep held-out cases separate from tuning cases.

Measure supported findings, false positives on safe controls, misses, execution
failures, completed-run latency, timeout frequency, tool calls/errors/retries,
recovery counters (output repairs, empty-completion retries), and available token
usage. Include failures rather than selecting only successful runs. Report sample
size and variability; a recovery counter is direct evidence only when the
recovery path actually fired, not merely when the variant succeeded. Choose any
acceptable quality/latency tradeoff before examining the candidate results.
Monetary comparisons require known usage and a dated pricing basis. Fewer tokens
alone is not evidence of better auditing.

Deterministic fixture tests can establish protocol correctness and reproducible
failure handling. They cannot establish better vulnerability discovery. Record
that distinction in release notes and [Validation](validation.md). This review
made no live-model quality measurements.
