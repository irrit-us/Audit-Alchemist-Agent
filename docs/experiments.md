# Experiment log and accumulated experience

A durable record of the live optimization rounds for the tiny audit benchmark.
Each entry states a hypothesis, the single variable changed, the comparable
budgets, all trials retained, and a post-hoc review of non-matching findings.
Negative results are kept so later rounds do not repeat them.

Method and commands are in [Evaluation](evaluation.md); the primary-source
rationale is in [Harness design research](harness-design.md); per-round evidence
is in [Validation](validation.md).

## Capability levels

Exact scoring is `(CWE, path, line)`. Because the model frequently finds the
right defect at an adjacent sink or under a defensible alternate CWE, two
distinct metrics are reported alongside it and never folded into exact F1:

- **line**: `(path, line)` only. Separates "found the sink" from "chose the
  expected CWE".
- **reviewed**: `unexpected_valid` from per-finding review
  (`alternate_match` + `valid_distinct`). Separates "reported a genuine issue"
  from "reported the labeled key".

## Ledger

| Round | Change under test | Completion | Exact F1 | Reviewed valid | Lesson |
| --- | --- | --- | ---: | ---: | --- |
| 1 | Blind instruction, 2 MiB stream cap | 2/7 | .182 | 3 | Reasoning SSE overran the cap; blind discovery is hard. |
| 2 | `response_format=json_object`, effort `low` | 3/7 | .143 | 6 | JSON mode removed prose-before-JSON; cap still aborted runs. |
| 3 | `--max-stream-bytes` 8 MiB | 6/7 | .125 | 16 | The cap, not detection, was the completion bottleneck. |
| 4 | Threshold prompt vs old prompt | 12/14 each | .182 / .065 | 15 / 14 | Threshold cheaper (~25%); exact F1 is label-bound and noisy. |
| 5 | Force a final turn at budget exhaustion | 12/14, 13/14 | .133 / .214 | 12 / 9 | Recovery converts exhausted runs into reports. |
| 6 | Bounded final-output repair (0 vs 2) | 10/14, 14/14 | .231 / .214 | 7 / 11 | Completion up; the repair path was not hit by chance. |
| 7 | Targeted risky cases (4x3) | 12/12, 11/12 | .000 / .087 | unreviewed | Exposed the empty-completion failure class. |
| 8 | Empty-completion retry | 11/12, 12/12 | .000 | unreviewed | Removed the round-7 failure class. |
| 9 | Focused `sha256` (1x6) | 6/6 both | .000 | unreviewed | Repaired 2 live malformed reports; direct evidence. |
| 10 | Tool budget 16 vs 24 | 14/14 both | .114 / .062 | 17 / 15 | +40% tokens, no supported-finding gain. Larger budget rejected. |
| 11 | Expanded 18-case validation | 17/18 | .432 | 12 | Clean control empty; found the over-budget-batch failure. |
| 12 | Reject over-budget batch, then finalize | 18/18 | .457 | 10 | Recovery fixed the round-11 failure; 3 repairs fired live. |
| 13 | Tool budget 16 vs 12 | 14/14, 13/14 | .121 / .125 | 15 / 16 | -31% tokens, exact TP unchanged, line TP 4 to 3; promising, needs replication. |
| 14 | Budget 16 vs 12, 3 trials | 21/21, 19/21 | .041 / .143 | 21 / 16 | -32% tokens; line F1 .122 to .286. Two model-side failures, not budget. Cost lever confirmed. |
| 15 | Context cap 2 MiB vs 128 KiB | 14/14, 10/14 | .125 / .000 | 14 / 11 | Aggressive pruning fails 4/14 with `conversation exceeds` and loses recall; rejected. |
| 16 | Stable 18-case report, 2 trials | 36/36 | .329 | 27 | Line F1 .493; clean control empty; 6 repairs fired. Headline capability. |

## Accumulated experience

1. **Completion and robustness are cheap to fix and worth fixing first.** The
   largest gains came from removing transport/validation walls: stream cap
   (rounds 2-3), limit forwarding (round 4), tool-budget exhaustion (rounds 5,
   11-12), and invalid/empty final output (rounds 6-9). Every one of these was a
   harness defect, not a model capability limit.

2. **Exact F1 is dominated by label and sink choice, not by missed discovery.**
   In every round, `unexpected_valid` exceeds exact `tp` by a large factor
   (e.g. round 4: 3 TP vs 15 valid; round 10: 2 vs 17). Reviews classify nearly
   all non-exact matches as alternate sinks or genuine separate issues; only a
   handful are invalid. Exact precision therefore understates capability.

3. **The line metric sits between the two.** Historical line-only F1 is
   consistently higher than exact F1 (round 4: .194 to .333; round 10: .114 to
   .286; round 12: .457 to .588). Most of the remaining exact loss is CWE
   taxonomy, which is a project label choice and should not drive model tuning.

4. **The budget curve is asymmetric.** Round 10 showed 24 tool calls cost ~40%
   more tokens with no gain. Round 13 showed 12 calls cut total tokens 31%
   (4.77M to 3.30M) with exact TP unchanged (2 vs 2), though line TP slipped 4
   to 3 and one run hit the stream cap. A smaller budget is the more promising
   direction, but n=14 cannot confirm it; replicate before changing a default.

5. **Recovery strategies are insurance, not quality levers.** Output repair,
   empty-completion retry, and over-budget finalization fire sporadically (0-3
   times per 12-18 runs). They prevent run loss; they do not change what the
   model finds.

6. **Prompt tokens dominate cost (95-97% of total).** Rounds 6, 10, and 12 all
   spend 95%+ of tokens resending conversation history. Reasoning effort and
   completion caps cannot move the needle; only fewer turns or a smaller
   per-turn context can.

7. **Context pruning is inactive at the default and dangerous when forced.**
   Request sizes are median ~69 KB and max ~329 KB against a 2 MiB cap, and no
   `context_pruned` event fired. A 128 KiB cap saved ~49% of tokens but failed
   4/14 runs with `conversation exceeds --max-context-bytes` and dropped exact
   TP to zero. Turn count, not context retention, is the cost lever.

8. **The clean control and per-case threat models work.** The
   `unsigned-opt-in-control` case produced zero findings under both the blind
   prompt and its own instruction. Giving cases their own scope (rounds 11-16)
   roughly doubles exact F1 versus the blind instruction, confirming that
   label/instruction alignment, not capability, explains the blind baseline.

9. **Stable capability on the promoted set:** 36/36 runs completed, exact
   P/R/F1 .308/.353/.329 and line P/R/F1 .462/.529/.493 (round 16, two trials per
   case). This is the number to compare future changes against.

10. **Candidate tools are retired on evidence.** Across rounds 4-16, `bash`
    accounts for 490 of 509 tool errors (96%) and `edit_file` was called once;
    `write_file` errors are rare. There is no edit-failure cluster to justify
    `apply_patch`, and no observed loss of task state to justify `update_plan`.

11. **Small samples cannot resolve 1-2 finding differences.** Fourteen to
    twenty-one trials with provider variance produce swings larger than most
    interventions. Decisions should rest on recovery counters and aggregate
    reviewed findings, not on single-round F1 ordering.

12. **Deterministic tests plus one live demonstration is the right evidence
    standard.** Contract changes (tool budget, repair, retry, citation
    fingerprint) are pinned by mock-provider tests; a live round is used only to
    show the class occurs and the path fires.

## Decision policy for future rounds

- Fix completion/contract failures before tuning quality; a run that dies cannot
  score.
- State one variable and one success criterion before running.
- Prefer metrics robust to labels: completion, recovery counters, `line_f1`,
  `unexpected_valid`.
- Reject changes that raise tokens without raising supported findings.
- Do not claim quality from a single 14-trial round; report variability and keep
  negative results.
- Add a new tool or orchestration layer only with an observed failure and a
  deterministic test.

## Outcomes of the first five directions

1. **Smaller budget: confirmed (round 14).** A 12-call budget cut ~32% of tokens
   with equal-or-better exact and line F1 across rounds 13-14. The two failures
   were a provider stream cap and an unobserved-line citation, not the budget.
   The global CLI default stays 32 because the tiny benchmark cannot justify a
   real-world default cut (H11); evaluation plans should use 12-16.
2. **Context reduction: rejected (round 15).** Pruning never fires at the 2 MiB
   default; a 128 KiB cap raised failures and lost recall. Turn count is the
   lever, so direction 1 is the way to cut cost.
3. **Stable reporting: done (round 16).** 36/36 completion, exact F1 .329, line
   F1 .493 over two trials per case. This is the baseline for future changes.
4. **Candidate tools: retired on evidence.** No `apply_patch` or `update_plan`
   evidence cluster exists; `bash` dominates errors and edits are rare.
5. **Exact keys: unchanged.** Line and reviewed metrics explain near misses.

## Recommended next directions

1. **Investigate the remaining execution failures.** Completion is 36/36 on the
   promoted set but lower on the blind budget rounds; the observed classes are
   stream-cap (`reasoning` SSE over 8 MiB) and repeated unobserved-line citations.
   Add run-level diagnostics and test whether a larger stream cap or clearer
   repair feedback removes them. Accept only on measured completion gains.
2. **Reduce turn count without a tighter context cap.** The 12-call result shows
   fewer turns are cheaper; test an earlier "finalize now" prompt or a tighter
   per-turn tool-result budget against line recall, not a context cap.
3. **Broaden held-out validation.** The promoted set is now a regression suite;
   add cross-file, injected-instruction, and safe-control cases that do not map
   to the already-reviewed sinks, then re-measure exact, line, and reviewed
   metrics at 2-3 trials.
