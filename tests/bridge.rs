//! Caller-side bridge: vulnerability discovery -> verification -> reporting.
//!
//! The harness owns one bounded node and leaves cross-node coordination to the
//! caller ([constraints](../docs/constraints.md), H13). This bridge is such a
//! caller: it drives three `alchemist audit` processes, gives every node the
//! same base system prompt, appends a brief role instruction, and threads each
//! node's findings into the next node.
//!
//! The audited data is the committed Ajna fixture under `datasets/`
//! (`ajna-protocol-compromise-2/audit`). It is staged with the harness's own
//! `dataset::stage_workspace`, so the repository copy stays read-only.
//!
//! For each node the bridge reads that node's run journal and pushes a metrics
//! record as soon as the node finishes: trajectory rounds, tool usage (total,
//! errors, and per-tool counts), token usage (input/output/total/reasoning), and
//! result accuracy against the dataset's expected findings. The same record is
//! returned to the caller. The mock provider keeps the run deterministic.
mod support;

use audit_harness::monitor;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// The shared base prompt every node must receive unchanged.
const BASE_PROMPT: &str = include_str!("../prompts/audit.txt");

/// The committed real-world dataset this bridge runs over.
const DATASET: &str = "datasets/tiny/ajna-protocol-compromise-2/audit/dataset.json";

/// Brief task instruction used by each role, appended after the base prompt.
const ROLES: [(&str, &str); 3] = [
    (
        "discovery",
        "Brief: enumerate candidate vulnerabilities and their root-cause lines.",
    ),
    (
        "verification",
        "Brief: independently verify each candidate and reject unsupported ones.",
    ),
    (
        "reporting",
        "Brief: produce the final consolidated findings report.",
    ),
];

type Key = (String, String, u64);

fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}

/// A model turn that reads one source file, then stops for the tool result.
fn tool_turn(path: &str) -> String {
    let arguments = json!({"path": path}).to_string();
    sse(
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"read-1","type":"function","function":{"name":"read_file","arguments":arguments}}]}}]}),
    ) + &sse(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}))
        + "data: [DONE]\n\n"
}

/// A final turn with provider-reported usage so token metrics are non-zero.
fn final_turn(findings: &Value) -> String {
    let answer = json!({"schema_version":1,"findings":findings}).to_string();
    sse(
        json!({"choices":[{"delta":{"content":answer},"finish_reason":"stop"}],"usage":{"prompt_tokens":24,"completion_tokens":6,"total_tokens":30}}),
    ) + "data: [DONE]\n\n"
}

fn keys(findings: &Value) -> BTreeSet<Key> {
    findings
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| {
            (
                finding["cwe"].as_str().unwrap().to_owned(),
                finding["path"].as_str().unwrap().to_owned(),
                finding["line"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}

fn f1(matched: usize, unexpected: usize, missed: usize) -> Option<f64> {
    let denominator = 2 * matched + unexpected + missed;
    (denominator != 0).then(|| 2.0 * matched as f64 / denominator as f64)
}

/// Read one node's journal and build the required metrics record.
fn metrics_for(role: &str, output: &Value, trace_dir: &Path, expected: &BTreeSet<Key>) -> Value {
    let journal = std::fs::read_dir(trace_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .unwrap();
    let summary = monitor::inspect(&journal).unwrap();
    let mut by_name: BTreeMap<String, u64> = BTreeMap::new();
    for line in std::fs::read_to_string(&journal).unwrap().lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        if record["event"]["type"].as_str() == Some("tool_start") {
            let name = record["event"]["name"].as_str().unwrap().to_owned();
            *by_name.entry(name).or_default() += 1;
        }
    }

    let actual = keys(&output["findings"]);
    let matched = expected.intersection(&actual).count();
    let unexpected = actual.difference(expected).count();
    let missed = expected.difference(&actual).count();
    json!({
        "node": role,
        "trajectory_rounds": summary["summary"]["turns"],
        "tool_usage": {
            "total": summary["summary"]["tools"],
            "errors": summary["summary"]["tool_errors"],
            "by_name": by_name,
        },
        "token_usage": {
            "input": summary["summary"]["usage"]["prompt_tokens"],
            "output": summary["summary"]["usage"]["completion_tokens"],
            "total": summary["summary"]["usage"]["total_tokens"],
            "reasoning": summary["summary"]["usage"]["reasoning_tokens"],
        },
        "accuracy": {
            "expected": expected.len(),
            "matched": matched,
            "missed": missed,
            "unexpected": unexpected,
            "precision": ratio(matched, matched + unexpected),
            "recall": ratio(matched, matched + missed),
            "f1": f1(matched, unexpected, missed),
        },
        "elapsed_ms": summary["summary"]["elapsed_ms"],
    })
}

/// Run the three nodes in order. Later nodes receive the accumulated findings
/// of earlier nodes inside their instruction. Each node's metrics are appended
/// to `metrics.jsonl` and printed on stderr the moment that node finishes.
fn bridge(node: &Path, work_dir: &Path, config: &Path, expected: &Value) -> Vec<Value> {
    let expected_keys = keys(expected);
    let mut metrics_file = File::create(work_dir.join("metrics.jsonl")).unwrap();
    let mut history: Vec<(&str, Value)> = Vec::new();
    let mut reports = Vec::new();
    for (role, brief) in ROLES {
        let mut instruction = brief.to_owned();
        for (prior_role, findings) in &history {
            instruction.push_str(&format!("\n\n{prior_role} findings:\n{findings}"));
        }
        let trace_dir = work_dir.join("traces").join(role);
        let output = Command::new(node)
            .arg("audit")
            .arg("--config")
            .arg(config)
            .arg("--instruction")
            .arg(&instruction)
            .arg("--trace-dir")
            .arg(&trace_dir)
            .current_dir(work_dir)
            .env("AUDIT_API_KEY", "fixture-key")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{role} node failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
        let metrics = metrics_for(role, &parsed, &trace_dir, &expected_keys);
        // Timely push: emit this node's metrics before starting the next node.
        writeln!(metrics_file, "{metrics}").unwrap();
        metrics_file.flush().unwrap();
        eprintln!("{metrics}");
        history.push((role, parsed["findings"].clone()));
        reports.push(json!({"output": parsed, "metrics": metrics}));
    }
    reports
}

#[test]
fn bridge_runs_the_ajna_dataset_and_pushes_node_metrics() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DATASET);
    let (dataset, root) = audit_harness::dataset::load(&manifest).unwrap();
    assert_eq!(dataset.name, "ajna-protocol-compromise-2-v1");
    assert_eq!(dataset.cases.len(), 1);
    let case = &dataset.cases[0];
    assert_eq!(case.target, "ajna-v2/src");
    assert_eq!(case.expected.len(), 1);
    let source = case.expected[0].path.clone();
    // Stage the committed data so the repository copy stays read-only.
    let workspace = audit_harness::dataset::stage_workspace(&root, &manifest).unwrap();
    let harness = tempfile::tempdir().unwrap();
    std::fs::write(harness.path().join("base.txt"), BASE_PROMPT).unwrap();

    let expected = json!([{
        "cwe": case.expected[0].cwe.clone(),
        "path": source.clone(),
        "line": case.expected[0].line,
    }]);
    let true_finding = json!({
        "cwe": case.expected[0].cwe.clone(),
        "path": source.clone(),
        "line": case.expected[0].line,
        "severity": "high",
        "title": "DISCOVERY unsettled bad-debt liquidation",
        "evidence": "bucketTake repays borrower debt while the losing deposit is still counted.",
    });
    let false_finding = json!({
        "cwe": "CWE-682",
        "path": source.clone(),
        "line": 150,
        "severity": "low",
        "title": "DISCOVERY unproven rounding concern",
        "evidence": "shape-only suspicion that execution did not confirm.",
    });
    let discovery = json!([true_finding, false_finding]);
    let verified_finding = json!({
        "cwe": case.expected[0].cwe.clone(),
        "path": source.clone(),
        "line": case.expected[0].line,
        "severity": "high",
        "title": "VERIFIED unsettled bad-debt liquidation",
        "evidence": "independent replay reproduced the liquidation accounting gap.",
    });
    let verification = json!([verified_finding]);
    let report_finding = json!({
        "cwe": case.expected[0].cwe.clone(),
        "path": source.clone(),
        "line": case.expected[0].line,
        "severity": "high",
        "title": "REPORT unsettled bad-debt liquidation",
        "evidence": "final consolidated report.",
    });
    let reporting = json!([report_finding]);

    let mut bodies = Vec::new();
    for findings in [&discovery, &verification, &reporting] {
        bodies.push(tool_turn(&source));
        bodies.push(final_turn(findings));
    }
    let (endpoint, requests, server) = server(bodies);

    let config = harness.path().join("node.toml");
    std::fs::write(
        &config,
        format!(
            "schema_version = 1\n[cli]\nmodel = 'fixture'\nendpoint = {endpoint:?}\ntarget = {:?}\nroot = {:?}\nmax_attempts = 1\ntimeout_ms = 15000\nformat = 'quiet'\n[prompts]\nsystem_file = 'base.txt'\n[tools]\nenabled = ['read_file']\n",
            case.target,
            workspace.path(),
        ),
    )
    .unwrap();

    let reports = bridge(
        Path::new(env!("CARGO_BIN_EXE_alchemist")),
        harness.path(),
        &config,
        &expected,
    );
    server.join().unwrap();

    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0]["output"]["findings"], discovery);
    assert_eq!(reports[1]["output"]["findings"], verification);
    assert_eq!(reports[2]["output"]["findings"], reporting);

    // Metrics: two trajectory rounds and one read_file call per node.
    for report in &reports {
        assert_eq!(report["metrics"]["trajectory_rounds"], 2);
        assert_eq!(report["metrics"]["tool_usage"]["total"], 1);
        assert_eq!(report["metrics"]["tool_usage"]["errors"], 0);
        assert_eq!(report["metrics"]["tool_usage"]["by_name"]["read_file"], 1);
        assert_eq!(report["metrics"]["token_usage"]["input"], 24);
        assert_eq!(report["metrics"]["token_usage"]["output"], 6);
        assert_eq!(report["metrics"]["token_usage"]["total"], 30);
        assert_eq!(report["metrics"]["accuracy"]["expected"], 1);
        assert_eq!(report["metrics"]["accuracy"]["matched"], 1);
        assert_eq!(report["metrics"]["accuracy"]["recall"], 1.0);
    }
    // Verification rejects the discovery false positive, so precision rises.
    assert_eq!(reports[0]["metrics"]["accuracy"]["precision"], 0.5);
    assert_eq!(reports[0]["metrics"]["accuracy"]["unexpected"], 1);
    assert_eq!(reports[1]["metrics"]["accuracy"]["precision"], 1.0);
    assert_eq!(reports[1]["metrics"]["accuracy"]["unexpected"], 0);
    assert_eq!(reports[2]["metrics"]["accuracy"]["precision"], 1.0);
    assert_eq!(reports[2]["metrics"]["accuracy"]["unexpected"], 0);

    // Timely push: the JSONL metrics file holds one record per node in order.
    let pushed: Vec<Value> = std::fs::read_to_string(harness.path().join("metrics.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(pushed.len(), 3);
    assert_eq!(pushed[0], reports[0]["metrics"]);
    assert_eq!(pushed[1], reports[1]["metrics"]);
    assert_eq!(pushed[2], reports[2]["metrics"]);

    // Every node request carries the shared base prompt.
    let captured: Vec<Value> = (0..6).map(|_| requests.recv().unwrap()).collect();
    for request in &captured {
        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["messages"][0]["content"], BASE_PROMPT);
    }
    // Each node sends two requests (tool turn then final turn) with the same brief.
    let instruction = |request: &Value| {
        let user: Value =
            serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        user["instruction"].as_str().unwrap().to_owned()
    };
    assert!(instruction(&captured[0]).contains(ROLES[0].1));
    assert_eq!(instruction(&captured[0]), instruction(&captured[1]));
    assert!(!instruction(&captured[0]).contains(ROLES[1].1));
    assert!(instruction(&captured[2]).contains(ROLES[1].1));
    assert_eq!(instruction(&captured[2]), instruction(&captured[3]));
    assert!(instruction(&captured[4]).contains(ROLES[2].1));
    assert_eq!(instruction(&captured[4]), instruction(&captured[5]));
    // Verification sees discovery; reporting sees both earlier nodes.
    assert!(instruction(&captured[2]).contains("DISCOVERY unsettled bad-debt liquidation"));
    assert!(instruction(&captured[4]).contains("DISCOVERY unsettled bad-debt liquidation"));
    assert!(instruction(&captured[4]).contains("VERIFIED unsettled bad-debt liquidation"));
}

fn server(bodies: Vec<String>) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/fixture", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let task = thread::spawn(move || {
        for body in bodies {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing fixture request");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            support::configure_http_stream(&stream);
            let mut bytes = vec![];
            let mut buffer = [0; 8192];
            let (offset, length) = loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < offset + length {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let request: Value = serde_json::from_slice(&bytes[offset..offset + length]).unwrap();
            tx.send(request).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (endpoint, rx, task)
}
