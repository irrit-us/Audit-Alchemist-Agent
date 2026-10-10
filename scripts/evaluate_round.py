"""Repeat comparable CLI evaluations; retain every trial and fail-closed review metrics.

Python 3.11+, standard library only. See docs/evaluation.md for the TOML plan.
Credentials are inherited through AUDIT_API_KEY, never read from the plan.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import tomllib


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(value):
    return digest(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


def key(finding):
    return finding["cwe"], finding["path"], finding["line"]


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def source_hash(root):
    return identity({p.relative_to(root).as_posix(): digest(p.read_bytes())
                     for p in sorted(root.rglob("*")) if p.is_file()
                     and p.name != "dataset.json" and not p.is_symlink()})


def variant_limits(plan, name):
    """Resolve shared limits plus optional per-variant overrides."""
    return {**plan["limits"], **plan.get("limits_by_variant", {}).get(name, {})}


def select_case(data, case_id, plan):
    """Pick a case, optionally replacing its instruction with the blind plan one."""
    case = next(c for c in data["cases"] if c["id"] == case_id)
    if plan.get("use_plan_instruction", True):
        return {**case, "instruction": plan["instruction"]}
    return dict(case)


def verify_forwarded_limits(report, limits):
    """Reject comparisons whose evaluated child did not receive the planned limits."""
    args = report["args"]
    for option, value in limits.items():
        if option in {"jobs", "timeout_ms", "max_output_bytes"}:
            actual = report[option]
        else:
            flag = "--" + option.replace("_", "-")
            if flag not in args:
                raise ValueError(f"evaluation did not forward {flag}")
            actual = args[args.index(flag) + 1]
        if str(actual) != str(value):
            raise ValueError(f"evaluation changed {option}")


def review_counts(case, reviews, report_sha256):
    """Reviews bind to the full finding and exact report, never just its sink key."""
    counts = dict.fromkeys(("unexpected_total", "unexpected_valid", "unexpected_invalid",
                           "unexpected_unreviewed", "unexpected_out_of_scope",
                           "unexpected_low_impact", "alternate_matches", "valid_distinct"), 0)
    queue = []
    expected = {key(f) for f in case["matched"] + case["missed"]}
    seen = set()
    if case["run"]["outcome"] != "success":
        return counts, queue
    for finding in case["run"]["findings"]:
        if key(finding) in expected or key(finding) in seen:
            continue
        seen.add(key(finding))
        fid = identity(finding)
        rid = f"{report_sha256}:{case['run']['case_id']}:{fid}"
        review = reviews.get(rid, {})
        verdict = review.get("verdict", "unreviewed")
        if verdict not in {"valid_distinct", "alternate_match", "invalid", "out_of_scope",
                           "low_impact", "unreviewed"}:
            raise ValueError(f"unknown verdict: {verdict}")
        if verdict != "unreviewed" and not review.get("evidence", "").strip():
            raise ValueError("review verdict requires evidence")
        counts["unexpected_total"] += 1
        if verdict in {"valid_distinct", "alternate_match"}:
            counts["unexpected_valid"] += 1
            counts["valid_distinct" if verdict == "valid_distinct" else "alternate_matches"] += 1
        else:
            counts[{"invalid": "unexpected_invalid", "out_of_scope": "unexpected_out_of_scope",
                    "low_impact": "unexpected_low_impact", "unreviewed": "unexpected_unreviewed"}[verdict]] += 1
        queue.append({"review_id": rid, "finding": finding, "verdict": verdict,
                      "evidence": review.get("evidence", "")})
    return counts, queue


def summarize(output, review_path=None):
    reviews = json.loads(review_path.read_text(encoding="utf-8")) if review_path else {}
    records = [json.loads(line) for line in (output / "executions.jsonl").read_text().splitlines()]
    groups, queue, case_metrics = {}, [], []
    for record in records:
        group = groups.setdefault(record["variant"], {"trials": 0, "success": 0, "failed": 0,
                                  "tp": 0, "fp": 0, "fn": 0, "line_tp": 0, "line_fp": 0,
                                  "line_fn": 0, "latencies_ms": [],
                                  "completed_latencies_ms": [], "timeouts": 0,
                                  "operational_totals": {}, "runs_with_usage": 0})
        group["trials"] += 1
        if not record.get("report"):
            group["failed"] += 1
            group["fn"] += record["expected_count"]
            group["line_fn"] += record["expected_count"]
            continue
        path = output / record["report"]
        report_bytes = path.read_bytes()
        if digest(report_bytes) != record["report_sha256"]:
            raise ValueError("report changed since execution")
        report = json.loads(report_bytes)
        for case in report["cases"]:
            success = case["run"]["outcome"] == "success"
            group["success" if success else "failed"] += 1
            group["timeouts"] += case["run"]["outcome"] == "timeout"
            group["latencies_ms"].append(case["run"]["elapsed_ms"])
            if success:
                group["completed_latencies_ms"].append(case["run"]["elapsed_ms"])
            for name, field in (("tp", "matched"), ("fp", "unexpected"), ("fn", "missed")):
                group[name] += len(case[field])
            # Line-only matching separates finding the sink from choosing the
            # CWE label; it is a distinct metric, never a replacement for exact.
            expected_lines = {(f["path"], f["line"]) for f in case["matched"] + case["missed"]}
            reported_lines = {(f["path"], f["line"]) for f in case["run"]["findings"]}
            line = {"line_tp": len(expected_lines & reported_lines),
                    "line_fp": len(reported_lines - expected_lines),
                    "line_fn": len(expected_lines - reported_lines)}
            for name, count in line.items():
                group[name] += count
            counts, pending = review_counts(case, reviews, record["report_sha256"])
            case_metrics.append({"run": record["run"], "variant": record["variant"], **line, **counts})
            queue.extend({"run": record["run"], "variant": record["variant"], **item} for item in pending)
            for name, count in counts.items():
                group[name] = group.get(name, 0) + count
        for summary in record["summaries"]:
            for name in ("turns", "tools", "tool_errors", "retries", "output_repairs", "empty_completions", "tool_budget_rejections"):
                group["operational_totals"][name] = group["operational_totals"].get(name, 0) + summary.get(name, 0)
            usage = summary.get("usage")
            if usage and any(usage.values()):
                group["runs_with_usage"] += 1
                for name, count in usage.items():
                    group["operational_totals"][name] = group["operational_totals"].get(name, 0) + count
    for group in groups.values():
        for label, n, d in (("precision", group["tp"], group["tp"] + group["fp"]),
                            ("recall", group["tp"], group["tp"] + group["fn"]),
                            ("f1", 2 * group["tp"], 2 * group["tp"] + group["fp"] + group["fn"]),
                            ("line_precision", group["line_tp"], group["line_tp"] + group["line_fp"]),
                            ("line_recall", group["line_tp"], group["line_tp"] + group["line_fn"]),
                            ("line_f1", 2 * group["line_tp"], 2 * group["line_tp"] + group["line_fp"] + group["line_fn"])):
            group[label] = n / d if d else None
        for field in ("latencies_ms", "completed_latencies_ms"):
            values = group[field]
            group[field + "_mean"] = statistics.mean(values) if values else None
            group[field + "_stdev"] = statistics.stdev(values) if len(values) > 1 else None
    return {"schema_version": 1, "groups": groups, "case_metrics": case_metrics,
            "review_queue": queue, "usage_note": "Provider-reported totals; zero/missing usage is unknown, not free. No cost claim."}


def log_records(output, summary, round_number, date=None):
    """Compact per-round and per-case records for round-over-round comparison.

    Exact-match metrics stay separate from reviewed unexpected-finding metrics so
    a later round can compare capability, false-positive behavior, and cost
    without re-reading every full report. Appended records are additive.
    """
    manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
    date = date or manifest["started_utc"][:10]
    findings_by_run = {}
    for item in summary["review_queue"]:
        findings_by_run.setdefault(item["run"], []).append({
            "cwe": item["finding"]["cwe"], "path": item["finding"]["path"],
            "line": item["finding"]["line"], "verdict": item["verdict"]})
    records = []
    for variant, group in sorted(summary["groups"].items()):
        operational = group["operational_totals"]
        limits = variant_limits(manifest["plan"], variant)
        config = {name: limits.get(name) for name in
                  ("model", "max_tokens", "max_tool_calls", "reasoning_effort",
                   "max_stream_bytes", "max_output_repairs")}
        config["trials"] = manifest["plan"]["trials"]
        config["workers"] = manifest["plan"].get("workers", 1)
        records.append({
            "type": "round", "round": round_number, "variant": variant, "date": date,
            "config": config,
            "exact": {name: group.get(name) for name in ("tp", "fp", "fn", "precision", "recall", "f1")},
            "line": {name: group.get(name) for name in
                     ("line_tp", "line_fp", "line_fn", "line_precision", "line_recall", "line_f1")},
            "trials": group["trials"], "success": group["success"], "failed": group["failed"],
            "timeouts": group["timeouts"], "unexpected_total": group.get("unexpected_total", 0),
            "unexpected_valid": group.get("unexpected_valid", 0),
            "unexpected_invalid": group.get("unexpected_invalid", 0),
            "unexpected_unreviewed": group.get("unexpected_unreviewed", 0),
            "unexpected_low_impact": group.get("unexpected_low_impact", 0),
            "unexpected_out_of_scope": group.get("unexpected_out_of_scope", 0),
            "alternate_matches": group.get("alternate_matches", 0),
            "valid_distinct": group.get("valid_distinct", 0),
            "turns": operational.get("turns", 0), "tools": operational.get("tools", 0),
            "tool_errors": operational.get("tool_errors", 0), "retries": operational.get("retries", 0),
            "output_repairs": operational.get("output_repairs", 0),
            "empty_completions": operational.get("empty_completions", 0),
            "tool_budget_rejections": operational.get("tool_budget_rejections", 0),
            "tokens": {name: operational.get(name, 0) for name in
                       ("prompt_tokens", "completion_tokens", "reasoning_tokens", "total_tokens")},
        })
        for metric in sorted((m for m in summary["case_metrics"] if m.get("variant") == variant),
                             key=lambda m: m["run"]):
            records.append({
                "type": "case", "round": round_number, "variant": variant, "date": date,
                "case_id": metric["run"],
                **{name: metric.get(name, 0) for name in
                   ("line_tp", "line_fp", "line_fn", "unexpected_total", "unexpected_valid",
                    "unexpected_invalid", "unexpected_unreviewed", "unexpected_low_impact",
                    "unexpected_out_of_scope", "alternate_matches", "valid_distinct")},
                "findings": findings_by_run.get(metric["run"], []),
            })
    return records


def append_log(path, records):
    existing = path.read_text(encoding="utf-8") if path.exists() else ""
    lines = [json.dumps(record, sort_keys=True) for record in records]
    with path.open("a", encoding="utf-8") as stream:
        if existing and not existing.endswith("\n"):
            stream.write("\n")
        stream.write("\n".join(lines) + "\n")


def run_plan(plan_path, binary, output):
    plan = tomllib.loads(plan_path.read_text(encoding="utf-8"))
    root = plan_path.resolve().parent.parent
    output.mkdir(parents=True, exist_ok=False)
    binary = binary.resolve()
    variants = {}
    for name, relative in plan["prompts"].items():
        prompt = (root / relative).read_text(encoding="utf-8")
        # JSON string syntax is also valid for these TOML basic strings.
        config = output / f"{name}.toml"
        config.write_text("schema_version = 1\n[prompts]\nsystem = " + json.dumps(prompt) + "\n", encoding="utf-8")
        variants[name] = {"config": config, "sha256": digest(prompt.encode())}
    datasets = []
    for entry in plan["datasets"]:
        path = root / entry["path"]
        data = json.loads(path.read_text(encoding="utf-8"))
        case = select_case(data, entry["case"], plan)
        datasets.append((path, data["name"], case, source_hash(path.parent)))
    manifest = {"schema_version": 1, "started_utc": datetime.now(timezone.utc).isoformat(),
                "harness_revision": git(root, "rev-parse", "HEAD"),
                "working_diff_sha256": digest(subprocess.check_output(["git", "-C", str(root), "diff", "HEAD"])),
                "binary_sha256": digest(binary.read_bytes()), "plan": plan,
                "plan_sha256": digest(plan_path.read_bytes()),
                "prompt_sha256": {name: v["sha256"] for name, v in variants.items()},
                "dataset_revision": git(root / "datasets/tiny", "rev-parse", "HEAD"),
                "sources": {case["id"]: sha for _, _, case, sha in datasets},
                "selection": "all trials, including failures; no best-of selection"}
    write_json(output / "manifest.json", manifest)

    def execute(job):
        trial, name, dataset = job
        path, dataset_name, case, sha = dataset
        run_id = f"{case['id']}-{trial}-{name}"
        run_dir = output / run_id
        run_dir.mkdir()
        with tempfile.TemporaryDirectory(prefix="alchemist-round-") as temp:
            staged = Path(temp) / "audit"
            shutil.copytree(path.parent, staged, ignore=shutil.ignore_patterns("dataset.json", "node_modules", "target", ".git"))
            write_json(staged / "dataset.json", {"schema_version": 1, "name": dataset_name, "cases": [case]})
            command = [str(binary), "--config", str(variants[name]["config"]), "evaluate",
                       "--dataset", str(staged / "dataset.json"), "--output", str(run_dir / "report.json"),
                       "--trace-dir", str(run_dir / "traces")]
            for option, value in variant_limits(plan, name).items():
                command.extend(["--" + option.replace("_", "-"), str(value)])
            result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            # Operational stderr is retained; the CLI excludes credentials from ordinary events.
            secret = os.environ.get("AUDIT_API_KEY", "")
            stderr = result.stderr.decode("utf-8", errors="replace")
            if secret:
                stderr = stderr.replace(secret, "[REDACTED]")
            (run_dir / "stderr.log").write_text(stderr, encoding="utf-8")
        summaries = []
        for trace in (run_dir / "traces").glob("*.jsonl"):
            for line in trace.read_text(encoding="utf-8").splitlines():
                event = json.loads(line).get("event", {})
                if event.get("name") == "run_end":
                    summaries.append(event["details"])
        report = run_dir / "report.json"
        record = {"run": run_id, "variant": name, "trial": trial, "case_id": case["id"],
                  "source_sha256": sha, "exit_code": result.returncode, "expected_count": len(case["expected"]),
                  "summaries": summaries, "report": None}
        if report.exists() and report.stat().st_size:
            verify_forwarded_limits(json.loads(report.read_bytes()), variant_limits(plan, name))
            record.update(report=report.relative_to(output).as_posix(), report_sha256=digest(report.read_bytes()))
        return record

    jobs = [(trial, name, dataset) for trial in range(1, plan["trials"] + 1)
            for dataset in datasets for name in (list(variants) if trial % 2 else list(reversed(variants)))]
    with (output / "executions.jsonl").open("x", encoding="utf-8") as log:
        with ThreadPoolExecutor(max_workers=plan.get("workers", 1)) as pool:
            # Completion order is logged promptly, so interrupted runs retain completed trials.
            from concurrent.futures import as_completed
            for future in as_completed([pool.submit(execute, job) for job in jobs]):
                record = future.result()
                log.write(json.dumps(record) + "\n")
                log.flush()
                print(record["run"], "exit", record["exit_code"], flush=True)
    write_json(output / "summary.json", summarize(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reviews", type=Path)
    parser.add_argument("--summarize", type=Path, help="Write a new reviewed summary, without model calls")
    parser.add_argument("--log", type=Path, help="Append compact round metrics to this JSONL log")
    parser.add_argument("--round", type=int, help="Round number recorded in --log")
    args = parser.parse_args()
    if args.summarize:
        summary = summarize(args.output.resolve(), args.reviews)
        write_json(args.summarize, summary)
        if args.log:
            if args.round is None:
                parser.error("--log requires --round")
            append_log(args.log, log_records(args.output.resolve(), summary, args.round))
    elif args.plan and args.binary:
        run_plan(args.plan, args.binary, args.output.resolve())
    else:
        parser.error("run requires --plan and --binary")


if __name__ == "__main__":
    main()
