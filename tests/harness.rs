use audit_harness::{
    context::snapshot,
    dataset::{Case, Dataset},
    evaluate::{aggregate, score},
    protocol::{Finding, FindingKey, Request, Response, Severity, VERSION},
    runner::{run, Outcome, RunConfig, RunResult},
};
use std::{path::PathBuf, time::Duration};

fn key() -> FindingKey {
    FindingKey {
        cwe: "CWE-78".into(),
        path: "sample.py".into(),
        line: 1,
    }
}
fn finding() -> Finding {
    Finding {
        cwe: "CWE-78".into(),
        path: "sample.py".into(),
        line: 1,
        severity: Severity::High,
        title: "Injection".into(),
        evidence: "Input reaches shell".into(),
    }
}
fn result(outcome: Outcome, findings: Vec<Finding>) -> RunResult {
    RunResult {
        case_id: "test".into(),
        outcome,
        elapsed_ms: 1,
        exit_code: Some(0),
        error: None,
        findings,
    }
}
fn request() -> Request {
    Request {
        schema_version: VERSION,
        case_id: "test".into(),
        target: "sample.py".into(),
        instruction: "Audit".into(),
    }
}

#[test]
fn protocol_roundtrip_and_rejection() {
    let response = Response {
        schema_version: VERSION,
        findings: vec![finding()],
    };
    let decoded: Response =
        serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
    decoded.validate().unwrap();
    for path in [
        "../escape.py",
        "/absolute.py",
        "./sample.py",
        "a//b.py",
        "a\\b.py",
        "C:/a.py",
    ] {
        let mut key = key();
        key.path = path.into();
        assert!(key.validate().is_err());
    }
    assert!(
        serde_json::from_str::<Response>(r#"{"schema_version":1,"findings":[],"extra":true}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<Response>(r#"{"schema_version":1,"findings":[{"cwe":"CWE-78","path":"sample.py","line":1,"severity":"high","title":"x","evidence":"x","extra":true}]}"#).is_err());
}

#[test]
fn scoring_counts_duplicates_once_and_failures_as_misses() {
    let case = Case {
        id: "test".into(),
        target: "sample.py".into(),
        instruction: "audit".into(),
        expected: vec![key()],
    };
    let report = score(&case, result(Outcome::Success, vec![finding(), finding()]));
    assert_eq!(report.duplicate_findings, 1);
    let metrics = aggregate(&[report], 5);
    assert_eq!(
        (
            metrics.true_positives,
            metrics.false_positives,
            metrics.false_negatives
        ),
        (1, 0, 0)
    );
    assert_eq!(metrics.f1, Some(1.0));
    let failed = aggregate(&[score(&case, result(Outcome::Timeout, vec![]))], 10);
    assert_eq!(failed.false_negatives, 1);
    assert_eq!(failed.exact_match_cases, 0);
    assert_eq!(failed.precision, None);
    assert_eq!(failed.recall, Some(0.0));
    let mut wrong = finding();
    wrong.cwe = "CWE-89".into();
    let mixed = aggregate(&[score(&case, result(Outcome::Success, vec![wrong]))], 0);
    assert_eq!(
        (
            mixed.true_positives,
            mixed.false_positives,
            mixed.false_negatives
        ),
        (0, 1, 1)
    );
}

#[test]
fn dataset_and_snapshot_limits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.py"), "print('hello')\n").unwrap();
    let case = Case {
        id: "test".into(),
        target: "sample.py".into(),
        instruction: "audit".into(),
        expected: vec![key()],
    };
    let mut dataset = Dataset {
        schema_version: VERSION,
        name: "test".into(),
        cases: vec![case.clone()],
    };
    dataset.validate(dir.path()).unwrap();
    dataset.cases.push(case);
    assert!(dataset.validate(dir.path()).is_err());
    assert!(snapshot(dir.path(), "sample.py", 2).is_err());
    assert_eq!(snapshot(dir.path(), "sample.py", 1024).unwrap().len(), 1);
    assert!(snapshot(dir.path(), "../escape.py", 1024).is_err());
}

async fn shell(script: &str, timeout_ms: u64, limit: usize) -> RunResult {
    #[cfg(unix)]
    let executable = PathBuf::from("/bin/sh");
    #[cfg(windows)]
    let executable = std::env::var_os("AUDIT_BASH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("ProgramFiles")
                .map(|root| PathBuf::from(root).join("Git/bin/bash.exe"))
                .filter(|path| path.is_file())
        })
        .unwrap_or_else(|| "bash".into());
    #[cfg(not(any(unix, windows)))]
    let executable = PathBuf::from("bash");
    run(
        &RunConfig {
            executable,
            args: vec!["-c".into(), script.into()],
            root: std::env::current_dir().unwrap(),
            timeout: Duration::from_millis(timeout_ms),
            max_output_bytes: limit,
        },
        request(),
    )
    .await
}

#[tokio::test]
async fn subprocess_failure_modes() {
    assert_eq!(
        shell(
            "cat >/dev/null; printf '%s' '{\"schema_version\":1,\"findings\":[]}'",
            2000,
            1024
        )
        .await
        .outcome,
        Outcome::Success
    );
    assert_eq!(
        shell("cat >/dev/null; printf garbage", 2000, 1024)
            .await
            .outcome,
        Outcome::InvalidResponse
    );
    assert_eq!(
        shell("cat >/dev/null; exit 7", 2000, 1024).await.outcome,
        Outcome::NonzeroExit
    );
    assert_eq!(
        shell("cat >/dev/null; sleep 5", 50, 1024).await.outcome,
        Outcome::Timeout
    );
    assert_eq!(
        shell("cat >/dev/null; yes x", 2000, 1024).await.outcome,
        Outcome::OutputLimit
    );
    assert_eq!(
        shell("cat >/dev/null; yes x >&2", 2000, 1024).await.outcome,
        Outcome::OutputLimit
    );
    let missing = run(
        &RunConfig {
            executable: "/nonexistent/audit-agent".into(),
            args: vec![],
            root: std::env::current_dir().unwrap(),
            timeout: Duration::from_secs(1),
            max_output_bytes: 1024,
        },
        request(),
    )
    .await;
    assert_eq!(missing.outcome, Outcome::SpawnError);
}

#[test]
fn smoke_dataset_validates() {
    let (dataset, _) = audit_harness::dataset::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("datasets/smoke/dataset.json"),
    )
    .unwrap();
    assert_eq!(dataset.cases.len(), 6);
}

#[test]
fn ajna_dataset_validates() {
    let (dataset, _) = audit_harness::dataset::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("datasets/tiny/ajna-protocol-compromise-2/audit/dataset.json"),
    )
    .unwrap();
    assert_eq!(dataset.name, "ajna-protocol-compromise-2-v1");
    assert_eq!(dataset.cases.len(), 1);
    assert_eq!(dataset.cases[0].id, "unsettled-bad-debt-liquidation");
    assert_eq!(dataset.cases[0].target, "ajna-v2/src");
    assert_eq!(dataset.cases[0].expected.len(), 1);
    assert_eq!(dataset.cases[0].expected[0].cwe, "CWE-841");
    assert_eq!(dataset.cases[0].expected[0].line, 168);
}

#[test]
fn real_world_datasets_validate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("datasets/tiny");
    let manifests = [
        "ajna-protocol-compromise-2/audit/dataset.json",
        "filelock-toctou/audit/dataset.json",
        "fast-jwt-iss/audit/dataset.json",
        "thin-vec-uaf/audit/dataset.json",
        "pymonocypher-overflow/audit/dataset.json",
        "h11-chunked-framing/audit/dataset.json",
        "zk-email-sha256/audit/dataset.json",
    ];
    for manifest in manifests {
        let (dataset, _) = audit_harness::dataset::load(&root.join(manifest)).unwrap();
        assert_eq!(dataset.cases.len(), 1, "{manifest}");
        assert_eq!(dataset.cases[0].expected.len(), 1, "{manifest}");
    }
}

#[test]
fn demo_end_to_end() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args([
            "benchmark",
            "--dataset",
            "datasets/smoke/dataset.json",
            "--agent",
            env!("CARGO_BIN_EXE_demo-agent"),
            "--jobs",
            "2",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["metrics"]["successful_cases"], 6);
    assert_eq!(report["metrics"]["true_positives"], 3);
    assert_eq!(report["metrics"]["false_positives"], 0);
    assert_eq!(report["metrics"]["false_negatives"], 0);
    assert_eq!(report["metrics"]["f1"], 1.0);
    assert_eq!(report["cases"][0]["run"]["case_id"], "command-injection");
}

#[test]
fn report_destination_is_checked_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("existing.json");
    std::fs::write(&report, "preserve this").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args([
            "benchmark",
            "--dataset",
            "datasets/smoke/dataset.json",
            "--agent",
            env!("CARGO_BIN_EXE_demo-agent"),
            "--output",
        ])
        .arg(&report)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(report).unwrap(), "preserve this");
    assert!(output.stdout.is_empty());
}

#[cfg(target_os = "linux")]
fn process_is_running(pid: u32) -> bool {
    // A killed orphan can briefly remain as a zombie until the system reaps it.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
        Err(_) => false,
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn deadline_and_cancellation_kill_descendants() {
    for cancel in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("descendant.pid");
        let script = format!(
            "cat >/dev/null; sleep 20 & printf '%s' $! > '{}'; wait",
            pid_file.display()
        );
        let config = RunConfig {
            executable: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            root: dir.path().to_owned(),
            timeout: Duration::from_millis(if cancel { 5000 } else { 250 }),
            max_output_bytes: 1024,
        };
        let task = tokio::spawn(async move { run(&config, request()).await });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let pid: u32 = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = text.parse() {
                    break pid;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "descendant did not start"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(task.await.unwrap().outcome, Outcome::Timeout);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while process_is_running(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "descendant survived cleanup"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[cfg(unix)]
#[test]
fn source_snapshot_does_not_follow_nested_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sources")).unwrap();
    std::fs::write(root.path().join("sources/good.py"), "pass\n").unwrap();
    std::fs::write(outside.path().join("secret.py"), "secret = 'hidden'\n").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.py"),
        root.path().join("sources/linked.py"),
    )
    .unwrap();
    let sources = snapshot(root.path(), "sources", 1024).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, "sources/good.py");
    assert!(snapshot(root.path(), "sources/linked.py", 1024).is_err());
}
