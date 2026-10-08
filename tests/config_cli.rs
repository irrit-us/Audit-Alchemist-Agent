mod support;
use audit_harness::{config::FileConfig, mcp::McpTools};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn cli(root: &Path, command: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_audit-harness"));
    cmd.current_dir(root)
        .arg(command)
        .arg("--config")
        .arg(root.join("node.toml"));
    cmd.env("AUDIT_API_KEY", "fixture-key");
    cmd
}
fn config(root: &Path, extra: &str) {
    std::fs::write(
        root.join("node.toml"),
        format!("schema_version = 1\n{extra}"),
    )
    .unwrap();
}
fn success(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn mcp_config(root: &Path, mode: &str) -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_server.py");
    format!("[mcp.fixture]\nenabled = true\ncommand = {:?}\nargs = [{:?}, {:?}, {:?}]\ntimeout_ms = 1500\ntools = [\"echo\"]\n", if cfg!(windows) { "python" } else { "python3" }, fixture.to_string_lossy(), mode, root.join("mcp.jsonl").to_string_lossy())
}
fn final_turn() -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{"content":json!({"schema_version":1,"findings":[]}).to_string()},"finish_reason":"stop"}]})
    )
}

#[test]
fn standalone_config_paths_overrides_and_lazy_skills() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("entry.py"), "print('fixture')\n").unwrap();
    std::fs::write(root.join("guide.md"), "CUSTOM_BODY_NOT_PRELOADED").unwrap();
    config(
        root,
        &format!(
            r#"
[cli]
model = "from-file"
target = "entry.py"
root = "."
max_tool_calls = 3
dry_run = true
agent_arg = ["--fixture-option", "literal with spaces", "$(literal)"]
[prompts]
append = "Focus on the current task."
[tools]
enabled = ["bash", "read_file", "load_skill"]
[skills]
enabled = ["local-guide"]
[[skills.custom]]
name = "local-guide"
description = "Local guidance"
file = "guide.md"
[skills.custom.resources]
"scripts/check.sh" = "printf checked\n"
{}
"#,
            mcp_config(root, "echo")
        ),
    );
    let checked = success(cli(root, "check-config").output().unwrap());
    assert_eq!(checked["mcp_servers"], json!(["fixture"]));
    let preview = success(
        cli(root, "audit")
            .current_dir(std::env::temp_dir())
            .args(["--model", "override", "--max-tool-calls", "7"])
            .output()
            .unwrap(),
    );
    assert_eq!(preview["model"], "override");
    assert_eq!(preview["max_tool_calls"], 7);
    assert_eq!(preview["files"], 1);
    assert_eq!(preview["tools"], json!(["load_skill", "bash", "read_file"]));
    assert_eq!(preview["skills"].as_array().unwrap().len(), 1);
    assert!(!preview.to_string().contains("CUSTOM_BODY"));
    assert!(!root.join("mcp.jsonl").exists());
    let guide = success(cli(root, "skills").arg("local-guide").output().unwrap());
    assert_eq!(guide["content"], "CUSTOM_BODY_NOT_PRELOADED");
    assert!(!cli(root, "skills")
        .arg("poc-validation")
        .output()
        .unwrap()
        .status
        .success());
    let resource = success(
        cli(root, "skills")
            .args(["local-guide", "--resource", "scripts/check.sh"])
            .output()
            .unwrap(),
    );
    assert_eq!(resource["content"], "printf checked\n");
}

#[test]
fn invalid_configs_fail_before_execution_even_with_overrides() {
    let dir = tempfile::tempdir().unwrap();
    for body in [
        "schema_version = 2",
        "schema_version = 1\nunknown = true",
        "schema_version = 1\n[cli]\nmax_tool_calls = 0",
        "schema_version = 1\n[cli]\nmax_tool_calls = true",
        "schema_version = 1\n[cli]\nmodel_typo = 'x'",
        "schema_version = 1\n[tools]\nenabled = ['unknown']",
        "schema_version = 1\n[tools]\nenabled = ['bash', 'bash']",
        "schema_version = 1\n[prompts]\nsystem = 'x'\nsystem_file = 'missing'",
        "schema_version = 1\n[skills]\nenabled = ['missing']",
        "schema_version = 1\n[mcp.x]\ncommand = 'x'\ntransport = 'http'",
        "schema_version = 1\n[mcp.x]\ncommand = 'x'\ntimeout_ms = 0",
    ] {
        std::fs::write(dir.path().join("node.toml"), body).unwrap();
        let output = cli(dir.path(), "check-config").output().unwrap();
        assert!(!output.status.success(), "{body}");
        assert!(output.stdout.is_empty());
    }
    config(dir.path(), "[cli]\nmax_tool_calls = 0");
    assert!(!cli(dir.path(), "audit")
        .args([
            "--max-tool-calls",
            "7",
            "--model",
            "x",
            "--target",
            ".",
            "--dry-run"
        ])
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn boolean_cli_override_disables_configured_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("entry.py"), "print('fixture')\n").unwrap();
    let (endpoint, _rx, server) = server(vec![final_turn()]);
    config(dir.path(), &format!("[cli]\nmodel = 'fixture'\ntarget = 'entry.py'\nendpoint = {endpoint:?}\ndry_run = true\n"));
    let result = success(
        cli(dir.path(), "audit")
            .arg("--dry-run=false")
            .output()
            .unwrap(),
    );
    assert_eq!(result["outcome"], "success");
    server.join().unwrap();
}

#[test]
fn mcp_tools_use_native_responses_and_anthropic_continuation() {
    for wire in ["responses", "anthropic"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("entry.py"), "print('fixture')\n").unwrap();
        let sse = |v: Value| format!("data: {v}\n\n");
        let call = if wire == "responses" {
            sse(
                json!({"type":"response.completed","response":{"output":[{"type":"function_call","id":"item-1","call_id":"call-1","name":"mcp_fixture__echo","arguments":"{}","status":"completed"}]}}),
            )
        } else {
            sse(
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call-1","name":"mcp_fixture__echo","input":{}}}),
            ) + &sse(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            ) + &sse(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}))
                + &sse(json!({"type":"message_stop"}))
        };
        let answer = json!({"schema_version":1,"findings":[]}).to_string();
        let final_body = if wire == "responses" {
            sse(
                json!({"type":"response.completed","response":{"output":[{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":answer}]}]}}),
            )
        } else {
            sse(json!({"type":"content_block_delta","delta":{"type":"text_delta","text":answer}}))
                + &sse(json!({"type":"message_stop"}))
        };
        let (endpoint, rx, server) = server(vec![call, final_body]);
        config(root, &format!("[cli]\nmodel = 'fixture'\ntarget = 'entry.py'\nendpoint = {endpoint:?}\nwire_api = {wire:?}\n[tools]\nenabled = []\n{}", mcp_config(root, "echo")));
        let result = success(cli(root, "audit").output().unwrap());
        assert_eq!(result["outcome"], "success");
        server.join().unwrap();
        let first = rx.recv().unwrap();
        assert_eq!(first["tools"][0]["name"], "mcp_fixture__echo");
        let second = rx.recv().unwrap();
        if wire == "responses" {
            let item = second["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["type"] == "function_call_output")
                .unwrap();
            assert_eq!(item["call_id"], "call-1");
            let output: Value = serde_json::from_str(item["output"].as_str().unwrap()).unwrap();
            assert_eq!(output["content"][0]["text"], "echo");
        } else {
            let messages = second["messages"].as_array().unwrap();
            let item = &messages.last().unwrap()["content"][0];
            assert_eq!(item["tool_use_id"], "call-1");
            let output: Value = serde_json::from_str(item["content"].as_str().unwrap()).unwrap();
            assert_eq!(output["content"][0]["text"], "echo");
        }
    }
}

#[test]
fn cli_agent_runs_mcp_and_custom_skills_with_monitoring() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("entry.py"), "print('fixture')\n").unwrap();
    let calls = [
        ("mcp_fixture__echo", json!({"text":"MCP_OBSERVED"})),
        ("load_skill", json!({"name":"local"})),
        ("bash", json!({"command":"touch should-not-exist"})),
    ];
    let batch: Vec<_> = calls.iter().enumerate().map(|(i, (name,args))| json!({"index":i,"id":format!("call-{i}"),"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect();
    let tool_turn = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{"tool_calls":batch},"finish_reason":"tool_calls"}]})
    );
    let (endpoint, rx, server) = server(vec![tool_turn, final_turn()]);
    config(
        root,
        &format!(
            r#"
[cli]
model = "fixture"
endpoint = {endpoint:?}
max_attempts = 1
timeout_ms = 8000
trace_dir = "traces"
[prompts]
system = "CONFIGURED_SYSTEM"
append = "CONFIGURED_APPEND"
[tools]
enabled = ["load_skill"]
[skills]
enabled = ["local"]
[[skills.custom]]
name = "local"
description = "Local guidance"
content = "ON_DEMAND_BODY"
{}
[mcp.fixture.env_from]
FIXTURE_MAPPED = "TEST_MCP_SOURCE"
"#,
            mcp_config(root, "pages")
        ),
    );
    let mut child = cli(root, "agent")
        .env("TEST_MCP_SOURCE", "mapped-value")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"schema_version":1,"case_id":"fixture","target":"entry.py","instruction":"Inspect this task."}).to_string().as_bytes()).unwrap();
    let report = success(child.wait_with_output().unwrap());
    assert_eq!(report["findings"], json!([]));
    server.join().unwrap();
    let first = rx.recv().unwrap();
    assert_eq!(
        first["messages"][0]["content"],
        "CONFIGURED_SYSTEM\n\nCONFIGURED_APPEND"
    );
    assert_eq!(first["tools"].as_array().unwrap().len(), 2);
    assert!(!first.to_string().contains("ON_DEMAND_BODY"));
    assert!(!first.to_string().contains("SERVER_INSTRUCTIONS"));
    let second = rx.recv().unwrap();
    let results: Vec<Value> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["role"] == "tool")
        .map(|v| serde_json::from_str(v["content"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(results[0]["content"][0]["text"], "MCP_OBSERVED");
    assert_eq!(
        results[0]["structuredContent"]["mapped_env"],
        "mapped-value"
    );
    assert_eq!(results[1]["content"], "ON_DEMAND_BODY");
    assert!(results[2]["error"].as_str().unwrap().contains("disabled"));
    assert!(!root.join("should-not-exist").exists());
    let log = std::fs::read_to_string(root.join("mcp.jsonl")).unwrap();
    assert!(log.contains("\"closed\": true"));
    assert!(log.contains("-32601"));
    let trace = std::fs::read_dir(root.join("traces"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let journal = std::fs::read_to_string(trace).unwrap();
    assert!(journal.contains("mcp_fixture__echo"));
    assert!(!journal.contains("MCP_OBSERVED"));
}

#[test]
fn evaluate_forwards_resolved_config_into_staged_workspaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("entry.py"), "print('fixture')\n").unwrap();
    std::fs::write(root.join("prompt.txt"), "EVALUATION_CONFIG_PROMPT").unwrap();
    std::fs::write(root.join("dataset.json"), json!({"schema_version":1,"name":"config","cases":[{"id":"only","target":"entry.py","instruction":"Inspect.","expected":[]}]}).to_string()).unwrap();
    let (endpoint, rx, server) = server(vec![final_turn()]);
    config(
        root,
        &format!(
            r#"
[cli]
model = "fixture"
endpoint = {endpoint:?}
dataset = "dataset.json"
timeout_ms = 8000
[prompts]
system_file = "prompt.txt"
[tools]
enabled = []
{}
"#,
            mcp_config(root, "echo")
        ),
    );
    let report = success(cli(root, "evaluate").output().unwrap());
    assert_eq!(report["metrics"]["failed_cases"], 0);
    server.join().unwrap();
    let request = rx.recv().unwrap();
    assert_eq!(
        request["messages"][0]["content"],
        "EVALUATION_CONFIG_PROMPT"
    );
    assert_eq!(request["tools"].as_array().unwrap().len(), 1);
    let log = std::fs::read_to_string(root.join("mcp.jsonl")).unwrap();
    let started: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_ne!(Path::new(started["cwd"].as_str().unwrap()), root);
}

#[tokio::test]
async fn mcp_failures_are_bounded_and_poisoned_sessions_are_not_retried() {
    for mode in [
        "bad_version",
        "duplicate",
        "cursor_loop",
        "wrong_id",
        "hang_init",
    ] {
        let dir = tempfile::tempdir().unwrap();
        config(dir.path(), &mcp_config(dir.path(), mode));
        let config = FileConfig::load(&dir.path().join("node.toml")).unwrap();
        let started = Instant::now();
        assert!(
            McpTools::connect(&config.mcp, dir.path()).await.is_err(),
            "{mode}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    for mode in [
        "malformed",
        "bad_result",
        "oversized",
        "hang_call",
        "descendant",
        "tool_error",
    ] {
        let dir = tempfile::tempdir().unwrap();
        config(dir.path(), &mcp_config(dir.path(), mode));
        let config = FileConfig::load(&dir.path().join("node.toml")).unwrap();
        let mut tools = McpTools::connect(&config.mcp, dir.path()).await.unwrap();
        let budget = Duration::from_millis(400);
        let result = tools.execute("mcp_fixture__echo", "{}", budget).await;
        assert!(result.get("error").is_some(), "{mode}: {result}");
        let again = tools.execute("mcp_fixture__echo", "{}", budget).await;
        if mode != "tool_error" {
            assert!(again["error"].as_str().unwrap().contains("unavailable"));
        }
        tools.shutdown().await;
        if mode == "descendant" {
            tokio::time::sleep(Duration::from_millis(2200)).await;
            assert!(!dir.path().join("mcp.jsonl.leaked").exists());
        }
    }
}

#[test]
fn cli_deadline_cancels_mcp_initialization_and_its_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("entry.py"), "print('fixture')\n").unwrap();
    config(root, &format!("[cli]\nmodel = 'fixture'\ntarget = 'entry.py'\nendpoint = 'http://127.0.0.1:9/unused'\ntimeout_ms = 800\n{}", mcp_config(root, "cancel_init")));
    let start = Instant::now();
    let output = cli(root, "audit").output().unwrap();
    assert!(!output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["outcome"], "timeout", "{result}");
    assert!(start.elapsed() < Duration::from_secs(4));
    let log = std::fs::read_to_string(root.join("mcp.jsonl")).unwrap();
    assert!(log.contains("initialize"));
    thread::sleep(Duration::from_millis(2200));
    assert!(!root.join("mcp.jsonl.leaked").exists());
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
