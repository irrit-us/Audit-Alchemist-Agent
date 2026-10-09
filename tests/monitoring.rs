mod support;

use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn server(replies: Vec<(u16, u64, String)>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let handle = thread::spawn(move || {
        for (status, delay, body) in replies {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "provider not called");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            support::configure_http_stream(&stream);
            let mut bytes = Vec::new();
            let mut buf = [0; 8192];
            loop {
                let count = stream.read(&mut buf).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buf[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_millis(delay));
            // Deadline tests intentionally close the connection before this write.
            let _ = write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (url, handle)
}

fn answer(content: &str) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{"content":content},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}})
    )
}

fn run(root: &Path, url: &str, extra: &[&str]) -> std::process::Output {
    std::fs::write(root.join("a.py"), "# source-private fixture-secret\npass\n").unwrap();
    Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args(["audit", "--root"])
        .arg(root)
        .args([
            "--target",
            "a.py",
            "--model",
            "fixture",
            "--endpoint",
            url,
            "--retry-base-ms",
            "0",
            "--retry-max-ms",
            "0",
            "--trace-dir",
        ])
        .arg(root.join("traces"))
        .args(extra)
        .env("AUDIT_API_KEY", "fixture-secret")
        .output()
        .unwrap()
}

fn trace(root: &Path) -> (Value, String) {
    let path = std::fs::read_dir(root.join("traces"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    (
        audit_harness::monitor::inspect(&path).unwrap(),
        std::fs::read_to_string(path).unwrap(),
    )
}

#[test]
fn retries_and_opted_in_debug_capture_preserve_privacy_and_json_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![
        (503, 0, "{}".into()),
        (200, 0, answer(r#"{"schema_version":1,"findings":[]}"#)),
    ]);
    let output = run(dir.path(), &url, &["--debug-trace", "--format", "jsonl"]);
    task.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["outcome"],
        "success"
    );
    let (summary, raw) = trace(dir.path());
    assert_eq!(summary["summary"]["retries"], 1);
    assert_eq!(summary["summary"]["usage"]["total_tokens"], 15);
    assert!(raw.contains("source-private"));
    assert!(!raw.contains("fixture-secret"));
    assert!(raw.contains("[REDACTED]"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("source-private"));
    assert!(!stderr.contains("fixture-secret"));
    assert!(!stderr.contains("\"type\":\"debug\""));
}

#[test]
fn waiting_provider_emits_heartbeat_and_records_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![(
        200,
        5800,
        answer(r#"{"schema_version":1,"findings":[]}"#),
    )]);
    let output = run(
        dir.path(),
        &url,
        &["--timeout-ms", "5500", "--format", "jsonl"],
    );
    task.join().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["outcome"],
        "timeout"
    );
    let (summary, raw) = trace(dir.path());
    assert_eq!(summary["summary"]["outcome"], "timeout");
    assert_eq!(summary["summary"]["phase"], "model");
    assert!(raw.contains("heartbeat"));
    assert!(!raw.contains("source-private"));
}

#[test]
fn invalid_final_output_is_an_error_at_validation_and_skills_need_no_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![(200, 0, answer("not-json"))]);
    let output = run(dir.path(), &url, &["--format", "quiet"]);
    task.join().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let (summary, _) = trace(dir.path());
    assert_eq!(summary["summary"]["outcome"], "error");
    assert_eq!(summary["summary"]["phase"], "validating");
    for args in [
        vec!["skills"],
        vec![
            "skills",
            "native-debugging",
            "--resource",
            "references/pwndbg.md",
        ],
        vec!["doctor"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_alchemist"))
            .current_dir(dir.path())
            .args(args)
            .env_remove("AUDIT_API_KEY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args([
            "audit",
            "--model",
            "fixture",
            "--target",
            ".",
            "--debug-trace",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--trace-dir"));
}
