//! Test observable process effects without changing Bash's command language.
use audit_harness::{context::Context, tools::WorkspaceTools};
use serde_json::json;
use std::time::Duration;

fn workspace(root: &std::path::Path) -> WorkspaceTools {
    WorkspaceTools::new(
        root,
        &Context {
            sources: vec![],
            total_bytes: 0,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn cancelling_bash_terminates_started_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = workspace(dir.path());
    let arguments = json!({"command":
        "(printf ready > ready; sleep 2; printf leaked > late) & wait"
    })
    .to_string();
    let mut operation = Box::pin(tools.execute("bash", &arguments, Duration::from_secs(10)));
    // Wait for the descendant itself, not a guessed process-start delay.
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut operation => panic!("Bash ended before cancellation: {result}"),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    if std::fs::read(dir.path().join("ready")).is_ok_and(|v| v == b"ready") {
                        break;
                    }
                }
            }
        }
    })
    .await;
    drop(operation);
    ready.expect("descendant did not become ready");
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(
        !dir.path().join("late").exists(),
        "cancelled descendant ran"
    );
}

#[tokio::test]
async fn successful_bash_cleans_up_redirected_background_children() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = workspace(dir.path());
    let result = tools
        .execute(
            "bash",
            &json!({"command":
                "(printf ready > ready; sleep 2; printf leaked > late) >/dev/null 2>&1 & while [ ! -s ready ]; do sleep 0.01; done"
            })
            .to_string(),
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(result["exit_code"], 0, "{result}");
    assert_eq!(std::fs::read(dir.path().join("ready")).unwrap(), b"ready");
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(
        !dir.path().join("late").exists(),
        "background child survived"
    );
}

#[tokio::test]
async fn bash_drains_both_pipes_and_exposes_end_of_large_output() {
    let dir = tempfile::tempdir().unwrap();
    let result = workspace(dir.path())
        .execute(
            "bash",
            &json!({"command":
                "for ((i=0;i<10000;i++)); do printf 'out0123456789\\n'; printf 'err0123456789\\n' >&2; done; printf stdout-end; printf stderr-end >&2"
            })
            .to_string(),
            Duration::from_secs(10),
        )
        .await;
    assert_eq!(result["exit_code"], 0, "{result}");
    assert_eq!(result["timed_out"], false);
    assert_eq!(result["truncated"], true);
    for (key, prefix, suffix) in [
        ("stdout", "out0123456789\n", "stdout-end"),
        ("stderr", "err0123456789\n", "stderr-end"),
    ] {
        let text = result[key].as_str().unwrap();
        assert!(text.starts_with(prefix), "{key}");
        assert!(text.ends_with(suffix), "{key}");
        assert!(text.contains("output truncated"));
        assert!(text.len() < 34_000);
    }
}

#[tokio::test]
async fn invalid_tool_arguments_never_mutate_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.py"), "original\noriginal\n").unwrap();
    let mut tools = workspace(dir.path());
    for (tool, args) in [
        (
            "bash",
            json!({"command":"printf changed > source.py","timeout_ms":0}),
        ),
        (
            "bash",
            json!({"command":"printf changed > source.py","unexpected":true}),
        ),
        (
            "write_file",
            json!({"path":"source.py","content":"changed","unexpected":true}),
        ),
        ("write_file", json!({"path":"source.py","content":42})),
        (
            "edit_file",
            json!({"path":"source.py","old_text":"original","new_text":"changed"}),
        ),
        (
            "edit_file",
            json!({"path":"source.py","old_text":"","new_text":"changed"}),
        ),
    ] {
        let result = tools
            .execute(tool, &args.to_string(), Duration::from_secs(5))
            .await;
        assert!(result.get("error").is_some(), "{tool}: {result}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("source.py")).unwrap(),
            "original\noriginal\n"
        );
    }
}
