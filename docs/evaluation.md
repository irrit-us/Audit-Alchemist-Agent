# Evaluation

Evaluation runs the built-in agent (or an external benchmark agent) over a
labeled dataset and scores the reported findings. The request, response, and
dataset contracts are in [Protocol](protocol.md).

## What the model sees

Ground-truth labels and case IDs are excluded from the LLM prompt. Small target
files are initially supplied with line labels; tools can explore additional
files and run PoCs. Findings must cite a line supplied initially or by read_file.

`evaluate` stages a separate writable temporary workspace per case, excluding
the dataset manifest, nested symlinks, and dependency/build directories. Copies
are bounded to 10,000 entries and 64 MiB. Source and configuration files are
preserved, including executable permissions; PoC artifacts are removed with the
temporary workspace after the case. This prevents ordinary cross-case mutation
and accidental reading of the label manifest. Bash still has host permissions;
for strict held-out evaluation, use an OS/container boundary that hides labels
and unrelated files. `benchmark` retains its external-agent working-directory
contract and does not stage copies.

## Matching and scoring

Matching is exact on `(CWE, source-root-relative path, 1-based sink line)`.
Duplicate predictions count once. For each case the report lists:

- `matched`: expected findings the agent reported.
- `unexpected`: reported findings that were not expected (false positives).
- `missed`: expected findings the agent did not report (false negatives).
- `duplicate_findings`: repeated prediction keys collapsed to one.

Micro precision, recall, and F1 aggregate across cases. Undefined ratios are
JSON `null`. Failed invocations retain all expected findings as misses and
cannot count as exact matches. Clean cases measure false-positive behavior
through unexpected findings and exact matches.

Exact line matching is intentionally strict; assess near misses separately before
changing matching policy. Each case can perform multiple model/tool turns within
the configured call, context, and time limits. Failed
cases retain expected findings as misses and therefore can lower recall;
precision still depends on reported predictions. Failures do not change a
completed evaluation's exit status.

## Smoke set

The six handcrafted samples cover CWE-78, CWE-89, and CWE-95 with paired
mitigations. They validate basic evaluation behavior, not representative
vulnerability coverage. A historical single-request `deepseek-flash` run completed all six cases with
3 true positives, no false positives or misses, and F1 = 1.0. See
[validation results](validation.md) and the [full report](reports/deepseek-evaluation.json).
That result predates the tool loop and does not measure its discovery quality.

## Unexpected-finding verification

Exact scoring is deliberately strict, so a run's non-matching findings are
classified separately after a live round. `reports/tiny-unexpected-verification.json`
records, for each unexpected finding, whether it is the same root cause at
another sink line or label (`alternate_sink`/`alternate_label`), a genuine
separate issue (`valid_distinct`), genuine but low impact (`valid_low_impact`),
or outside the case boundary (`out_of_scope`). Verified-genuine findings are
promoted into dataset cases. This classification is analysis metadata: the
harness still scores exact keys and does not silently relax them. The aggregate
`unexpected_valid`/`unexpected_invalid` counts are written to
`reports/tiny-metrics-log.jsonl` for round-over-round comparison.

The default system prompt asks for reachable root causes that break a security
property and to omit low-impact hardening and unproven observations. This is a
default-configuration filter, not a scoring change.

## Real-world fixtures

Seven real-world fixtures live in the `alchemist-dataset-tiny` submodule at
`datasets/tiny` (see its README). Each keeps the audited code under `audit/` and
the fix/evidence under `reference/`, so evaluation staging copies only the
audited code:

| Fixture | Class / key |
| --- | --- |
| `ajna-protocol-compromise-2` | accounting / reward manipulation (`CWE-841`, `ajna-v2/src/libraries/external/TakerActions.sol:168`) |
| `filelock-toctou` | TOCTOU symlink truncation (`CWE-59`, `filelock/_unix.py:41`) |
| `fast-jwt-iss` | issuer-array validation (`CWE-290`, `src/verifier.js:159`) |
| `thin-vec-uaf` | Rust double free on unwind (`CWE-415`, `src/lib.rs:2546`) |
| `pymonocypher-overflow` | cross-language heap overflow (`CWE-122`, `c_monocypher.pyx:345`) |
| `h11-chunked-framing` | chunked-body parser disagreement (`CWE-444`, `h11/_readers.py:197`) |
| `zk-email-sha256` | standalone SHA256 comparator soundness (`CWE-1284`, `sha.circom:126`) |

Each `README.md` records the upstream advisory, vulnerable and fixed versions,
and the scope boundary (notably that the fast-jwt, h11, and zk-email fixtures
are narrower than a full compromise). The labels are deliberate choices among
defensible CWEs and adjacent sink lines; treat them as tuning data. No live-model
result for these fixtures is recorded yet.

## Reproducibility

Record the model, prompt, dataset revision, limits, and repeat runs when
comparing changes. Temperature zero does not guarantee reproducibility across
providers. Run journals collect provider-reported token usage, but missing
usage currently appears as zero and is not a billing estimate. Evaluation
reports do not aggregate token/cost comparisons or automatic source hashes;
pin code and dataset commits for comparisons. Follow the
[optimization comparison procedure](harness-design.md#how-to-assess-an-optimization)
before claiming quality or efficiency gains. The deterministic
`demo-agent` fixture recognizes three simple source patterns and is a plumbing
check, not evidence of LLM discovery quality.
