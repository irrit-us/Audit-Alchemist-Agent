# Public constraints and design decisions

This project starts from a plain LLM API rather than a specific vendor's agent
CLI. Its public integration contract is intentionally small: one source
snapshot, one request, one versioned JSON response. The task is static
vulnerability discovery; models do not invoke tools or execute supplied source.
The module map is in [Architecture](architecture.md).

| Constraint | Implementation choice | Validation |
| --- | --- | --- |
| CLI configuration must be explicit and typed | clap subcommands, ranges, literal process arguments | CLI integration checks |
| Inputs and outputs need stable contracts | serde structs, required fields, schema versions, unknown-field rejection | Roundtrip and rejection tests |
| Source context and model output are finite | Source/file bounds, requested token limit, HTTP response bound | Snapshot and HTTP fixture tests |
| Pipes can deadlock if input/output are sequential | Concurrent stdin write, stdout/stderr reads, and process wait | Process fixture tests |
| Dropping a Tokio child does not normally stop it | kill-on-drop, deadline, Unix process groups, explicit direct-child wait | Timeout and descendant cleanup tests |
| Progress must not corrupt machine-readable results | tracing subscriber writes to stderr | End-to-end JSON parsing |
| Evaluation must separate detection quality from execution failures | Exact finding-key matching, failed-case misses, duplicate suppression | Scoring and demo tests |
| Labels must not leak into model context | Prompt contains only instruction and selected source snapshot | HTTP request inspection |
| Provider selection and credentials are deployment-specific | Full endpoint, model ID, named key environment variable | No credential required for local tests |
| Auditing needs bounded, read-only access to untrusted source | `context::tools` confines resolve/read/walk/search to a canonical root with entry, file, and byte caps | Tool and symlink tests |
| Model context must be finite and traceable to exact bytes | `context` builds a deterministic snapshot and rejects findings outside it | Context budget and finding tests |
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

For the live validation, the local Codex profile supplied the model, provider
URL, and bearer credential; the profile itself was not changed.
[Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
documents provider settings. The harness calls the plain
[DeepSeek chat-completions API](https://api-docs.deepseek.com/api/create-chat-completion/)
directly, independently of the profile's Codex wire protocol. It preserves the
profile's `deepseek-flash` model selection and the provider's default thinking
behavior. Temperature zero is requested but does not control sampling in
DeepSeek thinking mode, as documented by the provider.

The smoke labels use the public [CWE-78](https://cwe.mitre.org/data/definitions/78.html),
[CWE-89](https://cwe.mitre.org/data/definitions/89.html), and
[CWE-95](https://cwe.mitre.org/data/definitions/95.html) taxonomy. The samples
are original, synthetic source fixtures, not downloaded benchmarks. Corrected
variants use argument-vector process execution, SQL parameter binding, and
literal parsing respectively. The literal-parsing case explicitly limits input
upstream and asks about arbitrary code execution; it is not a claim that literal
parsing eliminates every denial-of-service risk.

Tokio is justified by concurrent pipe draining, deadlines, HTTP I/O, and bounded
evaluation workers. Ratatui is omitted because machine-readable reports and
tracing meet the workflow. No persistent service, agent framework, database, or
unbounded task queue is added.

Optimization should proceed from measured results: run the smoke set with a
chosen provider, inspect false positives and misses, update the focused prompt
or add representative cases, then rerun under the same model and budgets. Keep
changes isolated and compare execution failures separately from
precision/recall. Expand to a held-out dataset before making claims about
general discovery effectiveness. This version does not implement automatic
prompt search, exploit verification, or provider cost accounting.
