//! Incremental parser for the Codex `/responses` Server-Sent Events stream.
//!
//! The ChatGPT backend streams typed events. This module buffers bytes, splits
//! complete lines, and accumulates assistant text from
//! `response.output_text.delta` events, with the final `response.completed`
//! payload as a fallback. It has no HTTP dependency, so the streaming behavior
//! is unit-tested in isolation from the network path.

use anyhow::{Context, Result};
use serde_json::Value;

/// Accumulated state from one Codex responses stream.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct CodexOutput {
    pub text: String,
    pub error: Option<String>,
    pub completed: bool,
}

/// A byte-fed SSE parser that decodes complete `data:` lines as they arrive.
#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    output: CodexOutput,
}

impl SseParser {
    /// Feed a chunk of stream bytes. Chunk boundaries may split lines or even
    /// multi-byte characters; only complete lines are decoded.
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(bytes);
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = std::str::from_utf8(&line[..line.len() - 1])
                .context("Codex stream is not UTF-8")?;
            self.output.consume(line)?;
        }
        Ok(())
    }

    /// Flush any trailing line and return the accumulated output.
    pub fn finish(mut self) -> Result<CodexOutput> {
        if !self.buffer.is_empty() {
            let line = String::from_utf8(std::mem::take(&mut self.buffer))
                .context("Codex stream is not UTF-8")?;
            self.output.consume(line.trim_end_matches(['\r', '\n']))?;
        }
        Ok(self.output)
    }
}

impl CodexOutput {
    fn consume(&mut self, line: &str) -> Result<()> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(());
        };
        let data = data.trim_start();
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            self.completed = true;
            return Ok(());
        }
        let event: Value = serde_json::from_str(data).context("invalid Codex stream event")?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                }
            }
            Some("response.output_text.done") => {
                if self.text.is_empty() {
                    if let Some(text) = event.get("text").and_then(Value::as_str) {
                        self.text.push_str(text);
                    }
                }
            }
            Some("response.completed") => {
                self.completed = true;
                if self.text.is_empty() {
                    self.text = completed_text(&event);
                }
            }
            Some("response.failed") | Some("response.incomplete") | Some("error") => {
                self.error = Some(error_message(&event));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Assemble the final text from a `response.completed` event.
fn completed_text(event: &Value) -> String {
    let Some(output) = event.pointer("/response/output").and_then(Value::as_array) else {
        return String::new();
    };
    let mut text = String::new();
    for item in output {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                    text.push_str(chunk);
                }
            }
        }
    }
    text
}

fn error_message(event: &Value) -> String {
    event
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .or_else(|| event.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| event.get("message").and_then(Value::as_str))
        .unwrap_or("unknown Codex error")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_deltas_across_chunks() {
        let mut parser = SseParser::default();
        parser
            .push(
                b"event: response.output_text.delta\n\
                  data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"schema_\"}\n\n",
            )
            .unwrap();
        parser
            .push(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"version\\\":1,\\\"findings\\\":[]}\"}\n\n",
            )
            .unwrap();
        parser
            .push(b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n")
            .unwrap();
        let output = parser.finish().unwrap();
        assert!(output.completed);
        assert_eq!(output.text, "{\"schema_version\":1,\"findings\":[]}");
    }

    #[test]
    fn reads_completed_output_as_fallback() {
        let mut parser = SseParser::default();
        parser
            .push(b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"text\":\"hello\"},{\"text\":\" world\"}]}]}}\n")
            .unwrap();
        let output = parser.finish().unwrap();
        assert!(output.completed);
        assert_eq!(output.text, "hello world");
    }

    #[test]
    fn reports_errors() {
        let mut parser = SseParser::default();
        parser
            .push(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom\"}}}\n")
            .unwrap();
        let output = parser.finish().unwrap();
        assert_eq!(output.error.as_deref(), Some("boom"));
    }

    #[test]
    fn handles_done_and_crlf_and_trailing_line() {
        let mut parser = SseParser::default();
        parser.push(b"data: [DONE]\r\n").unwrap();
        assert!(parser.finish().unwrap().completed);

        let mut trailing = SseParser::default();
        trailing
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}")
            .unwrap();
        assert_eq!(trailing.finish().unwrap().text, "x");
    }

    #[test]
    fn rejects_malformed_events() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"data: {not json}\n").is_err());
    }
}
