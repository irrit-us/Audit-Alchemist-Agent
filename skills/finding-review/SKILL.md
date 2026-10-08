---
name: finding-review
description: Review finding evidence, duplicate root causes, impact, and final JSON.
---

# Finding review

For each finding, verify a concrete root cause, attacker-controlled entry point, reachable vulnerable operation, and consequence. Recheck mitigating guards and assumptions. Separate observed runtime behavior, established code facts, and remaining inference. Discard disproven leads; do not preserve their original severity in a later summary.

Group duplicate symptoms of one root cause unless they require independent fixes. Calibrate severity to the demonstrated consequence and prerequisites, not to the most dramatic theoretical outcome. A crash or sanitizer report alone does not establish code execution. A static argument may still establish a bug when its reachability and consequence are clear; describe its validation limits honestly.

Use read_file to obtain the exact source line for each citation. Cite original vulnerable code, not a generated PoC or an unrelated caller. After source edits, reread affected locations. Include concise reproduction evidence in the finding description when available.

Return only the versioned JSON schema required by the system prompt. Do not add numeric scores, custom fields, Markdown reports, or fixed filesystem output locations. An empty findings array is valid when no concrete vulnerability is established.

Adapted from 0RAYS/codex-auditor overall-report-skill evidence review principles; see ../LICENSE.codex-auditor and ../UPSTREAM.md in the source distribution.
