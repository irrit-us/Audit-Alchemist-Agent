mod support;

use audit_harness::{
    context::{Context, Source},
    protocol::{Finding, Severity},
    tools::WorkspaceTools,
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[tokio::test]
async fn workspace_tools_page_edit_search_and_report_recoverable_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "first\nneedle\nlast\n").unwrap();
    let context = Context {
        sources: vec![],
        total_bytes: 0,
    };
    let mut tools = WorkspaceTools::new(dir.path(), &context).unwrap();
    let timeout = Duration::from_secs(5);
    let result = tools
        .execute(
            "read_file",
            r#"{"path":"a.py","offset":2,"limit":1}"#,
            timeout,
        )
        .await;
    assert_eq!(result["content"], "2: needle\n");
    assert_eq!(result["next_offset"], 3);
    let mut finding = Finding {
        cwe: "CWE-78".into(),
        path: "a.py".into(),
        line: 2,
        severity: Severity::High,
        title: "x".into(),
        evidence: "x".into(),
    };
    tools.validate_finding(&finding).unwrap();
    finding.line = 3;
    assert!(tools.validate_finding(&finding).is_err());
    let result = tools
        .execute(
            "search",
            r#"{"target":".","needles":["needle","need"]}"#,
            timeout,
        )
        .await;
    assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    for (name, args) in [
        ("read_file", r#"{"path":"../escape.py"}"#),
        ("write_file", r#"{"path":"../escape.py","content":"x"}"#),
        ("read_file", r#"{"path":"a.py","offset":0}"#),
        (
            "edit_file",
            r#"{"path":"a.py","old_text":"missing","new_text":"x"}"#,
        ),
        ("unknown", "{}"),
        ("bash", "not-json"),
    ] {
        assert!(
            tools
                .execute(name, args, timeout)
                .await
                .get("error")
                .is_some(),
            "{name}"
        );
    }
    let result = tools
        .execute(
            "edit_file",
            r#"{"path":"a.py","old_text":"needle","new_text":"changed"}"#,
            timeout,
        )
        .await;
    assert_eq!(result["replacements"], 1);
    finding.line = 2;
    assert!(tools.validate_finding(&finding).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
        "first\nchanged\nlast\n"
    );
}

#[tokio::test]
async fn citation_fingerprint_rejects_external_source_change() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "first\nneedle\nlast\n").unwrap();
    let context = Context {
        sources: vec![Source {
            path: "a.py".into(),
            content: "first\nneedle\nlast\n".into(),
        }],
        total_bytes: 17,
    };
    let mut tools = WorkspaceTools::new(dir.path(), &context).unwrap();
    let timeout = Duration::from_secs(5);
    let mut finding = Finding {
        cwe: "CWE-78".into(),
        path: "a.py".into(),
        line: 2,
        severity: Severity::High,
        title: "x".into(),
        evidence: "x".into(),
    };
    tools.validate_finding(&finding).unwrap();
    // A Bash or external edit does not touch native state, so the fingerprint
    // must catch the stale citation.
    std::fs::write(dir.path().join("a.py"), "first\nchanged\nlast\n").unwrap();
    assert!(tools.validate_finding(&finding).is_err());
    // Re-reading the changed line makes the new content citable again.
    tools
        .execute(
            "read_file",
            r#"{"path":"a.py","offset":2,"limit":1}"#,
            timeout,
        )
        .await;
    tools.validate_finding(&finding).unwrap();
    // An unobserved line is still rejected.
    finding.line = 4;
    assert!(tools.validate_finding(&finding).is_err());
    // A later external change to another already-observed line is rejected.
    finding.line = 3;
    tools.validate_finding(&finding).unwrap();
    std::fs::write(dir.path().join("a.py"), "first\nchanged\nchanged-last\n").unwrap();
    assert!(tools.validate_finding(&finding).is_err());
}

#[tokio::test]
async fn bash_preserves_shell_syntax_and_native_failure_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = WorkspaceTools::new(
        dir.path(),
        &Context {
            sources: vec![],
            total_bytes: 0,
        },
    )
    .unwrap();
    let script = r#"values=('space value' '$HOME' '$(touch unexpected)')
printf '%s\n' "${values[@]}" > 'input with spaces.txt'
cat <<'LITERAL' >> 'input with spaces.txt'
$HOME `literal` $(also_literal)
LITERAL
mapfile -t lines < 'input with spaces.txt'
printf '[%s]\n' "${lines[@]}" | while IFS= read -r line; do
    printf '%s\n' "$line"
done
suffix=$(printf '%s' tail)
printf '%s\n' "$suffix"
for file in *.txt; do printf 'file:%s\n' "$file"; done
false | true
printf 'pipeline=%s\n' "$?"
false
printf 'continued\n'
"#;
    let result = tools
        .execute(
            "bash",
            &json!({"command":script}).to_string(),
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(result["exit_code"], 0, "{result}");
    assert_eq!(result["stderr"], "");
    assert_eq!(
        result["stdout"],
        "[space value]\n[$HOME]\n[$(touch unexpected)]\n[$HOME `literal` $(also_literal)]\ntail\nfile:input with spaces.txt\npipeline=0\ncontinued\n"
    );
    assert!(!dir.path().join("unexpected").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("input with spaces.txt")).unwrap(),
        "space value\n$HOME\n$(touch unexpected)\n$HOME `literal` $(also_literal)\n"
    );
}

#[tokio::test]
async fn bash_runs_pocs_reports_nonzero_exit_and_bounds_output_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = WorkspaceTools::new(
        dir.path(),
        &Context {
            sources: vec![Source {
                path: "a.py".into(),
                content: "x".into(),
            }],
            total_bytes: 1,
        },
    )
    .unwrap();
    let timeout = Duration::from_secs(5);
    let result = tools
        .execute(
            "bash",
            r#"{"command":"printf 'observed'; printf 'diagnostic' >&2; exit 7"}"#,
            timeout,
        )
        .await;
    assert_eq!(result["exit_code"], 7, "{result}");
    assert_eq!(result["stdout"], "observed");
    assert_eq!(result["stderr"], "diagnostic");
    let result = tools
        .execute(
            "bash",
            r#"{"command":"for ((i=0;i<5000;i++)); do printf '0123456789'; done"}"#,
            timeout,
        )
        .await;
    assert_eq!(result["truncated"], true);
    assert!(result["stdout"].as_str().unwrap().len() < 34_000);
    let start = Instant::now();
    let result = tools
        .execute(
            "bash",
            r#"{"command":"printf 'partial-result'; printf 'partial-error' >&2; (sleep 2; printf leaked > late.txt) & wait","timeout_ms":500}"#,
            timeout,
        )
        .await;
    assert!(
        result["error"].as_str().unwrap().contains("timeout"),
        "{result}"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(result["timed_out"], true);
    assert_eq!(result["stdout"], "partial-result");
    assert_eq!(result["stderr"], "partial-error");
    assert!(result["exit_code"].is_null());
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !dir.path().join("late.txt").exists(),
        "timed-out descendant survived"
    );
}

#[tokio::test]
async fn paged_reads_of_large_files_register_only_returned_lines() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "{}sink(name)\nlast\n",
        "# padding padding padding\n".repeat(60_000)
    );
    assert!(text.len() > 1_048_576);
    std::fs::write(dir.path().join("large.py"), text).unwrap();
    let mut tools = WorkspaceTools::new(
        dir.path(),
        &Context {
            sources: vec![],
            total_bytes: 0,
        },
    )
    .unwrap();
    let result = tools
        .execute(
            "read_file",
            r#"{"path":"large.py","offset":60001,"limit":1}"#,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(result["content"], "60001: sink(name)\n", "{result}");
    assert_eq!(result["next_offset"], 60002);
    assert!(result["total_lines"].is_null());
    let mut finding = Finding {
        cwe: "CWE-78".into(),
        path: "large.py".into(),
        line: 60001,
        severity: Severity::High,
        title: "x".into(),
        evidence: "x".into(),
    };
    tools.validate_finding(&finding).unwrap();
    finding.line = 60002;
    assert!(tools.validate_finding(&finding).is_err());
    let result = tools
        .execute(
            "read_file",
            r#"{"path":"large.py","offset":60002,"limit":1}"#,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(result["total_lines"], 60002);
    assert!(result["next_offset"].is_null());
}

fn calls() -> Vec<Value> {
    vec![
        json!({"id":"read-1","name":"read_file","arguments":json!({"path":"helper.py"}).to_string()}),
        json!({"id":"write-2","name":"write_file","arguments":json!({"path":"poc.sh","content":"printf 'poc-observed'\n"}).to_string()}),
        json!({"id":"bash-3","name":"bash","arguments":json!({"command":"bash poc.sh"}).to_string()}),
        json!({"id":"skill-4","name":"load_skill","arguments":json!({"name":"poc-validation"}).to_string()}),
    ]
}

fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn tool_turn(wire: &str) -> String {
    let calls = calls();
    match wire {
        "chat-completions" => {
            let mut text = String::new();
            for (i, call) in calls.iter().enumerate() {
                let args = call["arguments"].as_str().unwrap();
                text += &sse(
                    json!({"choices":[{"delta":{"tool_calls":[{"index":i,"id":call["id"],"type":"function","function":{"name":call["name"],"arguments":&args[..1]}}]}}]}),
                );
                text += &sse(
                    json!({"choices":[{"delta":{"tool_calls":[{"index":i,"function":{"arguments":&args[1..]}}]}}]}),
                );
            }
            text + &sse(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}))
                + "data: [DONE]\n\n"
        }
        "responses" => {
            let mut output = vec![
                json!({"type":"reasoning","id":"r1","summary":[],"encrypted_content":"opaque-state"}),
            ];
            output.extend(calls.iter().map(|c| json!({"type":"function_call","id":format!("item-{}", c["id"]),"call_id":c["id"],"name":c["name"],"arguments":c["arguments"],"status":"completed"})));
            sse(json!({"type":"response.completed","response":{"output":output}}))
        }
        "anthropic" => {
            let mut text = sse(
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            );
            text += &sse(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"inspect"}}),
            );
            text += &sse(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-state"}}),
            );
            for (i, call) in calls.iter().enumerate() {
                text += &sse(
                    json!({"type":"content_block_start","index":i+1,"content_block":{"type":"tool_use","id":call["id"],"name":call["name"],"input":{}}}),
                );
                text += &sse(
                    json!({"type":"content_block_delta","index":i+1,"delta":{"type":"input_json_delta","partial_json":call["arguments"]}}),
                );
            }
            text + &sse(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}))
                + &sse(json!({"type":"message_stop"}))
        }
        _ => unreachable!(),
    }
}

fn final_turn(wire: &str) -> String {
    let answer = json!({"schema_version":1,"findings":[{"cwe":"CWE-95","path":"helper.py","line":2,"severity":"high","title":"Untrusted eval","evidence":"name reaches eval(name); local fixture printed poc-observed"}]}).to_string();
    match wire {
        "chat-completions" => {
            sse(json!({"choices":[{"delta":{"content":answer},"finish_reason":"stop"}]}))
                + "data: [DONE]\n\n"
        }
        "responses" => sse(
            json!({"type":"response.completed","response":{"output":[{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":answer}]}]}}),
        ),
        "anthropic" => {
            sse(json!({"type":"content_block_delta","delta":{"type":"text_delta","text":answer}}))
                + &sse(json!({"type":"message_stop"}))
        }
        _ => unreachable!(),
    }
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
            tx.send(serde_json::from_slice(&bytes[offset..offset + length]).unwrap())
                .unwrap();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (endpoint, rx, task)
}

fn run(
    wire: &str,
    root: &std::path::Path,
    endpoint: &str,
    max_calls: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args(["audit", "--root"])
        .arg(root)
        .args([
            "--target",
            "entry.py",
            "--endpoint",
            endpoint,
            "--wire-api",
            wire,
            "--model",
            "fixture",
            "--max-attempts",
            "1",
            "--max-tool-calls",
            max_calls,
            "--timeout-ms",
            "8000",
            "--format",
            "quiet",
        ])
        .arg("--trace-dir")
        .arg(root.join("traces"))
        .env("AUDIT_API_KEY", "fixture-key")
        .output()
        .unwrap()
}

#[test]
fn all_wires_explore_write_run_poc_and_replay_native_tool_results() {
    for wire in ["chat-completions", "responses", "anthropic"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.py"), "from helper import run\n").unwrap();
        std::fs::write(
            dir.path().join("helper.py"),
            "def run(name):\n    return eval(name)\n",
        )
        .unwrap();
        let (endpoint, rx, task) = server(vec![tool_turn(wire), final_turn(wire)]);
        let output = run(wire, dir.path(), &endpoint, "4");
        assert!(
            output.status.success(),
            "{wire}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        task.join().unwrap();
        let first = rx.recv().unwrap();
        assert_eq!(first["tools"].as_array().unwrap().len(), 7);
        let second = rx.recv().unwrap();
        let history = if wire == "responses" {
            &second["input"]
        } else {
            &second["messages"]
        };
        assert!(history.to_string().contains("poc-observed"));
        assert!(history.to_string().contains("# PoC validation"));
        let trace = std::fs::read_dir(dir.path().join("traces"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let summary = audit_harness::monitor::inspect(&trace).unwrap();
        assert_eq!(summary["summary"]["outcome"], "success");
        assert_eq!(summary["summary"]["turns"], 2);
        assert_eq!(summary["summary"]["tools"], 4);
        let raw = std::fs::read_to_string(trace).unwrap();
        assert!(!raw.contains("poc-observed"));
        assert!(!raw.contains("fixture-key"));
        assert!(history.to_string().contains("exit_code"));
        assert!(history.to_string().contains("2:     return eval(name)"));
        match wire {
            "chat-completions" => {
                assert_eq!(history[2]["tool_calls"].as_array().unwrap().len(), 4);
                assert_eq!(history[5]["tool_call_id"], "bash-3");
            }
            "responses" => {
                assert_eq!(history[1]["encrypted_content"], "opaque-state");
                assert_eq!(history[8]["call_id"], "bash-3");
            }
            "anthropic" => {
                assert_eq!(history[1]["content"][0]["signature"], "signed-state");
                assert_eq!(history[2]["content"][2]["tool_use_id"], "bash-3");
            }
            _ => unreachable!(),
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["findings"][0]["path"],
            "helper.py"
        );
        assert!(dir.path().join("poc.sh").exists());
    }
}

#[test]
fn exhausted_budget_forces_a_final_report_without_tools() {
    for wire in ["chat-completions", "responses", "anthropic"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.py"), "from helper import run\n").unwrap();
        std::fs::write(
            dir.path().join("helper.py"),
            "def run(name):\n    return eval(name)\n",
        )
        .unwrap();
        // The first turn consumes the whole budget; the next request must drop
        // the tool definitions so the model has to return its final report.
        let (endpoint, rx, task) = server(vec![tool_turn(wire), final_turn(wire)]);
        let output = run(wire, dir.path(), &endpoint, "4");
        assert!(
            output.status.success(),
            "{wire}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        task.join().unwrap();
        let first = rx.recv().unwrap();
        assert_eq!(first["tools"].as_array().unwrap().len(), 7);
        let second = rx.recv().unwrap();
        assert!(
            second.get("tools").is_none(),
            "{wire} still offered tools after the budget"
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["findings"][0]["cwe"], "CWE-95");
    }
}

#[test]
fn over_budget_batch_fails_before_any_tool_mutation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("entry.py"), "pass\n").unwrap();
    let (endpoint, rx, task) = server(vec![tool_turn("chat-completions")]);
    let output = run("chat-completions", dir.path(), &endpoint, "1");
    assert_eq!(output.status.code(), Some(2));
    rx.recv().unwrap();
    task.join().unwrap();
    assert!(!dir.path().join("poc.sh").exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("max-tool-calls"));
}

#[test]
fn dry_run_explores_directories_without_credentials_and_reports_prompt_cost() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "pass\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_alchemist"))
        .args(["audit", "--root"])
        .arg(dir.path())
        .args(["--target", ".", "--model", "preview", "--dry-run"])
        .env_remove("AUDIT_API_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["files"], 0);
    assert!(report["tools"].as_array().unwrap().contains(&json!("bash")));
    assert!(report["tools"]
        .as_array()
        .unwrap()
        .contains(&json!("load_skill")));
    assert!(report["estimated_prompt_tokens"].as_u64().unwrap() > 0);
}

#[test]
fn incomplete_tool_streams_never_execute_calls() {
    for (wire, body) in [
        (
            "chat-completions",
            tool_turn("chat-completions").replace("tool_calls\"}", "length\"}"),
        ),
        (
            "chat-completions",
            tool_turn("chat-completions").replace("data: [DONE]\n\n", ""),
        ),
        (
            "responses",
            tool_turn("responses").replace("response.completed", "response.incomplete"),
        ),
        (
            "anthropic",
            tool_turn("anthropic").replace(&sse(json!({"type":"message_stop"})), ""),
        ),
        (
            "anthropic",
            tool_turn("anthropic").replace(
                "\"stop_reason\":\"tool_use\"",
                "\"stop_reason\":\"max_tokens\"",
            ),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.py"), "pass\n").unwrap();
        let (endpoint, rx, task) = server(vec![body]);
        // The complete fixture contains four calls. Allow all four so a budget
        // rejection cannot hide a broken incomplete-stream check.
        let output = run(wire, dir.path(), &endpoint, "4");
        assert_eq!(output.status.code(), Some(2), "{wire}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["outcome"], "provider_error");
        assert!(!report["error"]
            .as_str()
            .unwrap()
            .contains("exhausted --max-tool-calls"));
        rx.recv().unwrap();
        task.join().unwrap();
        assert!(!dir.path().join("poc.sh").exists());
    }
}

#[test]
fn duplicate_tool_ids_never_execute_mutations_on_any_wire() {
    for wire in ["chat-completions", "responses", "anthropic"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.py"), "pass\n").unwrap();
        let body = tool_turn(wire).replace("\"write-2\"", "\"read-1\"");
        let (endpoint, rx, task) = server(vec![body]);
        let output = run(wire, dir.path(), &endpoint, "4");
        assert_eq!(output.status.code(), Some(2), "{wire}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["outcome"], "provider_error");
        assert!(
            report["error"]
                .as_str()
                .unwrap()
                .contains("duplicate tool call"),
            "{wire}: {report}"
        );
        rx.recv().unwrap();
        task.join().unwrap();
        assert!(!dir.path().join("poc.sh").exists(), "{wire}");
    }
}

#[test]
fn evaluation_workspaces_exclude_labels_and_do_not_share_writes() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("dataset.json");
    std::fs::write(&manifest, "private expected labels").unwrap();
    std::fs::write(dir.path().join("a.py"), "pass\n").unwrap();
    std::fs::write(dir.path().join("config.json"), "{}").unwrap();
    let first = audit_harness::dataset::stage_workspace(dir.path(), &manifest).unwrap();
    let second = audit_harness::dataset::stage_workspace(dir.path(), &manifest).unwrap();
    assert!(!first.path().join("dataset.json").exists());
    assert!(first.path().join("config.json").exists());
    std::fs::write(first.path().join("a.py"), "modified").unwrap();
    assert_eq!(
        std::fs::read_to_string(second.path().join("a.py")).unwrap(),
        "pass\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
        "pass\n"
    );
}
