# Evaluation

Evaluation runs the built-in agent (or an external benchmark agent) over a
labeled dataset and scores the reported findings. The request, response, and
dataset contracts are in [Protocol](protocol.md).

## What the model sees

Ground-truth labels and case IDs are excluded from the LLM prompt. The source
snapshot contains only the case target, with normalized paths and source text;
expected findings never enter model context. Findings must reference a file in
the snapshot and an existing line.

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
changing matching policy. Each case performs at most one model request. Failed
cases lower precision/recall but do not change a completed evaluation's exit
status.

## Smoke set

The six handcrafted samples cover CWE-78, CWE-89, and CWE-95 with paired
mitigations. They validate basic evaluation behavior, not representative
vulnerability coverage. A live `deepseek-flash` run completed all six cases with
3 true positives, no false positives or misses, and F1 = 1.0. See
[validation results](validation.md) and the [full report](reports/deepseek-evaluation.json).

## Reproducibility

Record the model, prompt, dataset revision, limits, and repeat runs when
comparing changes. Temperature zero does not guarantee reproducibility across
providers. API usage and cost, and automatic source hashes, are not collected;
pin code and dataset commits for comparisons. The deterministic
`demo-agent` fixture recognizes three simple source patterns and is a plumbing
check, not evidence of LLM discovery quality.
