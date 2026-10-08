//! Console rendering of the model stream and final findings.
//!
//! Human-facing output goes to stderr so the machine-readable report on stdout
//! stays intact. Formats:
//!
//! - `quiet`: nothing.
//! - `text` / `markdown`: the findings, rendered after the run.
//! - `cot`: a readable chain of thought (dim reasoning) followed by the answer.
//! - `body`: only the answer body, one timed line at a time with terminal
//!   control to rewrite the in-progress line.
//! - `json`: nothing on stderr (the JSON report already goes to stdout).
//! - `jsonl`: one JSON event per line.

use crate::{
    protocol::Response,
    provider::events::{EventSink, StreamEvent},
};
use clap::ValueEnum;
use std::{
    io::{self, Write},
    time::Instant,
};

/// How to render the live stream and final findings on stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Quiet,
    Text,
    Markdown,
    Cot,
    Body,
    Json,
    Jsonl,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Quiet => "quiet",
            OutputFormat::Text => "text",
            OutputFormat::Markdown => "markdown",
            OutputFormat::Cot => "cot",
            OutputFormat::Body => "body",
            OutputFormat::Json => "json",
            OutputFormat::Jsonl => "jsonl",
        }
    }

    /// Formats whose stdout contract is JSON only and need no stderr renderer.
    pub fn is_machine(self) -> bool {
        matches!(self, OutputFormat::Quiet | OutputFormat::Json)
    }

    /// Formats that consume streaming events.
    pub fn streams(self) -> bool {
        matches!(
            self,
            OutputFormat::Cot | OutputFormat::Body | OutputFormat::Jsonl
        )
    }
}

/// When to emit ANSI color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    pub fn resolve(self, is_tty: bool) -> bool {
        match self {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => is_tty,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ColorChoice::Auto => "auto",
            ColorChoice::Always => "always",
            ColorChoice::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Reasoning,
    Text,
}

/// A streaming console renderer. Implements [`EventSink`] and renders the
/// final findings through [`Renderer::finish`].
pub struct Renderer<W: Write> {
    format: OutputFormat,
    color: bool,
    tty: bool,
    out: W,
    line_started: Instant,
    line: String,
    channel: Channel,
}

impl<W: Write> Renderer<W> {
    pub fn new(format: OutputFormat, color: bool, tty: bool, out: W) -> Self {
        let now = Instant::now();
        Self {
            format,
            color,
            tty,
            out,
            line_started: now,
            line: String::new(),
            channel: Channel::Text,
        }
    }

    /// Render the final findings for `text` and `markdown` and flush.
    pub fn finish(&mut self, response: &Response) -> io::Result<()> {
        match self.format {
            OutputFormat::Text => self.render_findings(response, false)?,
            OutputFormat::Markdown => self.render_findings(response, true)?,
            OutputFormat::Cot | OutputFormat::Body => {
                self.flush_line(true)?;
            }
            OutputFormat::Quiet | OutputFormat::Json | OutputFormat::Jsonl => {}
        }
        self.out.flush()
    }

    fn write_event(&mut self, event: &StreamEvent) -> io::Result<()> {
        match (self.format, event) {
            (OutputFormat::Jsonl, event) => {
                serde_json::to_writer(&mut self.out, event)?;
                writeln!(self.out)?;
                self.out.flush()
            }
            (OutputFormat::Text | OutputFormat::Markdown, _) => Ok(()),
            (OutputFormat::Body, StreamEvent::Text { text }) => self.push(Channel::Text, text),
            (OutputFormat::Cot, StreamEvent::Text { text }) => self.push(Channel::Text, text),
            (OutputFormat::Cot, StreamEvent::Reasoning { text }) => {
                self.push(Channel::Reasoning, text)
            }
            (_, StreamEvent::Usage(_) | StreamEvent::Done) => {
                self.flush_line(true)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn push(&mut self, channel: Channel, text: &str) -> io::Result<()> {
        if channel != self.channel && !self.line.is_empty() {
            self.flush_line(true)?;
        }
        self.channel = channel;
        for ch in text.chars() {
            if ch == '\n' {
                self.flush_line(true)?;
            } else {
                self.line.push(ch);
                self.flush_line(false)?;
            }
        }
        if self.tty {
            self.out.flush()?;
        }
        Ok(())
    }

    /// Write the current line. With `newline` the line is committed; otherwise
    /// the terminal's current line is rewritten in place.
    fn flush_line(&mut self, newline: bool) -> io::Result<()> {
        if self.line.is_empty() && newline {
            return Ok(());
        }
        let elapsed = self.line_started.elapsed().as_secs_f64();
        let rendered = self.style(&self.line, self.channel);
        if self.tty {
            write!(self.out, "\r\x1b[2K")?;
        }
        write!(self.out, "[+{elapsed:6.3}s] {rendered}")?;
        if newline {
            writeln!(self.out)?;
            self.line.clear();
            self.line_started = Instant::now();
        }
        Ok(())
    }

    fn style(&self, text: &str, channel: Channel) -> String {
        if !self.color {
            return text.to_owned();
        }
        match channel {
            Channel::Reasoning => format!("\x1b[2m{text}\x1b[0m"),
            Channel::Text => text.to_owned(),
        }
    }

    fn render_findings(&mut self, response: &Response, markdown: bool) -> io::Result<()> {
        if response.findings.is_empty() {
            return writeln!(self.out, "No findings.");
        }
        for finding in &response.findings {
            let severity = finding.severity.as_str();
            if markdown {
                writeln!(self.out, "## {} ({})", finding.title, finding.cwe)?;
                writeln!(self.out, "- severity: {severity}")?;
                writeln!(self.out, "- location: `{}:{}`", finding.path, finding.line)?;
                writeln!(self.out, "\n> {}\n", finding.evidence)?;
            } else {
                writeln!(
                    self.out,
                    "[{severity}] {}:{}  {}  {}",
                    finding.path, finding.line, finding.cwe, finding.title
                )?;
                writeln!(self.out, "    {}", finding.evidence)?;
            }
        }
        Ok(())
    }
}

impl<W: Write> EventSink for Renderer<W> {
    fn on_event(&mut self, event: &StreamEvent) {
        let _ = self.write_event(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Finding, Severity, VERSION};

    fn render(format: OutputFormat, color: bool, events: &[StreamEvent]) -> String {
        let mut buffer = Vec::new();
        let mut renderer = Renderer::new(format, color, false, &mut buffer);
        for event in events {
            renderer.on_event(event);
        }
        String::from_utf8(buffer).unwrap()
    }

    fn finding() -> Finding {
        Finding {
            cwe: "CWE-78".into(),
            path: "sources/x.py".into(),
            line: 4,
            severity: Severity::High,
            title: "Command injection".into(),
            evidence: "name reaches shell".into(),
        }
    }

    #[test]
    fn body_emits_one_timed_line_per_complete_line() {
        let output = render(
            OutputFormat::Body,
            false,
            &[
                StreamEvent::Text {
                    text: "first\nsecond".into(),
                },
                StreamEvent::Done,
            ],
        );
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("[+"));
        assert!(lines[0].ends_with("first"));
        assert!(lines[1].ends_with("second"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn cot_marks_reasoning_dim_only_with_color() {
        let events = [
            StreamEvent::Reasoning {
                text: "why\n".into(),
            },
            StreamEvent::Text {
                text: "answer\n".into(),
            },
        ];
        let plain = render(OutputFormat::Cot, false, &events);
        assert!(plain.contains("why"));
        assert!(plain.contains("answer"));
        assert!(!plain.contains('\u{1b}'));
        let colored = render(OutputFormat::Cot, true, &events);
        assert!(colored.contains("\x1b[2m"));
    }

    #[test]
    fn jsonl_writes_typed_events() {
        let output = render(
            OutputFormat::Jsonl,
            false,
            &[StreamEvent::Text { text: "hi".into() }, StreamEvent::Done],
        );
        let values: Vec<serde_json::Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(values[0]["type"], "text");
        assert_eq!(values[1]["type"], "done");
    }

    #[test]
    fn text_and_markdown_render_findings_on_finish() {
        let response = Response {
            schema_version: VERSION,
            findings: vec![finding()],
        };
        let mut plain = Vec::new();
        Renderer::new(OutputFormat::Text, false, false, &mut plain)
            .finish(&response)
            .unwrap();
        let plain = String::from_utf8(plain).unwrap();
        assert!(plain.contains("CWE-78"));
        assert!(plain.contains("sources/x.py:4"));

        let mut markdown = Vec::new();
        Renderer::new(OutputFormat::Markdown, false, false, &mut markdown)
            .finish(&response)
            .unwrap();
        let markdown = String::from_utf8(markdown).unwrap();
        assert!(markdown.contains("## Command injection"));
        assert!(markdown.contains("> name reaches shell"));
    }

    #[test]
    fn quiet_and_json_produce_nothing() {
        let events = [StreamEvent::Text {
            text: "hi\n".into(),
        }];
        assert!(render(OutputFormat::Quiet, false, &events).is_empty());
        assert!(render(OutputFormat::Json, false, &events).is_empty());
    }
}
