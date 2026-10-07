# Public constraints and design decisions

This project starts from a plain LLM API rather than a specific vendor's agent CLI. Its public integration contract is intentionally small: one source snapshot, one chat-completions request, one versioned JSON response. The task is static vulnerability discovery; models do not invoke tools or execute supplied source.

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

These decisions follow the documented interfaces of [clap derive](https://docs.rs/clap/latest/clap/_derive/_tutorial/index.html), [Serde container attributes](https://serde.rs/container-attrs.html), [Tokio process management](https://docs.rs/tokio/latest/tokio/process/index.html), [tracing-subscriber formatting](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/index.html), and [reqwest client configuration](https://docs.rs/reqwest/0.12/reqwest/struct.ClientBuilder.html). Serde flattening with unknown-field rejection is avoided in the final contract implementation.

For the live validation, the local Codex profile supplied the model, provider URL, and bearer credential; the profile itself was not changed. [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference) documents provider settings. The harness calls the plain [DeepSeek chat-completions API](https://api-docs.deepseek.com/api/create-chat-completion/) directly, independently of the profile's Codex wire protocol. It preserves the profile's `deepseek-flash` model selection and the provider's default thinking behavior. Temperature zero is requested but does not control sampling in DeepSeek thinking mode, as documented by the provider.

The smoke labels use the public [CWE-78](https://cwe.mitre.org/data/definitions/78.html), [CWE-89](https://cwe.mitre.org/data/definitions/89.html), and [CWE-95](https://cwe.mitre.org/data/definitions/95.html) taxonomy. The samples are original, synthetic source fixtures, not downloaded benchmarks. Corrected variants use argument-vector process execution, SQL parameter binding, and literal parsing respectively. The literal-parsing case explicitly limits input upstream and asks about arbitrary code execution; it is not a claim that literal parsing eliminates every denial-of-service risk.

Tokio is justified by concurrent pipe draining, deadlines, HTTP I/O, and bounded evaluation workers. Ratatui is omitted because machine-readable reports and tracing meet the initial workflow. No persistent service, agent framework, database, or unbounded task queue is added.

Optimization should proceed from measured results: run the smoke set with a chosen provider, inspect false positives and misses, update the focused prompt or add representative cases, then rerun under the same model and budgets. Keep changes isolated and compare execution failures separately from precision/recall. Expand to a held-out dataset before making claims about general discovery effectiveness. This version does not implement automatic prompt search, exploit verification, provider cost accounting, or retries.
