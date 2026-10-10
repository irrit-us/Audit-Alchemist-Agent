mod support;

use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

/// Real HTTP transport and real CLI adapter against a one-shot local provider.
fn fixture(
    status: u16,
    content: serde_json::Value,
    finish: &str,
) -> (
    String,
    mpsc::Receiver<serde_json::Value>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let body = if status == 200 {
        // The adapter always requests streaming, so serve SSE.
        let delta = serde_json::json!({
            "choices": [{"delta": {"content": content.to_string()}, "finish_reason": null}]
        });
        let stop = serde_json::json!({
            "choices": [{"delta": {}, "finish_reason": finish}]
        });
        format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n")
    } else {
        "{}".to_string()
    };
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "no API request received"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        support::configure_http_stream(&stream);
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        let (header_end, length) = loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(position) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = std::str::from_utf8(&bytes[..position])
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(header.starts_with("post /v1/chat/completions "));
                assert!(header.contains("authorization: bearer fixture-key"));
                let length = header
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                break (position + 4, length);
            }
        };
        while bytes.len() < header_end + length {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
        }
        sender
            .send(serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap())
            .unwrap();
        write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    (endpoint, receiver, handle)
}

fn audit(endpoint: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args([
            "audit",
            "--root",
            "datasets/smoke",
            "--target",
            "sources/command_unsafe.py",
            "--instruction",
            "Audit command execution with attacker-controlled name.",
            "--endpoint",
            endpoint,
            "--model",
            "fixture-model",
            "--max-attempts",
            "1",
            "--max-output-repairs",
            "0",
            "--timeout-ms",
            "5000",
        ])
        .env("AUDIT_API_KEY", "fixture-key")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap()
}

#[test]
fn plain_api_audit_transmits_only_scoped_source_and_validates_findings() {
    let finding = serde_json::json!({"schema_version":1,"findings":[{"cwe":"CWE-78","path":"sources/command_unsafe.py","line":5,"severity":"high","title":"Command injection","evidence":"Attacker-controlled name reaches a shell command"}]});
    let (endpoint, receiver, handle) = fixture(200, finding, "stop");
    let output = audit(&endpoint);
    let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"], "success");
    assert_eq!(report["findings"][0]["cwe"], "CWE-78");
    assert_eq!(request["model"], "fixture-model");
    let user: serde_json::Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert!(user.get("expected").is_none());
    assert!(user.get("case_id").is_none());
    assert_eq!(user["sources"].as_array().unwrap().len(), 1);
    assert!(user["sources"][0]["content"]
        .as_str()
        .unwrap()
        .contains("shell=True"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-key"));
}

#[test]
fn evaluation_forwards_reasoning_effort_and_stream_limit_to_child() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.py"), "print('safe')\n").unwrap();
    std::fs::write(dir.path().join("dataset.json"), serde_json::json!({
        "schema_version": 1, "name": "forwarding", "cases": [{
            "id": "safe", "target": "source.py", "instruction": "Audit this file.", "expected": []
        }]
    }).to_string()).unwrap();
    let (endpoint, receiver, handle) = fixture(
        200,
        serde_json::json!({"schema_version":1,"findings":[]}),
        "stop",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args(["evaluate", "--dataset"])
        .arg(dir.path().join("dataset.json"))
        .args([
            "--endpoint",
            &endpoint,
            "--model",
            "fixture-model",
            "--reasoning-effort",
            "low",
            "--max-stream-bytes",
            "3145728",
            "--max-attempts",
            "1",
            "--timeout-ms",
            "5000",
        ])
        .env("AUDIT_API_KEY", "fixture-key")
        .output()
        .unwrap();
    let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    handle.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(request["reasoning_effort"], "low");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let args = report["args"].as_array().unwrap();
    assert!(args
        .windows(2)
        .any(|pair| pair[0] == "--max-stream-bytes" && pair[1] == "3145728"));
    assert_eq!(report["metrics"]["successful_cases"], 1);
}

#[test]
fn api_failures_truncation_and_out_of_scope_findings_fail_the_run() {
    let empty = serde_json::json!({"schema_version":1,"findings":[]});
    let outside = serde_json::json!({"schema_version":1,"findings":[{"cwe":"CWE-78","path":"other.py","line":1,"severity":"high","title":"x","evidence":"x"}]});
    for (status, content, finish) in [
        (429, empty.clone(), "stop"),
        (200, empty, "length"),
        (200, outside, "stop"),
    ] {
        let (endpoint, receiver, handle) = fixture(status, content, finish);
        let output = audit(&endpoint);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["outcome"], "provider_error");
        assert_eq!(report["findings"].as_array().unwrap().len(), 0);
    }
}
