# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Verified non-exact-match findings from the live rounds
  (`docs/reports/tiny-unexpected-verification.json`) and promoted the genuine
  separate issues to dataset cases in the tiny submodule (`blake2b-key-overflow`,
  `aead-key-length`, `chacha20-short-key`, `signature-check-short-buffers`,
  `argon2-null-allocation`, `clear-panic-double-free`, `empty-hs256-null-key`,
  `cache-expiry-without-iat`, `cache-max-age-with-exp`, and the low-impact
  `layout-rounded-size-overflow-low-impact`). A clean `unsigned-opt-in-control`
  case with no expected findings measures false-positive behavior directly.
- Distinct `unexpected_valid`/`unexpected_invalid` metrics, per-finding reviews
  bound to the full claim and report hash (`docs/reports/round4-reviews.json`,
  `docs/reports/round5-reviews.json`, `docs/reports/round6-reviews.json`), and
  per-round/per-case execution records in `docs/reports/tiny-metrics-log.jsonl`
  for round-over-round comparison (`round10`-`round16` reviews included).
  `scripts/evaluate_round.py --log` appends the compact log and supports
  per-variant limit overrides and per-case threat-model runs. The default prompt
  now prefers reachable medium-or-higher root causes and omits low-impact
  hardening.
- Line-only `(path, line)` precision/recall/F1 alongside exact `(CWE, path, line)`
  and reviewed `unexpected_valid`, so sink discovery is not hidden by CWE label
  choice. The [experiment log](docs/experiments.md) records the round ledger and
  the accumulated evidence: robustness fixes drove completion, exact F1 is
  label-bound, larger budgets did not help, a 12-call budget cut ~32% of tokens
  at equal-or-better F1 (rounds 13-14), forced context pruning is rejected
  (round 15), and the 18-case stable baseline is exact F1 .329 / line F1 .493
  over two trials (round 16). `update_plan` and `apply_patch` are retired on the
  measured tool-error distribution (`bash` 96% of errors, `edit_file` called
  once).
- Two bounded recovery strategies selected from well-known harnesses
  (`docs/harness-design.md`): `--max-output-repairs` (default 2) returns a
  rejected final report's JSON/schema/evidence error to the model for a corrected
  answer, and an empty completion is retried as a transient provider response up
  to `--max-attempts`. A tool batch that would exceed the budget is rejected
  without executing a call, then finalized (`tool_budget_rejections`). All are
  covered by deterministic tests; rounds 9 and 12 repaired live malformed
  reports. Recovery counters (`output_repairs`, `empty_completions`,
  `tool_budget_rejections`) are recorded in run journals and the metrics log. A
  failed child's stderr tail is surfaced through operational logs (never the
  report).

- Blind live evaluation baseline over the seven `datasets/tiny` fixtures
  (`docs/reports/tiny-baseline-round1.json`): 2/7 completed, 1 exact match, and
  per-case trajectory rounds, tool usage, and input/output token metrics. The
  blind instruction and manifest-excluding staging keep labels out of the model
  context. Recorded as a tuning baseline, not a quality claim.

- Real-world audit fixtures moved to the `alchemist-dataset-tiny` repository and
  vendored through the `datasets/tiny` git submodule: the Ajna incident plus six
  small fixtures covering TOCTOU symlink truncation, issuer-array validation,
  Rust unwind double free, a cross-language heap overflow, chunked-body parser
  disagreement, and standalone SHA256 comparator soundness. Each keeps its
  audited code under `audit/` and fix/evidence under `reference/`, with a
  versioned manifest and a `real_world_datasets_validate` harness test.

- Three-node caller-side bridge regression (`tests/bridge.rs`) that runs the
  committed Ajna fixture from `datasets/tiny/ajna-protocol-compromise-2/audit`
  through discovery, verification, and reporting `alchemist audit` nodes with one
  shared base prompt and per-role brief instructions, threads findings forward,
  and pushes per-node metrics (trajectory rounds, tool usage, input/output token
  usage, and result accuracy) as each node finishes.

- Configurable context pruning with protected recent turns and loaded skills,
  bounded temporary output archives recoverable through native Bash, and
  request/projection telemetry. Documented pinned Codex, DeepSeek Harness, Pi,
  and OpenCode design references and CLI/provider regression coverage.

### Fixed

- When `--max-tool-calls` is exhausted, the provider now removes the tool
  definitions and forces a final report turn instead of failing with
  `agent exhausted --max-tool-calls without a final report`. Exercised on all
  three wires by `exhausted_budget_forces_a_final_report_without_tools`.
- A finding's cited line is content-fingerprinted when supplied by the initial
  context or `read_file` and re-checked against the current source before the
  report is accepted, so a Bash or external edit after the read can no longer
  stale a citation. Native writes and edits invalidate observations, and
  re-reading a changed line makes it citable again
  (`citation_fingerprint_rejects_external_source_change`).
- A tool batch that would exceed the remaining `--max-tool-calls` now executes
  nothing, then the provider drops the tool schemas and forces a final report,
  instead of failing the run. Covered on the chat-completions wire by
  `over_budget_batch_is_rejected_then_finalizes_without_mutation`; a 16 vs 24
  call A/B showed the larger budget costs ~40% more tokens with no supported
  finding gain, so the default stays 16.

### Changed

- Chat-completions requests now set `response_format = {"type":"json_object"}`
  so the final report is one JSON object rather than prose followed by JSON, and
  `--reasoning-effort` (`minimal`/`low`/`medium`/`high`) is forwarded to
  reasoning models. Prompt/completion wire contents and scoring are unchanged.

- Replace the fixed 2 MiB response cap with `--max-stream-bytes` (default
  8 MiB). Reasoning models emit far more SSE framing than assembled text, so the
  old cap aborted otherwise valid runs.

- Rename the default CLI binary and new release archives to `alchemist`.
  The Rust package/library name remains unchanged.

- Bound model-visible tool results as valid JSON, including escaping and
  truncation metadata, while retaining execution status. Count serialized
  request bytes without allocating an extra complete request.

## [0.1.1] - 2026-10-08

### Fixed

- Use GCC with Rust's bundled musl CRT for the final static PIE link, while
  compiling native C dependencies with musl-gcc. The initial `v0.1.0` CI candidate
  retained an ELF interpreter and was blocked from publication by the static-link gate.

## [0.1.0] - 2026-10-08 (unpublished candidate)

### Fixed

- Windows HTTP test fixtures now reset accepted sockets to blocking mode with
  read/write timeouts; delayed request fragments no longer cause `WouldBlock`.

### Changed

- Kept Bash as a command string plus optional timeout, with a shorter tool
  description and regression coverage for native shell syntax and failure semantics.

- Shortened the default audit prompt, skill descriptions, and contributor entry
  point. Detailed guidance stays on demand; skill loading is driven by need.

### Added

- CI-built AMD64 Linux GNU, static Linux musl, and Windows MSVC release archives,
  checksums, build metadata, extracted-binary smoke checks, and gated tag publication.

- Versioned standalone TOML configuration, explicit CLI overrides, custom on-demand
  skills, native tool selection, and resolved evaluation configuration snapshots.
- Opt-in MCP stdio tools on all three provider wires, with bounded discovery,
  request deadlines, tool-budget accounting, monitoring, and process cleanup.
- CLI-first workflow-node constraints and real CLI/MCP regression fixtures.

- Process lifecycle, rejected-mutation, Unicode stream fragmentation, and
  all-provider tool rejection regression checks. Incomplete-stream tests now
  allow the full fixture tool budget, avoiding an unrelated failure masking bugs.
- Linux/Windows Rust CI matrix and an independent Linux debugger job, with
  explicit time limits, read-only repository permissions, and required tool checks.

- **Harness design constraints** with primary-source research, contributor
  guidance, enforcement/test mappings, and prioritized reliability/evaluation gaps.

- **Debugger selection guidance** for 17 programming languages, with preferred
  mature tools, runtime/platform distinctions, focused on-demand references,
  launch examples, and upstream documentation links.

- **Specialized debugging skills** for Foundry, GDB, Node Inspector, and pwntools,
  with independently loadable scripts. `load_skill` supports `save_to`, and
  `skills --resource --output` exports exact resources without overwriting files.
  Foundry includes typed cheatcode tests and an import-free VM/console fallback
  for Solidity 0.6–0.8 compatibility; optional tool discovery includes Forge and Node.
- **Operational monitoring**: bounded per-run JSONL journals, five-second
  heartbeats, model/HTTP/retry/tool timings and outcomes, cumulative usage,
  cancellation summaries, and `inspect-trace` for live or interrupted runs.
- **Opt-in debug traces**: bounded request/response/tool payloads and error
  chains, configured API-key redaction, and local `doctor` diagnostics.
- **Built-in skills**: a compiled catalog and `load_skill` tool for code audits,
  PoC validation, native debugging, tmux, and finding review. Selected
  codex-auditor guidance is adapted with pinned MIT attribution.

- **Native agent tool loop** for chat-completions, Responses/Codex, and Anthropic:
  Bash, paged source reads, file writes, exact edits, listing, and batched literal
  search. Provider reasoning state and tool-call IDs survive continuation.
- **Execution budgets**: `--max-tool-calls`, `--max-context-bytes`, bounded tool
  output, one overall deadline, and Windows process jobs for descendant cleanup.
- **On-demand discovery** for directory/large-file targets, numbered prompt
  context, credential-free dry runs, tool progress events, and isolated writable
  evaluation copies without the dataset manifest.

- **Mainstream wire formats.** `--wire-api` selects `chat-completions`
  (OpenAI-compatible), `responses` (OpenAI Responses), or `anthropic`
  (Anthropic Messages). All deltas are normalized into text, reasoning, and
  usage events.
- **Console output formats.** `audit --format` supports `quiet`, `text`,
  `markdown`, `cot` (readable chain of thought), `body` (per-line timed answer
  with terminal control), `json`, and `jsonl`; `--color` chooses ANSI color and
  `--tui` opens an interactive `ratatui` terminal UI.
- **`provider_error` outcome** for a failed model request from the now
  in-process `audit` path; `evaluate` and `benchmark` still supervise external
  agents.
- **Sign In With ChatGPT support.** `--auth codex` reuses a Codex CLI login at
  `$CODEX_HOME/auth.json`, refreshes the access token through the public OAuth
  token endpoint near expiry, and calls the Codex `/responses` backend with
  streamed Server-Sent Events parsing.
- **Bounded transport retries.** Connection, TLS, timeout, and transient HTTP
  failures are retried with exponential backoff, full jitter, and `Retry-After`,
  bounded by `--max-attempts`, `--retry-base-ms`, and `--retry-max-ms` inside the
  run deadline.
- **Context token estimation.** `estimate_tokens`, `--max-source-tokens`, and
  `audit --dry-run` report the context before spending a request.
- **Foundational modules**: read-only bounded filesystem tools, deterministic
  context assembly, lock-free batch progress, credential handling, SSE parsing,
  and retry policy.
- **Documentation set**: `docs/index.md`, `docs/architecture.md`,
  `docs/configuration.md`, `docs/authentication.md`, and `docs/evaluation.md`,
  plus this changelog.

### Changed

- Paged reads stream large files with bounded retained memory and an explicit
  scan cap; they no longer reject a small requested region solely because the
  file exceeds 1 MiB. Only returned lines become eligible finding citations.
- Bash timeouts preserve captured diagnostics and report `timed_out`; retained
  output preserves UTF-8 characters across chunk and head/tail boundaries.

- Audit guidance now encourages exploration and local PoC validation; final
  findings must cite an initially supplied or explicitly read source line.
- Search no longer silently stops when an exact byte budget is exhausted.
- Dataset containment checks canonicalize both sides for Windows compatibility.

- Source is grouped into `src/context/` (source access and context) and
  `src/provider/` (credentials, transport policy, streaming, and adapters).
- The committed evaluation artifact moved to `docs/reports/`.
- Transient retries replace the previous "one request, no retries" behavior;
  successful responses are still never retried.

## [0.1.0] - 2026-10-07

### Added

- Initial lightweight LLM audit harness: versioned JSON protocol, dataset
  validation, exact scoring, bounded subprocess supervision, a chat-completions
  adapter, a deterministic demo agent, and DeepSeek smoke validation.
