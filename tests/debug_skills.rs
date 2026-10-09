use audit_harness::{context::Context, skills, tools::WorkspaceTools};
use serde_json::{json, Value};
use std::{process::Command, time::Duration};

#[tokio::test]
async fn skill_scripts_export_exact_bytes_without_overwriting_or_traversing() {
    let dir = tempfile::tempdir().unwrap();
    let context = Context {
        sources: vec![],
        total_bytes: 0,
    };
    let mut tools = WorkspaceTools::new(dir.path(), &context).unwrap();
    for (name, resource, destination) in [
        ("foundry-debugging", "scripts/RawDebug.sol", "RawDebug.sol"),
        ("gdb-debugging", "scripts/capture.gdb", "capture.gdb"),
        ("node-inspector", "scripts/inspect.mjs", "inspect.mjs"),
        (
            "pwntools-debugging",
            "scripts/tube_probe.py",
            "tube_probe.py",
        ),
    ] {
        let arguments = json!({"name":name,"resource":resource,"save_to":destination}).to_string();
        let result = tools
            .execute("load_skill", &arguments, Duration::from_secs(1))
            .await;
        assert!(result.get("error").is_none(), "{result}");
        assert!(
            result.get("content").is_none(),
            "exports should not consume model context with source"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(destination)).unwrap(),
            skills::load(name, Some(resource)).unwrap()["content"]
        );
        std::fs::write(dir.path().join(destination), "user-owned").unwrap();
        assert!(tools
            .execute("load_skill", &arguments, Duration::from_secs(1))
            .await
            .get("error")
            .is_some());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(destination)).unwrap(),
            "user-owned"
        );
    }
    for arguments in [
        json!({"name":"node-inspector","resource":"../../outside","save_to":"test.mjs"}),
        json!({"name":"node-inspector","resource":"scripts/inspect.mjs","save_to":"../outside.mjs"}),
        json!({"name":"node-inspector","resource":"scripts/inspect.mjs","save_to":"missing/inspect.mjs"}),
        json!({"name":"node-inspector","save_to":"inspect.mjs"}),
    ] {
        assert!(tools
            .execute("load_skill", &arguments.to_string(), Duration::from_secs(1))
            .await
            .get("error")
            .is_some());
    }
}

#[test]
fn cli_exports_raw_resources_from_any_directory_and_keeps_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_alchemist");
    let args = [
        "skills",
        "node-inspector",
        "--resource",
        "scripts/inspect.mjs",
        "--output",
        "inspect.mjs",
    ];
    let output = Command::new(binary)
        .current_dir(dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inspect.mjs")).unwrap(),
        skills::load("node-inspector", Some("scripts/inspect.mjs")).unwrap()["content"]
    );
    std::fs::write(dir.path().join("inspect.mjs"), "keep").unwrap();
    let output = Command::new(binary)
        .current_dir(dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inspect.mjs")).unwrap(),
        "keep"
    );
    let output = Command::new(binary)
        .current_dir(dir.path())
        .arg("skills")
        .output()
        .unwrap();
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 10);

    assert!(catalog
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item.get("content").is_none()));
}
