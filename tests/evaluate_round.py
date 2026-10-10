"""Regression tests for evidence review and all-trial evaluation accounting."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("evaluate_round", Path(__file__).resolve().parents[1] / "scripts/evaluate_round.py")
ROUND = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ROUND)


class ReviewTests(unittest.TestCase):
    def setUp(self):
        self.finding = dict(cwe="CWE-59", path="lock.py", line=44, severity="high",
                            title="Symlink truncation", evidence="Attacker swaps a shared lock path before open.")
        self.expected = dict(cwe="CWE-59", path="lock.py", line=41)
        self.case = dict(run=dict(case_id="lock", outcome="success", elapsed_ms=10,
                                 findings=[self.finding]), matched=[], missed=[self.expected],
                         unexpected=[self.finding])
        self.rid = "report:lock:" + ROUND.identity(self.finding)

    def test_unreviewed_is_not_invalid(self):
        counts, queue = ROUND.review_counts(self.case, {}, "report")
        self.assertEqual(counts["unexpected_unreviewed"], 1)
        self.assertEqual(counts["unexpected_invalid"], 0)
        self.assertEqual(queue[0]["review_id"], self.rid)

    def test_exact_matches_and_duplicates_are_excluded(self):
        self.case["run"]["findings"] += [self.finding, self.expected]
        counts, _ = ROUND.review_counts(self.case, {}, "report")
        self.assertEqual(counts["unexpected_total"], 1)

    def test_review_requires_full_claim_and_report_identity(self):
        reviews = {self.rid: dict(verdict="alternate_match", evidence="Same open operation, adjacent sink.")}
        counts, _ = ROUND.review_counts(self.case, reviews, "report")
        self.assertEqual(counts["unexpected_valid"], 1)
        self.assertEqual(counts["alternate_matches"], 1)
        self.assertEqual(ROUND.review_counts(self.case, reviews, "different-report")[0]["unexpected_unreviewed"], 1)
        self.finding["evidence"] = "Different unsupported claim at the same line."
        self.assertEqual(ROUND.review_counts(self.case, reviews, "report")[0]["unexpected_unreviewed"], 1)

    def test_scope_and_low_impact_remain_separate(self):
        for verdict, field in (("out_of_scope", "unexpected_out_of_scope"), ("low_impact", "unexpected_low_impact")):
            counts, _ = ROUND.review_counts(self.case, {self.rid: dict(verdict=verdict, evidence="Reviewed prerequisites.")}, "report")
            self.assertEqual(counts[field], 1)
            self.assertEqual(counts["unexpected_valid"], 0)
            self.assertEqual(counts["unexpected_invalid"], 0)

    def test_verdict_requires_evidence(self):
        with self.assertRaises(ValueError):
            ROUND.review_counts(self.case, {self.rid: dict(verdict="valid_distinct")}, "report")

    def test_failed_run_does_not_accept_findings(self):
        self.case["run"]["outcome"] = "timeout"
        self.assertEqual(ROUND.review_counts(self.case, {}, "report")[0]["unexpected_total"], 0)

    def test_per_variant_limits_override_shared(self):
        plan = {"limits": {"model": "m", "max_output_repairs": 2},
                "limits_by_variant": {"baseline": {"max_output_repairs": 0}}}
        self.assertEqual(ROUND.variant_limits(plan, "baseline")["max_output_repairs"], 0)
        self.assertEqual(ROUND.variant_limits(plan, "candidate")["max_output_repairs"], 2)

    def test_case_instruction_can_be_kept_for_validation_rounds(self):
        data = {"cases": [{"id": "c", "instruction": "case text", "expected": []}]}
        plan = {"instruction": "blind plan text"}
        self.assertEqual(ROUND.select_case(data, "c", plan)["instruction"], "blind plan text")
        kept = ROUND.select_case(data, "c", {**plan, "use_plan_instruction": False})
        self.assertEqual(kept["instruction"], "case text")

    def test_dropped_child_limits_reject_comparison(self):
        with self.assertRaisesRegex(ValueError, "did not forward"):
            ROUND.verify_forwarded_limits({"args": ["agent"]}, {"reasoning_effort": "low"})
        ROUND.verify_forwarded_limits({"args": ["agent", "--reasoning-effort", "low"], "jobs": 1},
                                      {"reasoning_effort": "low", "jobs": 1})

    def test_round_log_is_compact_and_append_only(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            ROUND.write_json(root / "manifest.json", {"started_utc": "2026-10-09T00:00:00+00:00",
                "plan": {"trials": 2, "workers": 4, "limits": {"model": "m", "max_tokens": 1,
                "max_tool_calls": 2, "reasoning_effort": "low", "max_stream_bytes": 8}}})
            summary = {"groups": {"candidate": {"trials": 2, "success": 1, "failed": 1, "timeouts": 0,
                       "tp": 1, "fp": 0, "fn": 1, "precision": 1.0, "recall": 0.5, "f1": 0.666,
                       "operational_totals": {"turns": 3, "tools": 4, "tool_errors": 1, "retries": 0,
                       "prompt_tokens": 5, "completion_tokens": 6, "reasoning_tokens": 0, "total_tokens": 11},
                       "unexpected_total": 1, "unexpected_valid": 1, "unexpected_invalid": 0,
                       "unexpected_unreviewed": 0, "unexpected_low_impact": 0, "unexpected_out_of_scope": 0,
                       "alternate_matches": 1, "valid_distinct": 0}},
                       "case_metrics": [{"run": "c-1-candidate", "variant": "candidate",
                       "unexpected_total": 1, "unexpected_valid": 1, "unexpected_invalid": 0,
                       "unexpected_unreviewed": 0, "unexpected_low_impact": 0, "unexpected_out_of_scope": 0,
                       "alternate_matches": 1, "valid_distinct": 0}],
                       "review_queue": [{"run": "c-1-candidate", "variant": "candidate", "verdict": "alternate_match",
                       "finding": {"cwe": "CWE-59", "path": "lock.py", "line": 44}}]}
            log = root / "log.jsonl"
            ROUND.append_log(log, ROUND.log_records(root, summary, 4))
            lines = log.read_text().splitlines()
            self.assertEqual(len(lines), 2)
            self.assertEqual(json.loads(lines[0])["round"], 4)
            self.assertEqual(json.loads(lines[1])["findings"][0]["verdict"], "alternate_match")
            ROUND.append_log(log, ROUND.log_records(root, summary, 5))
            self.assertEqual(len(log.read_text().splitlines()), 4)

    def test_line_metrics_separate_sink_from_cwe_label(self):
        finding = dict(cwe="CWE-367", path="lock.py", line=41, severity="high",
                       title="Symlink truncation", evidence="same sink, different CWE")
        expected = dict(cwe="CWE-59", path="lock.py", line=41)
        case = dict(run=dict(case_id="lock", outcome="success", elapsed_ms=1, findings=[finding]),
                    matched=[], unexpected=[finding], missed=[expected])
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            report = root / "report.json"
            ROUND.write_json(report, {"cases": [case]})
            (root / "executions.jsonl").write_text(json.dumps(dict(
                run="lock-1-candidate", variant="candidate", report="report.json",
                report_sha256=ROUND.digest(report.read_bytes()), summaries=[])) + "\n")
            group = ROUND.summarize(root)["groups"]["candidate"]
            self.assertEqual((group["tp"], group["fp"], group["fn"]), (0, 1, 1))
            self.assertEqual((group["line_tp"], group["line_fp"], group["line_fn"]), (1, 0, 0))
            self.assertEqual(group["line_f1"], 1.0)

    def test_all_trials_and_changed_report_detection(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            report = root / "report.json"
            ROUND.write_json(report, {"cases": [self.case]})
            records = [dict(run="one", variant="candidate", report="report.json",
                            report_sha256=ROUND.digest(report.read_bytes()), summaries=[]),
                       dict(run="two", variant="candidate", report=None, expected_count=1)]
            (root / "executions.jsonl").write_text("\n".join(json.dumps(r) for r in records))
            group = ROUND.summarize(root)["groups"]["candidate"]
            self.assertEqual((group["trials"], group["success"], group["failed"], group["fn"]), (2, 1, 1, 2))
            report.write_text('{}')
            with self.assertRaises(ValueError):
                ROUND.summarize(root)


if __name__ == "__main__":
    unittest.main()
