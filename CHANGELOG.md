# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

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
