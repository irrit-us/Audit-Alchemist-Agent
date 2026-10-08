# Architecture

Audit Alchemist is a lightweight, task-specific vulnerability discovery agent. It
uses a bounded model/tool loop to explore source, create and run PoCs, and emit
evidence-backed JSON findings. It supports console streaming and an optional TUI
without requiring an agent framework or persistent service.

The primary deployment is a CLI node inside a caller-owned workflow. An explicit
TOML file configures the node; the caller owns scheduling and cross-node state.
The optional TUI consumes the same events and does not own execution policy.

## Layers

The library separates provider protocols, tool execution, and evidence validation.

| Layer | Module | Responsibility |
| --- | --- | --- |
| Contract | `src/protocol.rs` | Versioned request/response types and path/CWE validation |
| Configuration | `src/config.rs`, CLI parser | Explicit versioned TOML, path resolution, CLI overrides, and evaluation snapshots |
| MCP | `src/mcp.rs` | Opt-in stdio servers, bounded discovery/calls, namespaced tools, and subprocess cleanup |
| Source access | `src/context/tools.rs` | Bounded, read-only filesystem operations inside a canonical root |
| Context | `src/context/mod.rs` | Deterministic snapshot assembly, token estimate, finding validation |
| Prompt | `src/provider/prompt.rs`, `prompts/audit.txt` | JSON-escaped instructions/numbered source and evidence-driven audit guidance |
| Agent tools | `src/tools.rs` | Bash, paged reads, writes, exact edits, listing, batched search, observed-line validation |
| Conversation | `src/provider/conversation.rs` | Streamed tool calls and provider-native history replay, including reasoning state |
| Skills | `src/skills.rs`, `skills/` | Compiled metadata catalog, on-demand guidance, and exact script resources |
| Monitoring | `src/monitor.rs` | Bounded local run journals, operational events, debug capture, and trace inspection |
| Credentials | `src/provider/auth.rs`, `src/provider/auth/token.rs` | API keys and Sign In With ChatGPT/Codex login and refresh |
| Transport policy | `src/provider/retry.rs` | Transient-failure classification and jittered backoff |
| Wire formats | `src/provider/wire.rs` | Request bodies and stream decoders for chat-completions, Responses, and Anthropic |
| Streaming | `src/provider/sse.rs`, `src/provider/events.rs` | Incremental SSE reader and normalized events/usage |
| Adapters | `src/provider/mod.rs` | Credentialed clients for every wire format, with deadline and retry |
| Supervision | `src/runner.rs` | External agent process execution, deadlines, and cleanup |
| Evaluation | `src/dataset.rs`, `src/evaluate.rs`, `src/progress.rs` | Dataset loading, exact scoring, batch progress |
| Console | `src/output.rs` | `text`, `markdown`, `cot`, `body`, and `jsonl` renderers |
| TUI | `src/tui.rs` | Interactive `ratatui` view over the event stream |
| Entry points | `src/main.rs`, `src/bin/demo-agent.rs` | CLI orchestration and the deterministic fixture |

## Data flow

1. `main` applies explicit TOML defaults and CLI overrides, then loads the dataset with `dataset::load`, which
   validates schema, targets, labels, and source lines.
2. For each case, `runner::run` spawns the built-in `agent` subcommand (or an
   external benchmark agent) with a `protocol::Request` on stdin and reads one
   JSON response from stdout.
   Evaluation forwards resolved prompts, skills, tools, and MCP settings in a
   temporary configuration snapshot; paths retain their file-relative meaning.
3. `context::initial` supplies a small target file, or leaves directories and
   oversized files for tool exploration. `provider::audit_with` runs the model
   under one deadline. `conversation` decodes tool calls and preserves native
   assistant messages, Responses reasoning items, and Anthropic signatures.
   `tools` executes calls in order and returns bounded results; failures are
   available to the model for correction. The loop ends with a validated JSON
   report or an explicit limit/provider error. Findings must cite observed lines.
   Enabled MCP stdio servers initialize and advertise selected tool schemas
   before the first model turn. Their calls share the native tool budget and
   monitoring events. No MCP instruction text is added to the system prompt.
4. The `audit` command renders the events through `output` (or `tui`) on stderr
   and emits the JSON report on stdout. `evaluate` scores findings with
   `evaluate::score`; `progress` counts started, completed, and failed cases.
5. The report is written to stdout or `--output`; `tracing` progress goes to
   stderr only.

## Design principles

- **Bounded everywhere.** Source bytes, files, directory entries, HTTP bodies,
  output tokens, deadlines, and retry attempts all have explicit caps.
- **Explicit limits.** Oversized requests, incomplete streams, unsupported
  citations, and exhausted tool budgets fail the case. Reads expose paging;
  shell output truncation is marked. Invalid final JSON is never repaired.
- **Evidence-driven execution.** Prompts distinguish source/tool data from
  instructions and observed PoC results from hypotheses. Bash can run code;
  process limits and root-relative file tools are not a sandbox.
- **Independent evaluation workspaces.** Each `evaluate` case gets a temporary
  writable copy with the dataset manifest excluded. Ordinary writes cannot
  contaminate later cases; arbitrary host access still requires OS isolation.
- **Machine-readable stdout, diagnostics on stderr.** Progress and retry
  warnings can never corrupt a report.
- **Credential handling.** The provider does not put tokens in command-line
  arguments, logs, or reports. Bash uses the host environment and permissions.

## Implementation references

Reviewed on 2026-10-08; these are design references, not copied implementations
or evidence of measured audit-quality improvements.

| Reference | Adaptation |
| --- | --- |
| [Pi read tool](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/core/tools/read.ts) | Small, focused file tools with offset/limit reads and bounded output. |
| [OpenCode read tool](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/tool/read.ts) | Explicit line labels, ranges, and continuation metadata. |
| [Goose system prompt](https://github.com/block/goose/blob/main/crates/goose/src/prompts/system.md) | Describe actual available capabilities; keep tool-specific contracts in tool definitions. |
| [Codex prompting guide](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide) | Focused discovery, batching, clear tool semantics, bounded head/tail output. |
| [Early Anthropic coding scaffold](https://www.anthropic.com/engineering/swe-bench-sonnet) and [Claude Code best practices](https://www.anthropic.com/engineering/claude-code-best-practices) | Minimal tool loop, inspect first, reproduce with a small executable test, verify evidence. The early scaffold is public guidance, not a claim of access to private Claude Code source; the best-practices URL now redirects to current documentation. |

Native replay follows [OpenAI function calling](https://developers.openai.com/api/docs/guides/function-calling)
and [Anthropic tool definitions](https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools).

See [Constraints](constraints.md) for required invariants and enforcement status,
[Harness design research](harness-design.md) for the research and follow-up gaps,
and [Configuration](configuration.md) for the resulting flags.
