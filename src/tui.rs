//! Interactive terminal UI for a single audit.
//!
//! The TUI reads normalized [`StreamEvent`]s from the provider task over a
//! channel and renders the target, reasoning, answer, findings, usage, and
//! status. All terminal I/O is isolated in [`run_audit`]; [`App`] and
//! [`render`] contain no terminal state, so they are validated headlessly with
//! `ratatui`'s `TestBackend`.

use crate::{
    context::Context,
    protocol::{Request, Response},
    provider::{
        self,
        events::{EventSink, StreamEvent, Usage},
        LlmOptions,
    },
};
use anyhow::{Context as _, Result};
use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    crossterm::{
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Constraint, Layout},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    io,
    sync::mpsc::{self, Sender},
    time::{Duration, Instant},
};

/// TUI state. Pure with respect to the terminal.
pub struct App {
    pub target: String,
    pub model: String,
    pub status: String,
    pub reasoning: Vec<String>,
    pub output: Vec<String>,
    pub findings: Vec<String>,
    pub usage: Usage,
    pub elapsed_ms: u64,
    pub done: bool,
    pub failed: bool,
    started: Instant,
    partial_reasoning: String,
    partial_output: String,
}

impl App {
    pub fn new(target: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            model: model.into(),
            status: "running".into(),
            reasoning: Vec::new(),
            output: Vec::new(),
            findings: Vec::new(),
            usage: Usage::default(),
            elapsed_ms: 0,
            done: false,
            failed: false,
            started: Instant::now(),
            partial_reasoning: String::new(),
            partial_output: String::new(),
        }
    }

    fn push(lines: &mut Vec<String>, partial: &mut String, chunk: &str) {
        for ch in chunk.chars() {
            if ch == '\n' {
                lines.push(std::mem::take(partial));
            } else {
                partial.push(ch);
            }
        }
    }

    /// Apply one streaming event.
    pub fn handle_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::Reasoning { text } => {
                Self::push(&mut self.reasoning, &mut self.partial_reasoning, text);
            }
            StreamEvent::Text { text } => {
                Self::push(&mut self.output, &mut self.partial_output, text);
            }
            StreamEvent::Usage(usage) => self.usage = *usage,
            StreamEvent::Done => {
                if !self.partial_reasoning.is_empty() {
                    self.reasoning
                        .push(std::mem::take(&mut self.partial_reasoning));
                }
                if !self.partial_output.is_empty() {
                    self.output.push(std::mem::take(&mut self.partial_output));
                }
                self.done = true;
            }
        }
        if self.done {
            self.elapsed_ms = self.started.elapsed().as_millis() as u64;
            self.status = if self.findings.is_empty() {
                "complete".into()
            } else {
                format!("complete · {} findings", self.findings.len())
            };
        }
    }

    /// Populate the findings pane from the final response.
    pub fn on_finish(&mut self, response: &Response) {
        self.findings = response
            .findings
            .iter()
            .map(|finding| {
                format!(
                    "[{}] {}:{} {} — {}",
                    finding.severity.as_str(),
                    finding.path,
                    finding.line,
                    finding.cwe,
                    finding.title
                )
            })
            .collect();
        self.done = true;
        self.elapsed_ms = self.started.elapsed().as_millis() as u64;
        self.status = if self.findings.is_empty() {
            "complete · no findings".into()
        } else {
            format!("complete · {} findings", self.findings.len())
        };
    }

    fn footer(&self) -> String {
        let mut footer = format!(
            "{} · {} ms · {} finding(s)",
            self.status,
            self.elapsed_ms,
            self.findings.len()
        );
        if !self.usage.is_empty() {
            footer.push_str(&format!(
                " · tokens {}/{}/{}",
                self.usage.prompt_tokens, self.usage.completion_tokens, self.usage.total_tokens
            ));
        }
        footer
    }
}

impl EventSink for App {
    fn on_event(&mut self, event: &StreamEvent) {
        self.handle_event(event);
    }
}

/// Render one frame. Split out so it can be validated with `TestBackend`.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Min(4),
        Constraint::Length(3),
    ])
    .split(frame.area());

    let header = Paragraph::new(format!(
        "target: {}   ·   model: {}   ·   {}",
        app.target, app.model, app.status
    ))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Audit Alchemist"),
    );
    frame.render_widget(header, chunks[0]);

    let reasoning = Paragraph::new(app.reasoning.join("\n"))
        .block(Block::default().borders(Borders::ALL).title("Reasoning"))
        .wrap(Wrap { trim: false });
    frame.render_widget(reasoning, chunks[1]);

    let output = Paragraph::new(app.output.join("\n"))
        .block(Block::default().borders(Borders::ALL).title("Answer"))
        .wrap(Wrap { trim: false });
    frame.render_widget(output, chunks[2]);

    let findings = Paragraph::new(app.findings.join("\n"))
        .block(Block::default().borders(Borders::ALL).title("Findings"))
        .wrap(Wrap { trim: false });
    frame.render_widget(findings, chunks[3]);

    let footer = Paragraph::new(app.footer()).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Press q to quit"),
    );
    frame.render_widget(footer, chunks[4]);
}

/// Collect a buffer into a plain string, useful for headless validation.
pub fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

struct ChannelSink {
    tx: Sender<StreamEvent>,
}

impl EventSink for ChannelSink {
    fn on_event(&mut self, event: &StreamEvent) {
        let _ = self.tx.send(event.clone());
    }
}

/// Run an audit inside the TUI and return the final response.
pub async fn run_audit(
    mut app: App,
    options: LlmOptions,
    request: Request,
    context: Context,
    timeout: Duration,
) -> Result<Response> {
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let mut sink = ChannelSink { tx };
    // Poll the provider future and the UI loop together so the sink does not
    // need to be `Send`.
    let mut future = std::pin::pin!(provider::audit_with(
        &options, &request, &context, timeout, &mut sink
    ));

    let mut terminal = setup()?;
    let mut outcome: Option<Result<Response>> = None;
    let mut cancelled = false;
    loop {
        while let Ok(event) = rx.try_recv() {
            app.handle_event(&event);
        }
        terminal.draw(|frame| render(frame, &app))?;
        if outcome.is_some() || app.done {
            break;
        }
        tokio::select! {
            result = &mut future => outcome = Some(result),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if event::poll(Duration::ZERO).unwrap_or(false) {
                    if let Ok(Event::Key(key)) = event::read() {
                        if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                        {
                            cancelled = true;
                            break;
                        }
                    }
                }
            }
        }
    }

    if cancelled {
        restore(&mut terminal)?;
        anyhow::bail!("audit cancelled");
    }

    let result = match outcome {
        Some(result) => result,
        None => (&mut future).await,
    };
    if let Ok(response) = &result {
        app.on_finish(response);
    }
    let _ = terminal.draw(|frame| render(frame, &app));
    wait_for_exit(Duration::from_secs(3));
    restore(&mut terminal)?;
    result
}

fn setup() -> Result<Terminal<CrosstermBackend<io::Stderr>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stderr);
    Terminal::new(backend).context("create terminal")
}

fn restore(terminal: &mut Terminal<CrosstermBackend<io::Stderr>>) -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leave alternate screen")?;
    terminal.show_cursor().context("show cursor")?;
    Ok(())
}

/// Wait until the user presses a key or the timeout elapses.
fn wait_for_exit(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => {
                let _ = event::read();
                return;
            }
            Ok(false) => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Finding, Severity, VERSION};
    use ratatui::backend::TestBackend;

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
    fn renders_target_reasoning_output_and_findings() {
        let mut app = App::new("sources/x.py", "model-x");
        app.handle_event(&StreamEvent::Reasoning {
            text: "because input\n".into(),
        });
        app.handle_event(&StreamEvent::Text {
            text: "the answer\n".into(),
        });
        app.on_finish(&Response {
            schema_version: VERSION,
            findings: vec![finding()],
        });

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = buffer_to_string(terminal.backend().buffer());

        assert!(content.contains("sources/x.py"));
        assert!(content.contains("model-x"));
        assert!(content.contains("because input"));
        assert!(content.contains("the answer"));
        assert!(content.contains("CWE-78"));
        assert!(content.contains("Command injection"));
    }

    #[test]
    fn reports_no_findings_status() {
        let mut app = App::new("safe.py", "model");
        app.on_finish(&Response {
            schema_version: VERSION,
            findings: vec![],
        });
        assert!(app.footer().contains("no findings"));
    }
}
