//! Bounded, append-only run journals and content-free operational events.
use crate::provider::events::{EventSink, StreamEvent, Usage};
use anyhow::{ensure, Context, Result};
use clap::Args;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const TRACE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Default, Args)]
pub struct MonitorOptions {
    /// Write a unique, bounded JSONL run journal in this directory.
    #[arg(long)]
    pub trace_dir: Option<PathBuf>,
    /// Include bounded model requests, responses, and tool payloads (may contain sensitive code).
    #[arg(long, requires_if("true", "trace_dir"), num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    pub debug_trace: bool,
}

pub fn operation(name: &str, details: Value) -> StreamEvent {
    StreamEvent::Operation {
        name: name.into(),
        details,
    }
}

fn add(a: Usage, b: Usage) -> Usage {
    Usage {
        prompt_tokens: a.prompt_tokens.saturating_add(b.prompt_tokens),
        completion_tokens: a.completion_tokens.saturating_add(b.completion_tokens),
        total_tokens: a.total_tokens.saturating_add(b.total_tokens),
        reasoning_tokens: a.reasoning_tokens.saturating_add(b.reasoning_tokens),
    }
}

pub struct Monitor<'a> {
    sink: &'a mut dyn EventSink,
    file: Option<File>,
    path: Option<PathBuf>,
    start: Instant,
    sequence: u64,
    bytes: usize,
    truncated: bool,
    error: Option<String>,
    ended: bool,
    debug: bool,
    secret: Option<String>,
    phase: String,
    turns: u64,
    tools: u64,
    tool_errors: u64,
    retries: u64,
    output_repairs: u64,
    empty_completions: u64,
    tool_budget_rejections: u64,
    completed_usage: Usage,
    current_usage: Usage,
}

impl<'a> Monitor<'a> {
    pub fn new(
        options: &MonitorOptions,
        api_key_env: &str,
        sink: &'a mut dyn EventSink,
    ) -> Result<Self> {
        ensure!(
            !options.debug_trace || options.trace_dir.is_some(),
            "--debug-trace requires --trace-dir"
        );
        let (file, path) = if let Some(dir) = &options.trace_dir {
            std::fs::create_dir_all(dir).context("create trace directory")?;
            let path = dir.join(format!("audit-{}.jsonl", crate::provider::session_id()));
            let mut open = OpenOptions::new();
            open.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.mode(0o600);
            }
            let file = open.open(&path).context("create run trace")?;
            (Some(file), Some(path))
        } else {
            (None, None)
        };
        Ok(Self {
            sink,
            file,
            path,
            start: Instant::now(),
            sequence: 0,
            bytes: 0,
            truncated: false,
            error: None,
            ended: false,
            debug: options.debug_trace,
            secret: std::env::var(api_key_env).ok().filter(|s| !s.is_empty()),
            phase: "starting".into(),
            turns: 0,
            tools: 0,
            tool_errors: 0,
            retries: 0,
            output_repairs: 0,
            empty_completions: 0,
            tool_budget_rejections: 0,
            completed_usage: Usage::default(),
            current_usage: Usage::default(),
        })
    }

    pub fn start(&mut self, model: &str, wire: &str, timeout_ms: u64, case_id: &str, target: &str) {
        self.on_event(&operation(
            "run_start",
            json!({"model":model,"wire_api":wire,
            "timeout_ms":timeout_ms,"trace_path":self.path,"debug_trace":self.debug,"case_id":case_id,"target":target}),
        ));
    }

    pub fn heartbeat(&mut self) {
        self.on_event(&operation(
            "heartbeat",
            json!({"phase":self.phase,"elapsed_ms":self.start.elapsed().as_millis() as u64,
            "turns":self.turns,"tools":self.tools,"retries":self.retries}),
        ));
    }

    pub fn finish(&mut self, outcome: &str) -> Result<()> {
        self.ended = true;
        self.on_event(&operation("run_end", json!({"outcome":outcome,"phase":self.phase,
            "elapsed_ms":self.start.elapsed().as_millis() as u64,"turns":self.turns,
            "tools":self.tools,"tool_errors":self.tool_errors,"retries":self.retries,
            "output_repairs":self.output_repairs,
            "empty_completions":self.empty_completions,
            "tool_budget_rejections":self.tool_budget_rejections,
            "usage":add(self.completed_usage, self.current_usage),"trace_truncated":self.truncated})));
        if let Some(error) = &self.error {
            anyhow::bail!("run trace write failed: {error}");
        }
        Ok(())
    }

    fn record(&mut self, event: &StreamEvent) {
        if self.file.is_none() || self.error.is_some() {
            return;
        }
        let terminal = matches!(event, StreamEvent::Operation { name, .. } if name == "run_end");
        if self.truncated && !terminal {
            return;
        }
        self.sequence += 1;
        let value = json!({"schema_version":1,"sequence":self.sequence,
            "timestamp_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            "elapsed_ms":self.start.elapsed().as_millis() as u64,"event":event});
        let mut line = serde_json::to_string(&value).expect("serialize trace event");
        line.push('\n');
        if self.bytes + line.len() > TRACE_BYTES - 8192 && !terminal {
            self.truncated = true;
            let marker = operation("trace_truncated", json!({"limit_bytes":TRACE_BYTES}));
            let mut data = serde_json::to_vec(&json!({"schema_version":1,"sequence":self.sequence,
                "timestamp_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                "elapsed_ms":self.start.elapsed().as_millis() as u64,"event":marker}))
            .unwrap();
            data.push(b'\n');
            self.write(&data);
            self.sink.on_event(&marker);
            return;
        }
        self.write(line.as_bytes());
    }

    fn write(&mut self, data: &[u8]) {
        let file = self.file.as_mut().expect("trace file");
        if let Err(error) = file.write_all(data).and_then(|_| file.flush()) {
            self.error = Some(error.to_string());
            tracing::error!("run trace write failed: {error}");
        } else {
            self.bytes += data.len();
        }
    }
}

impl EventSink for Monitor<'_> {
    fn on_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::Debug { stage, content } => {
                if self.debug {
                    let event = StreamEvent::Debug {
                        stage: stage.clone(),
                        content: debug_payload(content, self.secret.as_deref()),
                    };
                    self.record(&event);
                }
                return;
            }
            StreamEvent::Text { .. } | StreamEvent::Reasoning { .. } => {
                // Debug captures the assembled turn once instead of duplicating deltas.
                self.sink.on_event(event);
                return;
            }
            StreamEvent::Usage(usage) => {
                self.current_usage.merge(*usage);
                let cumulative = StreamEvent::Usage(add(self.completed_usage, self.current_usage));
                self.record(&cumulative);
                self.sink.on_event(&cumulative);
                return;
            }
            StreamEvent::ToolStart { name, .. } => {
                self.tools += 1;
                self.phase = format!("tool:{}", crate::tools::bounded_output(name, 128));
            }
            StreamEvent::ToolEnd { is_error, .. } => {
                self.tool_errors += u64::from(*is_error);
                self.phase = "between_turns".into();
            }
            StreamEvent::Operation { name, .. } if name == "turn_start" => {
                self.turns += 1;
                self.completed_usage = add(self.completed_usage, self.current_usage);
                self.current_usage = Usage::default();
                self.phase = "model".into();
            }
            StreamEvent::Operation { name, .. } if name == "retry" => {
                self.retries += 1;
            }
            StreamEvent::Operation { name, .. } if name == "output_repair" => {
                self.output_repairs += 1;
                self.phase = "repairing".into();
            }
            StreamEvent::Operation { name, .. } if name == "empty_completion_retry" => {
                self.empty_completions += 1;
            }
            StreamEvent::Operation { name, .. } if name == "tool_budget_rejected" => {
                self.tool_budget_rejections += 1;
            }
            StreamEvent::Operation { name, .. } if name == "turn_end" => {
                self.phase = "validating".into();
            }
            _ => {}
        }
        self.record(event);
        self.sink.on_event(event);
    }
}

impl Drop for Monitor<'_> {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.finish("cancelled");
        }
    }
}

/// Inspect a live or completed journal without displaying captured payloads.
pub fn inspect(path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((TRACE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= TRACE_BYTES, "trace exceeds 16 MiB");
    let partial = !bytes.is_empty() && !bytes.ends_with(b"\n");
    let mut count = 0;
    let mut last = Value::Null;
    let mut start = Value::Null;
    let mut end = Value::Null;
    let mut heartbeat = Value::Null;
    let mut previous = 0;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        let record: Value = serde_json::from_slice(line).context("invalid trace record")?;
        ensure!(record["schema_version"] == 1, "unsupported trace schema");
        let sequence = record["sequence"]
            .as_u64()
            .context("missing trace sequence")?;
        ensure!(sequence > previous, "trace sequence is not increasing");
        previous = sequence;
        let event = &record["event"];
        if event["name"] == "run_start" {
            start = event["details"].clone();
        }
        if event["name"] == "run_end" {
            end = event["details"].clone();
        }
        if event["type"] == "operation" && event["name"] == "heartbeat" {
            heartbeat = event["details"].clone();
        }
        if event["type"] != "debug" {
            last = json!({"type":event["type"],"name":event["name"],"elapsed_ms":record["elapsed_ms"]});
        }
        count += 1;
    }
    Ok(
        json!({"records":count,"partial_tail":partial,"complete":!end.is_null(),
        "status":if end.is_null() { "running_or_interrupted" } else { "finished" },
        "start":start,"summary":end,"last_heartbeat":heartbeat,"last_event":last}),
    )
}

/// Carries session events to the supervisor without borrowing its output sink.
pub(crate) struct QueueSink(pub tokio::sync::mpsc::UnboundedSender<StreamEvent>);
impl EventSink for QueueSink {
    fn on_event(&mut self, event: &StreamEvent) {
        let _ = self.0.send(event.clone());
    }
}

pub(crate) fn debug_payload(content: &str, key: Option<&str>) -> String {
    let mut content = content.to_owned();
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        let mut forms = vec![key.to_owned()];
        for _ in 0..3 {
            let encoded = serde_json::to_string(forms.last().unwrap()).unwrap();
            forms.push(encoded[1..encoded.len() - 1].to_owned());
        }
        for form in forms.iter().rev() {
            content = content.replace(form, "[REDACTED]");
        }
    }
    crate::tools::bounded_output(&content, crate::tools::OUTPUT_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Events(Vec<StreamEvent>);
    impl EventSink for Events {
        fn on_event(&mut self, event: &StreamEvent) {
            self.0.push(event.clone());
        }
    }

    #[test]
    fn trace_creation_and_write_errors_are_reported() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut discard = ();
        let invalid = MonitorOptions {
            trace_dir: Some(file.path().into()),
            debug_trace: false,
        };
        assert!(Monitor::new(&invalid, "UNSET_TEST_MONITOR_KEY", &mut discard).is_err());
        let dir = tempfile::tempdir().unwrap();
        let options = MonitorOptions {
            trace_dir: Some(dir.path().into()),
            debug_trace: false,
        };
        let mut monitor = Monitor::new(&options, "UNSET_TEST_MONITOR_KEY", &mut discard).unwrap();
        // A read-only handle deterministically simulates a failed append on every OS.
        monitor.file = Some(File::open(monitor.path.as_ref().unwrap()).unwrap());
        monitor.start("fixture", "responses", 1, "test", ".");
        assert!(monitor
            .finish("success")
            .unwrap_err()
            .to_string()
            .contains("trace write failed"));
    }

    #[test]
    fn metadata_omits_content_and_aggregates_partial_usage_across_turns() {
        let dir = tempfile::tempdir().unwrap();
        let options = MonitorOptions {
            trace_dir: Some(dir.path().into()),
            debug_trace: false,
        };
        let mut events = Events::default();
        let path;
        {
            let mut monitor =
                Monitor::new(&options, "UNSET_TEST_MONITOR_KEY", &mut events).unwrap();
            path = monitor.path.clone().unwrap();
            monitor.start("fixture", "responses", 1000, "test", ".");
            monitor.on_event(&operation("turn_start", json!({"turn":1})));
            for usage in [
                Usage {
                    prompt_tokens: 10,
                    ..Usage::default()
                },
                Usage {
                    completion_tokens: 4,
                    ..Usage::default()
                },
            ] {
                monitor.on_event(&StreamEvent::Usage(usage));
            }
            monitor.on_event(&StreamEvent::Text {
                text: "private-source".into(),
            });
            monitor.on_event(&StreamEvent::Reasoning {
                text: "private-reasoning".into(),
            });
            monitor.on_event(&StreamEvent::Debug {
                stage: "request".into(),
                content: "private-request".into(),
            });
            monitor.on_event(&operation("turn_start", json!({"turn":2})));
            monitor.on_event(&StreamEvent::Usage(Usage {
                prompt_tokens: 20,
                completion_tokens: 3,
                ..Usage::default()
            }));
            monitor.on_event(&StreamEvent::ToolStart {
                name: "bash".into(),
                call_id: "c1".into(),
            });
            monitor.on_event(&StreamEvent::ToolEnd {
                name: "bash".into(),
                call_id: "c1".into(),
                is_error: true,
                elapsed_ms: 7,
            });
            monitor.on_event(&operation("retry", json!({"attempt":1})));
            monitor.heartbeat();
            monitor.finish("success").unwrap();
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("private-"));
        let summary = inspect(&path).unwrap();
        assert_eq!(summary["summary"]["usage"]["total_tokens"], 37);
        assert_eq!(summary["summary"]["usage"]["prompt_tokens"], 30);
        assert_eq!(summary["summary"]["turns"], 2);
        assert_eq!(summary["summary"]["tool_errors"], 1);
        assert_eq!(summary["summary"]["retries"], 1);
        assert!(events
            .0
            .iter()
            .any(|e| matches!(e, StreamEvent::Usage(u) if u.total_tokens == 37)));
        assert!(!events
            .0
            .iter()
            .any(|e| matches!(e, StreamEvent::Debug { .. })));
    }

    #[test]
    fn debug_redacts_raw_and_json_escaped_keys_before_truncating() {
        let key = "fake-\"credential\\value";
        let encoded = serde_json::to_string(key).unwrap();
        let content = format!("{} {key} {encoded} tail", "x".repeat(16370));
        let captured = debug_payload(&content, Some(key));
        assert!(!captured.contains("credential"));
        let dir = tempfile::tempdir().unwrap();
        let mut events = Events::default();
        let options = MonitorOptions {
            trace_dir: Some(dir.path().into()),
            debug_trace: true,
        };
        let path;
        {
            let mut monitor =
                Monitor::new(&options, "UNSET_TEST_MONITOR_KEY", &mut events).unwrap();
            monitor.secret = Some(key.into());
            path = monitor.path.clone().unwrap();
            monitor.start("fixture", "responses", 1, "test", ".");
            monitor.on_event(&StreamEvent::Debug {
                stage: "request".into(),
                content,
            });
            monitor.finish("error").unwrap();
        }
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(!raw.contains("credential"));
        assert!(raw.contains("[REDACTED]"));
        assert!(!events
            .0
            .iter()
            .any(|e| matches!(e, StreamEvent::Debug { .. })));
    }

    #[test]
    fn trace_cap_preserves_terminal_summary_and_cancellation_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let options = MonitorOptions {
            trace_dir: Some(dir.path().into()),
            debug_trace: true,
        };
        let path;
        {
            let mut discard = ();
            let mut monitor =
                Monitor::new(&options, "UNSET_TEST_MONITOR_KEY", &mut discard).unwrap();
            path = monitor.path.clone().unwrap();
            monitor.start("fixture", "responses", 1, "test", ".");
            // Put the accounting near its cap without filling disk in the test.
            monitor.bytes = TRACE_BYTES - 8192 - 10;
            monitor.on_event(&StreamEvent::Debug {
                stage: "request".into(),
                content: "x".repeat(100),
            });
            assert!(monitor.truncated);
        }
        let summary = inspect(&path).unwrap();
        assert_eq!(summary["summary"]["outcome"], "cancelled");
        assert_eq!(summary["summary"]["trace_truncated"], true);
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("trace_truncated"));
    }

    #[test]
    fn unique_traces_and_partial_tail_are_inspectable_but_corruption_fails() {
        let dir = tempfile::tempdir().unwrap();
        let options = MonitorOptions {
            trace_dir: Some(dir.path().into()),
            debug_trace: false,
        };
        let mut discard = ();
        let mut first = Monitor::new(&options, "UNSET_TEST_MONITOR_KEY", &mut discard).unwrap();
        let mut other_discard = ();
        let second = Monitor::new(&options, "UNSET_TEST_MONITOR_KEY", &mut other_discard).unwrap();
        assert_ne!(first.path, second.path);
        first.start("fixture", "responses", 1, "test", ".");
        let path = first.path.clone().unwrap();
        assert_eq!(inspect(&path).unwrap()["complete"], false);
        // Simulate a process killed in the middle of an append.
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"schema")
            .unwrap();
        assert_eq!(inspect(&path).unwrap()["partial_tail"], true);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        assert!(inspect(&path).is_err());
    }
}
