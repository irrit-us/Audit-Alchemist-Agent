# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed

- Shortened the default audit prompt, skill descriptions, and contributor entry
  point. Detailed guidance stays on demand; skill loading is driven by need.

### Added

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
