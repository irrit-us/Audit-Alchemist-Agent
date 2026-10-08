//! Tests for the foundational context, provider, and progress modules.

use audit_harness::{
    context::{
        self, estimate_tokens,
        tools::{SearchLimits, SourceRoot, WalkLimits},
        ContextBudget,
    },
    progress::Progress,
    protocol::{Finding, Severity},
    provider::{auth, responses::SseParser, retry},
};
use std::time::Duration;

/// A small mixed source tree with a pruned dependency directory.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("pkg/sub")).unwrap();
    std::fs::create_dir_all(dir.path().join("pkg/node_modules/x")).unwrap();
    std::fs::write(dir.path().join("pkg/a.py"), "print('a')\n").unwrap();
    std::fs::write(dir.path().join("pkg/sub/b.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("pkg/data.txt"), "not source\n").unwrap();
    std::fs::write(dir.path().join("pkg/node_modules/x/skip.js"), "skip\n").unwrap();
    dir
}

fn finding(path: &str, line: u32) -> Finding {
    Finding {
        cwe: "CWE-78".into(),
        path: path.into(),
        line,
        severity: Severity::High,
        title: "title".into(),
        evidence: "evidence".into(),
    }
}

#[test]
fn walk_is_deterministic_and_prunes_dependencies() {
    let dir = fixture();
    let root = SourceRoot::open(dir.path()).unwrap();
    let paths: Vec<_> = root
        .walk("pkg", WalkLimits::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    assert_eq!(paths, vec!["pkg/a.py", "pkg/sub/b.rs"]);
}

#[test]
fn walk_and_read_enforce_bounds() {
    let dir = fixture();
    let root = SourceRoot::open(dir.path()).unwrap();
    assert!(root
        .walk(
            "pkg",
            WalkLimits {
                max_entries: 100,
                max_files: 1,
            }
        )
        .is_err());
    assert!(root.read("pkg/a.py", 2).is_err());
    assert_eq!(root.read("pkg/a.py", 64).unwrap().content, "print('a')\n");
    assert!(root.read("../escape.py", 64).is_err());
    assert!(root.resolve("/absolute.py").is_err());
}

#[test]
fn search_reports_matches_with_limits() {
    let dir = fixture();
    let root = SourceRoot::open(dir.path()).unwrap();
    let matches = root
        .search("pkg", "print", SearchLimits::default())
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, "pkg/a.py");
    assert_eq!(matches[0].line, 1);
    assert!(root.search("pkg", "", SearchLimits::default()).is_err());
    assert!(root
        .search(
            "pkg",
            "print",
            SearchLimits {
                max_matches: 0,
                ..SearchLimits::default()
            }
        )
        .is_err());
}

#[test]
fn context_budget_and_finding_validation() {
    let dir = fixture();
    let budget = ContextBudget {
        max_bytes: 1024,
        max_files: 128,
        max_entries: 1_000,
    };
    let context = context::build(dir.path(), "pkg", &budget).unwrap();
    assert_eq!(context.files(), 2);
    assert_eq!(
        context.total_bytes(),
        "print('a')\n".len() + "fn main() {}\n".len()
    );
    context.validate_finding(&finding("pkg/a.py", 1)).unwrap();
    assert!(context.validate_finding(&finding("pkg/a.py", 99)).is_err());
    assert!(context.validate_finding(&finding("missing.py", 1)).is_err());

    let tight = ContextBudget {
        max_bytes: 5,
        ..budget
    };
    assert!(context::build(dir.path(), "pkg", &tight).is_err());
}

#[test]
fn progress_counts_across_threads() {
    let progress = Progress::new(64);
    std::thread::scope(|scope| {
        for _ in 0..64 {
            let progress = progress.clone();
            scope.spawn(move || {
                progress.begin("case");
                progress.finish("case", "success", false, 0);
            });
        }
    });
    let counts = progress.counts();
    assert_eq!(counts.total, 64);
    assert_eq!(counts.started, 64);
    assert_eq!(counts.completed, 64);
    assert_eq!(counts.failed, 0);
    assert_eq!(counts.remaining(), 0);
    assert_eq!(counts.succeeded(), 64);
}

#[test]
fn progress_tracks_failed_cases() {
    let progress = Progress::new(3);
    progress.begin("a");
    progress.finish("a", "timeout", true, 10);
    progress.finish("b", "success", false, 20);
    let counts = progress.counts();
    assert_eq!(counts.failed, 1);
    assert_eq!(counts.completed, 2);
    assert_eq!(counts.remaining(), 1);
    assert_eq!(counts.succeeded(), 1);
}

#[test]
fn sse_parser_streams_deltas_for_codex() {
    let mut parser = SseParser::default();
    parser
        .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n")
        .unwrap();
    parser.push(b"data: [DONE]\n\n").unwrap();
    let output = parser.finish().unwrap();
    assert!(output.completed);
    assert_eq!(output.text, "hi");
}

#[test]
fn explicit_auth_path_wins_and_credentials_redact() {
    let explicit = std::path::PathBuf::from("/tmp/audit-auth-test.json");
    assert_eq!(auth::auth_path(Some(explicit.clone())).unwrap(), explicit);
    let credentials = auth::Credentials {
        bearer: "super-secret".into(),
        account_id: Some("acc_1".into()),
    };
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("super-secret"));
    assert!(debug.contains("acc_1"));
}

#[test]
fn retry_policy_is_bounded_and_honors_hints() {
    assert!(retry::retryable_status(429));
    assert!(retry::retryable_status(503));
    assert!(!retry::retryable_status(400));
    assert!(!retry::retryable_status(401));
    let policy = retry::RetryPolicy::from_millis(3, 100, 1_000);
    assert!(policy.allows_retry(0));
    assert!(policy.allows_retry(1));
    assert!(!policy.allows_retry(2));
    assert!(!retry::RetryPolicy::disabled().allows_retry(0));
    assert_eq!(retry::parse_retry_after("3"), Some(Duration::from_secs(3)));
    assert_eq!(retry::parse_retry_after("nope"), None);
    // A server hint is capped by the configured ceiling.
    assert_eq!(
        retry::backoff(&policy, 0, Some(Duration::from_secs(30)), 1),
        Duration::from_millis(1_000)
    );
    // Jitter never exceeds the exponential ceiling.
    assert!(retry::backoff(&policy, 5, None, 1) <= Duration::from_millis(1_000));
}

#[test]
fn token_estimate_is_a_documented_heuristic() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    let budget = ContextBudget::new(1024);
    assert_eq!(budget.max_bytes, 1024);
}
