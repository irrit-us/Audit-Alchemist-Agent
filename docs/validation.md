# Validation

## Live tiny-dataset baseline round 2: 2026-10-09

Two harness fixes from round 1: chat-completions now sets `response_format =
{"type":"json_object"}`, and `--reasoning-effort low` bounds reasoner output.
All Rust tests pass (53 unit, 107 total). Re-ran the same blind instruction and
limits, with up to two trials for cases that failed with an execution error.
Full metrics are in [the round-2 report](reports/tiny-baseline-round2.json).

| Metric | Round 1 | Round 2 |
| --- | --- | --- |
| Successful cases | 2/7 | 3/7 (best of <=2 trials) |
| Expected-line hits (any CWE) | 1/7 | 3/7 |
| Exact TP / FP / misses | 1 / 3 / 6 | 1 / 6 / 6 |
| Exact precision / recall / F1 | 0.25 / 0.143 / 0.182 | 0.143 / 0.143 / 0.143 |

The fixes removed the prose-before-JSON failure and let two more fixtures
complete. Semantic recall improved: `filelock-toctou` now reports the expected
line 41 (as `CWE-367`, not the expected `CWE-59`) and `pymonocypher-overflow`
reports the expected line 345 (as `CWE-476`, not `CWE-122`). Exact F1 did not
improve because those runs also add false positives and the CWE taxonomy
differs; four fixtures (fast-jwt, thin-vec, zk-email, ajna) still had no
successful trial in two attempts. Remaining failures are execution errors at the
model phase (reasoner truncation or a transient stream error), not detection
decisions. This remains a tuning baseline, not a quality claim.

## Live tiny-dataset baseline round 1: 2026-10-09

Ran the built-in agent over the seven real-world fixtures from the `datasets/tiny`
submodule with a blind, hint-free instruction; `evaluate` staging excludes each
manifest, so no case ID or label reached the model. Model `deepseek-flash`
(chat-completions), `--max-tokens 32768`, `--max-tool-calls 16`, one attempt per
case. Full metrics are in [the baseline report](reports/tiny-baseline-round1.json).

| Metric | Value |
| --- | --- |
| Successful cases | 2/7 (`filelock-toctou`, `h11-chunked-framing`) |
| Execution failures | 5/7 |
| True / false positives / misses | 1 / 3 / 6 |
| Precision / recall / F1 | 0.25 / 0.143 / 0.182 |
| Trajectory rounds / tool calls | 53 / 79 |
| Tokens (input / output / reasoning) | 847,227 / 65,179 / 54,676 |

The single exact match was h11 (`CWE-444`, `h11/_readers.py:197`).
`filelock-toctou` was a near miss: three `CWE-59` findings at lines 44/28/22
instead of the expected line 41. The five failures are execution failures, not
detection decisions: `deepseek-flash` is a reasoning model that can exhaust the
output-token cap (`finish_reason: length`), and several runs hit a transient
model-stream error or the tool budget before emitting the required JSON object.
The blind instruction is deliberately unscoped, so this measures a hard setting
and is a tuning baseline, not a quality claim. A `deepseek-chat` probe showed the
same strict-JSON failure mode (prose before the JSON object).

## Caller-side bridge: 2026-10-09

`tests/bridge.rs` drives three `alchemist audit` nodes (discovery, verification,
reporting) through a caller-side bridge against a mock chat-completions provider.
The audited data is the committed Ajna fixture at
`datasets/tiny/ajna-protocol-compromise-2/audit`, staged with the harness's own
`dataset::stage_workspace` so the repository copy stays read-only; the case
target, instruction, and expected finding come from that manifest. The test
asserts that all three requests carry the same `prompts/audit.txt` base prompt,
that each node receives its own brief instruction and no later brief, and that
discovery and verification findings are threaded into the later nodes.
Each node also performs one `read_file` tool call, so the bridge reads that
node's run journal and pushes a metrics record on completion: two trajectory
rounds, one tool call, provider-reported input/output token usage, and result
accuracy (precision/recall/F1 against a one-key expected set). Discovery's
planted false positive drives its precision to 0.5 while verification and
reporting reach 1.0. The pushed records are appended to `metrics.jsonl` per node
and returned to the caller. The `real_world_datasets_validate` harness test loads and validates all seven
fixtures from the submodule. All 105 Rust tests passed (52 unit, 53 integration),
with formatting and strict Clippy clean on `1.95.0-x86_64-pc-windows-msvc` using
the local `.mozbuild` VC tools and Windows SDK 10.0.26100.0. The bridge is
caller-side test code: the harness still owns one bounded node, and no
orchestration or metrics sink was added to `alchemist`. No live model or network
request was used.

## CLI node configuration and MCP: 2026-10-08

All 98 Rust tests passed locally on Windows/MSVC, including real CLI tests for
TOML precedence, config-relative paths, boolean overrides, custom skill loading,
disabled-tool dispatch, evaluation snapshots, MCP continuation on all three
provider wires, and whole-run cancellation during server initialization.
Local MCP fixtures exercise pagination, stderr draining, server requests,
tool errors, malformed/oversized messages, protocol failures, request timeouts,
and descendant cleanup. Formatting and strict Clippy passed. The debugger suite
passed seven checks, with four skips for optional GDB/pwntools/Foundry checks.
The example TOML validates and dry-runs without credentials or MCP startup.
Python 3 is required only for the test fixture and is provisioned in both Rust CI
jobs; the harness gains no Python runtime dependency. No live MCP service or
live-model discovery-quality evaluation was performed in this round.

The first hosted Windows run exposed a fixture timing assumption: the 800 ms
whole-run deadline could expire before Python finished cold startup. The
cancellation fixture now allows startup time, asserts descendant readiness,
and releases a would-be leak only after the CLI has exited. Production deadline
and cleanup code did not change for this correction.

## Windows HTTP fixture correction: 2026-10-08

The first matrix run exposed `WouldBlock` in the Windows mock-provider request
reader. The three HTTP fixtures polled nonblocking listeners but assumed
accepted streams were blocking. They now explicitly restore blocking mode and
set both read and write timeouts. A regression test forces nonblocking mode on
every platform and delays two request fragments until the server is ready.
All 90 Rust tests, formatting, and strict Clippy passed locally on Windows/MSVC.
This changes test infrastructure only; production Bash and provider behavior
are unchanged. See [Winsock accept semantics](https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-accept).

## Process and provider regression coverage: 2026-10-08

All 89 Rust tests passed locally on Windows/MSVC, along with formatting and
strict Clippy. New checks cover cancellation after descendant readiness,
background cleanup after successful Bash completion, simultaneous large stdout
and stderr with retained tails, rejected arguments without file mutations,
duplicate tool IDs on all provider wires, and Unicode SSE data split at every
byte boundary. Subprocess failure checks previously gated to Unix now run on
Windows too. The incomplete-stream fixture now permits its full four-call
batch, ensuring a budget failure cannot hide missing stream validation, and
also covers Responses and Anthropic incomplete turns.

The debugger suite passed nine checks with two local dependency skips (GDB and
pwntools); Node and opt-in Foundry checks ran. Strict dependency mode correctly
failed for those missing tools. No live-model quality evaluation or line-coverage
percentage was measured.

CI now runs independent Linux and Windows Rust jobs, plus a Linux debugger job
that requires Node, GDB, and pwntools. Jobs have wall-clock bounds and read-only
repository permissions; matrix failures do not cancel the other platform's
diagnostics. This follows [GitHub's matrix guidance](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/run-job-variations).
Lifecycle checks exercise the cleanup obligations described in
[Tokio's process documentation](https://docs.rs/tokio/latest/tokio/process/index.html).
Local results above do not assert that a particular hosted CI run passed.

## Debugging skills and concise defaults: 2026-10-08

Validated on Windows with Rust 1.95.0 (MSVC): all 81 Rust tests, formatting,
strict Clippy, and `git diff --check` passed. All ten skills passed frontmatter
validation. Debugger scripts ran 11 checks: nine passed; native GDB and pwntools
checks were skipped because those dependencies were absent. Node Inspector and
opt-in Foundry tests ran, including import-free helpers under Solidity 0.6.12
and 0.8.26 and typed helpers against the local forge-std test checkout. Linux CI
installs GDB and pwntools for its script checks; this record is a local result.

The audit prompt shrank from 3,475 to 1,882 UTF-8 bytes and the compact ten-skill
catalog from 1,863 to 1,195 bytes (about 42% combined). AGENTS.md shrank from
2,506 to 1,293 bytes. Measurements normalize line endings and exclude source
context, tool definitions, provider framing, and on-demand resources. These
are text-size reductions, not tokenizer measurements or demonstrated audit
quality gains. No live-model quality evaluation was performed for this change.

## Tool reliability follow-up: 2026-10-08

The second pass uses the same Windows/MSVC/Git Bash environment and validation
commands below. It adds streaming reads of files over 1 MiB, exact page citation
checks, CRLF/empty/unterminated-line coverage, oversized-line and scan-budget
rejection, UTF-8 capture across buffer boundaries, and bounded head/tail storage.
The timeout fixture now verifies that both partial output streams survive while
the descendant process is still terminated. No live-model quality measurement
was performed in this pass.

All 69 tests passed (41 unit, 28 integration); formatting, strict Clippy, and
`git diff --check` also passed.

## Tool-loop validation: 2026-10-08

Validated locally on Windows with Rust 1.95.0 (MSVC) and Git Bash:

```text
cargo +1.95.0-x86_64-pc-windows-msvc fmt --all -- --check
cargo +1.95.0-x86_64-pc-windows-msvc clippy --locked --all-targets -- -D warnings
cargo +1.95.0-x86_64-pc-windows-msvc test --locked --all-targets
64 tests passed (37 unit, 27 integration)
```

The local HTTP fixtures verify a complete read-file → write-PoC → Bash-execution
→ final-finding exchange for chat-completions, Responses, and Anthropic. Checks
cover fragmented tool arguments, native result IDs, encrypted Responses reasoning,
Anthropic thinking signatures, final-answer phase selection, duplicate-call
rejection, incomplete streams, tool-call limits, paged citation provenance,
nonzero Bash exits, output truncation, timeout descendant cleanup, writable
evaluation isolation, and credential-free directory previews. The exact search
byte-boundary regression is covered. Existing protocol, scoring, provider, and
deterministic smoke tests also pass.

No live-model requests were used for this change. These checks establish tool
and protocol behavior, not improved vulnerability discovery quality or token
efficiency. The Linux-specific process tests were not run on this Windows host.

## Historical live evaluation: 2026-10-07

> Historical record. This run predates the bounded transport retries and the
> Sign In With ChatGPT adapter; it remains the recorded chat-completions smoke
> result.

The live evaluation used credentials from `/home/ubuntu/.codex/deepseek.config.toml`, read privately and passed as `AUDIT_API_KEY` to the harness process. No credential was printed, persisted in this repository, or added to command arguments. The local profile was left unchanged. The model selection was `deepseek-flash`; its provider URL was adapted to `https://api.deepseek.com/chat/completions` for a plain LLM request.

Run configuration:

```sh
./target/debug/audit-harness evaluate \
  --dataset datasets/smoke/dataset.json \
  --endpoint https://api.deepseek.com/chat/completions \
  --model deepseek-flash --jobs 2 --timeout-ms 120000 \
  --output deepseek-evaluation.json
```

The credential environment must be supplied privately before reproducing this command; choose a new output filename because the harness refuses overwrites. Defaults: 256 KiB source context, 4,096 maximum requested output tokens, 1 MiB stdout/stderr limits. Each of the six cases made one request. No retries or output repairs were performed, and the prompt did not include ground truth. The run did not override DeepSeek's default thinking setting.

| Result | Value |
| --- | ---: |
| Successful cases | 6 / 6 |
| Exact-match cases | 6 / 6 |
| True positives | 3 |
| False positives | 0 |
| False negatives | 0 |
| Precision / recall / F1 | 1.0 / 1.0 / 1.0 |
| Total wall-clock duration | 10.702 s |

All three corrected counterparts produced no findings. The unsafe shell, SQL, and eval cases produced the expected CWE, path, and sink line. The full findings and individual timings are in [deepseek-evaluation.json](reports/deepseek-evaluation.json).

This is one run on six small handcrafted cases with three vulnerability families. It establishes working provider integration and correct results on the supplied smoke set. It does not establish general vulnerability discovery effectiveness, repeatability, calibrated severity, or robustness to prompt injection. A broader held-out dataset and repeated measurements remain the appropriate next validation step. Token usage and cost were not captured.

Local checks passed with stable Rust 1.99.0:

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
11 integration tests passed
```

Tests cover strict response schemas, matching and duplicate suppression, failed-case scoring, dataset validity, source bounds, nested symlink exclusion, report overwrite protection, process failures and excessive output, deadline/cancellation descendant cleanup on Linux, deterministic demo evaluation, HTTP authentication/request scoping, API failures, truncated completions, and out-of-snapshot findings.

SHA-256 fingerprints at validation:

```text
f0b9d369fab9d6983948c48604e5b1ee7f84b73e5ce4ef8fe2b57fb1d5d67b98  Cargo.lock
e5a232d5710d050ed6090c5f3f9888ccc1b4d905fcfb45da174986e81ef20d52  prompts/audit.txt
78ad6f862ea38f31ebced76663fed2483c3856990829e2cdf0ce765d4e1e49db  datasets/smoke/dataset.json
aae6cc3c771f9ed152b4e44ab5bfe1b89c8abdbd2e770b448aaf6ea52f9967a1  datasets/smoke/sources/command_safe.py
0bddaa2baf3f9d6b80270ad8927d9ac01b0ebec1f879bb153b1614df8e780575  datasets/smoke/sources/command_unsafe.py
9ce6d0974781dcb7840c2e08ed60b2a6faf1c4ea2109690dd4fee9a8747c0597  datasets/smoke/sources/eval_unsafe.py
4983c0ca231983478b4b0d43127a5ef159f5df5c1ed0ef6ecbfdeb630ce66b9b  datasets/smoke/sources/literal_safe.py
eb22131678fc7dca12d25da9f4cfc9a640bdec081eae2f9f38a297ee5ef7b9ad  datasets/smoke/sources/sql_safe.py
65ebcd527f8cd77f70ccaa8c0fd000b845713d3c27fbaadcd5c922ebead89a43  datasets/smoke/sources/sql_unsafe.py
```
