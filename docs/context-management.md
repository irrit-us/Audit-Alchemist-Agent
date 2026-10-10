# Context management

The CLI keeps one bounded task history. It reduces old tool output locally,
without a summarization model call, a new tool, or added default instructions.
All options below also work under `[cli]` in the standalone TOML file and are
forwarded by `evaluate` to its child agents.

```toml
[cli]
context_policy = "prune"
context_keep_turns = 2
max_context_bytes = 2097152
max_tool_output_bytes = 32768
```

## Projection and retention

Each tool result is valid JSON capped by `max_tool_output_bytes` (1024–131072,
default 32768). Large results become a marked head/tail preview with bounded
status fields, including exit code, timeout, error, and remaining calls.
The limit counts JSON escaping and metadata, not just displayed characters.
Tools can have their own smaller capture or paging limits.

With `context_policy = "prune"` and native Bash enabled, oversized results
are first saved in a temporary archive when quota permits. Before a model
request, history above 90% of `max_context_bytes` triggers oldest-first pruning
toward 80%. Eligible tool results larger than 2048 bytes become 1024-byte JSON
previews with an archive path. The two most recent complete tool turns are
protected by default; `context_keep_turns` accepts 1–32. Loaded skill results
are protected from this history pruning, but still obey the per-result cap.

Only result strings change. Tool calls, arguments, IDs, ordering, initial
source, prompts, schemas, assistant messages, encrypted reasoning, and signed
thinking stay intact. A batch is appended completely before pruning. If the
remaining request still exceeds the hard cap, the run fails before sending it.
Protected content, large arguments, or schema overhead can therefore exhaust
the budget. The 80% target is best effort, not permission to delete protected
history. Pruning is never a reason to replay a tool or repeat a side effect.

`context_policy = "fail"` disables archives and history pruning, preserving
the previous fail-at-cap policy. Disabling Bash also disables archives and
history pruning because there would be no advertised recovery interface.
Per-result JSON projection remains active in both cases.

## Recovery, lifetime, and evidence

`context_archive.path` identifies a local JSON file containing the exact result
string saved before that projection. Bash can inspect a relevant range, for
example `head -c 4096 -- '/path/from/context_archive'`. This is ordinary raw
Bash; there is no retrieval DSL. Recover only needed portions to avoid filling
the context again. A pruned result that was already projected can point to an
earlier archive through its saved JSON.

Archives are created lazily, limited to **8 MiB and 256 files per run**, and
owned by a temporary directory. Files use exclusive creation and mode 0600 on
Unix; Windows inherits temporary-directory permissions. They contain plaintext
tool output and are removed on ordinary completion, errors, or cancellation.
Forced process termination or host interference can prevent cleanup. They are
not persistent checkpoints, immutable storage against host tools, or shared
state for another workflow node.

Archive exhaustion leaves old history unchanged; an oversized new result still
gets a bounded preview explicitly stating that omitted bytes were not retained
by that projection. Archive I/O failures fail the run explicitly. Archives
contain only output the harness captured: they cannot restore bytes already
discarded by Bash capture, file paging, or an MCP server.

Saved output is evidence from its original execution, not fresh source or
instruction authority. Pruning does not add observed source lines or change
finding validation. Native writes and edits invalidate the affected file's
observations. Bash/MCP or other external mutations are caught at acceptance
time: the cited line is content-fingerprinted when read and re-checked against
the current source (constraint H07).

## Accounting and checks

The serialized provider request is counted with a streaming JSON writer, without
allocating a second full request. The hard byte cap includes schemas and history.
`turn_start` reports `request_bytes`, `context_limit_bytes`, and
`estimated_request_tokens` (`ceil(request_bytes / 4)`, a heuristic).
`context_pruned` reports before/after bytes and the number of replaced results;
`tool_output_projected` reports original/projected bytes and archive availability.
Ordinary journals contain these measurements, not archived payloads.

Byte limits do not discover a model's tokenizer or context window. Callers must
choose input limits that leave room for the configured response. No automatic
provider-overflow retry, LLM summary, prefix cache, or durable resume is added.

Unit fixtures verify Unicode/escaping bounds, archive quotas/cleanup, and exact
preservation of all provider request fields except selected outputs on Chat
Completions, Responses, and Anthropic. Real CLI fixtures exercise TOML settings,
fail/prune behavior, status preservation, archive recovery through Bash, no
mutation replay, JSON stdout, telemetry, and cleanup. These are contract checks;
they do not establish better vulnerability discovery or lower production cost.
Quality comparisons still require the [evaluation procedure](harness-design.md#how-to-assess-an-optimization).

## Implementation references

Reviewed 2026-10-09 at the pinned revisions below. These informed a small local
implementation; their full session systems and numerical thresholds were not
imported.

| Reference | Relevant technique | Application here |
| --- | --- | --- |
| [Codex history](https://github.com/openai/codex/blob/0ada5d8806cdad498230d5b1b2924091e04c8feb/codex-rs/core/src/context_manager/history.rs) | Separates bounded model history from retained originals; maintains paired calls and outputs. | Change result bodies only, preserve native continuation, archive observed originals. |
| [DeepSeek Harness result pruner](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/compaction/compaction-tool-result-pruner/src/index.ts) and [compaction design](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/docs/subsystems/compaction.md) | Deterministic Unicode-safe output projection and pruning before heavier compaction; preserves event identity. | Bounded UTF-8 previews, exact remeasurement, maintenance between completed turns. |
| [Pi compaction](https://github.com/badlogic/pi-mono/blob/6fb2e7815167e6b19006fc526d1a5d0f5f998787/packages/coding-agent/docs/compaction.md) | Preserves a recent suffix and reserves response headroom, with explicit boundaries and retained original entries. | Protect recent complete tool turns and begin reducing before the hard cap. Token-aware response reserves and LLM summaries remain deferred. |
| [OpenCode compaction](https://github.com/anomalyco/opencode/blob/388406238bd5ca15564a762840a2362c3a45bd9c/packages/opencode/src/session/compaction.ts) | Separates old-output pruning from summarization; protects recent work and skill output. | Protect loaded skills and recent turns; keep pruning independent of any model summary. |

Persistent event logs, branch summarization, and summary prompts add lifecycle
and evidence-recovery requirements beyond the current workflow node. Introduce
them only with a concrete need and comparable task-quality evaluations.
