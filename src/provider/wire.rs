//! Wire formats and their stream decoders.
//!
//! Supports the three mainstream shapes:
//!
//! - `chat-completions`: OpenAI-compatible (`/v1/chat/completions`), which also
//!   covers DeepSeek, Groq, Azure, and local servers.
//! - `responses`: the OpenAI Responses API (also used by the Codex backend).
//! - `anthropic`: the Anthropic Messages API (`/v1/messages`).
//!
//! Each decoder normalizes deltas into [`StreamEvent`]s and reports usage when
//! the provider does.

use super::events::{EventSink, StreamEvent, Usage};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

/// The wire format used for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum WireApi {
    /// OpenAI-compatible chat completions.
    ChatCompletions,
    /// OpenAI Responses API.
    Responses,
    /// Anthropic Messages API.
    Anthropic,
}

impl WireApi {
    /// Stable CLI spelling, matching `clap`'s kebab-case value.
    pub fn as_str(self) -> &'static str {
        match self {
            WireApi::ChatCompletions => "chat-completions",
            WireApi::Responses => "responses",
            WireApi::Anthropic => "anthropic",
        }
    }
}

/// Build the request body for a wire format.
pub fn request_body(
    wire: WireApi,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Value {
    match wire {
        WireApi::ChatCompletions => serde_json::json!({
            "model": model,
            "temperature": 0,
            "max_tokens": max_tokens,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        }),
        WireApi::Responses => serde_json::json!({
            "model": model,
            "instructions": system,
            "input": [{"role": "user", "content": [{"type": "input_text", "text": user}]}],
            "max_output_tokens": max_tokens,
            "stream": true,
            "store": false
        }),
        WireApi::Anthropic => serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "stream": true,
            "messages": [{"role": "user", "content": user}]
        }),
    }
}

/// Accumulates one stream, emitting normalized events.
#[derive(Debug)]
pub struct WireStream {
    wire: WireApi,
    pub text: String,
    pub reasoning: String,
    pub usage: Usage,
    pub completed: bool,
    pub error: Option<String>,
}

impl WireStream {
    pub fn new(wire: WireApi) -> Self {
        Self {
            wire,
            text: String::new(),
            reasoning: String::new(),
            usage: Usage::default(),
            completed: false,
            error: None,
        }
    }

    /// Decode one `data:` payload.
    pub fn handle_data(&mut self, data: &str, sink: &mut dyn EventSink) -> Result<()> {
        if data == "[DONE]" {
            self.completed = true;
            return Ok(());
        }
        let event: Value = serde_json::from_str(data).context("invalid stream event")?;
        match self.wire {
            WireApi::ChatCompletions => self.handle_chat(&event, sink),
            WireApi::Responses => self.handle_responses(&event, sink),
            WireApi::Anthropic => self.handle_anthropic(&event, sink),
        }
        Ok(())
    }

    /// Return the accumulated answer after checking the stream completed.
    pub fn finish(self) -> Result<(String, Usage)> {
        if let Some(error) = self.error {
            bail!("model response failed: {error}");
        }
        ensure!(self.completed, "stream ended before completion");
        ensure!(
            !self.text.trim().is_empty(),
            "model response contained no output text"
        );
        Ok((self.text, self.usage))
    }

    fn handle_chat(&mut self, event: &Value, sink: &mut dyn EventSink) {
        if is_present(event.get("error")) {
            self.error = Some(json_error(event.get("error").unwrap()));
            return;
        }
        if is_present(event.get("usage")) {
            self.set_usage(openai_usage(event.get("usage").unwrap()), sink);
        }
        let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        if let Some(delta) = choice.get("delta") {
            let reasoning = delta
                .get("reasoning_content")
                .and_then(Value::as_str)
                .or_else(|| delta.get("reasoning").and_then(Value::as_str));
            if let Some(reasoning) = reasoning {
                self.emit_reasoning(reasoning, sink);
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                self.emit_text(content, sink);
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            // Tool calls continue the agent loop; truncation/filtering must fail.
            if reason != "stop" && reason != "tool_calls" {
                self.error = Some(format!("model stopped with finish_reason {reason}"));
            }
        }
    }

    fn handle_responses(&mut self, event: &Value, sink: &mut dyn EventSink) {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.emit_text(delta, sink);
                }
            }
            "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.emit_reasoning(delta, sink);
                }
            }
            "response.completed" => {
                self.completed = true;
                if self.text.is_empty() {
                    let text = completed_text(event);
                    if !text.is_empty() {
                        self.emit_text(&text, sink);
                    }
                }
                if let Some(usage) = event.pointer("/response/usage") {
                    self.set_usage(responses_usage(usage), sink);
                }
            }
            "response.failed" | "response.incomplete" => {
                let error = event.pointer("/response/error").unwrap_or(event);
                self.error = Some(json_error(error));
            }
            "error" => self.error = Some(json_error(event)),
            _ => {}
        }
    }

    fn handle_anthropic(&mut self, event: &Value, sink: &mut dyn EventSink) {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.set_usage(anthropic_usage(usage), sink);
                }
            }
            "content_block_delta" => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            self.emit_text(text, sink);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                            self.emit_reasoning(thinking, sink);
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    if reason != "end_turn" && reason != "tool_use" {
                        self.error = Some(format!("model stopped with stop_reason {reason}"));
                    }
                }
                if let Some(usage) = event.get("usage") {
                    self.set_usage(anthropic_usage(usage), sink);
                }
            }
            "message_stop" => self.completed = true,
            "error" => {
                let error = event.get("error").unwrap_or(event);
                self.error = Some(json_error(error));
            }
            _ => {}
        }
    }

    fn emit_text(&mut self, text: &str, sink: &mut dyn EventSink) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        sink.on_event(&StreamEvent::Text {
            text: text.to_owned(),
        });
    }

    fn emit_reasoning(&mut self, text: &str, sink: &mut dyn EventSink) {
        if text.is_empty() {
            return;
        }
        self.reasoning.push_str(text);
        sink.on_event(&StreamEvent::Reasoning {
            text: text.to_owned(),
        });
    }

    fn set_usage(&mut self, usage: Usage, sink: &mut dyn EventSink) {
        if usage.is_empty() {
            return;
        }
        self.usage.merge(usage);
        sink.on_event(&StreamEvent::Usage(self.usage));
    }
}

fn is_present(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !value.is_null())
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn openai_usage(usage: &Value) -> Usage {
    Usage {
        prompt_tokens: number(usage, "prompt_tokens"),
        completion_tokens: number(usage, "completion_tokens"),
        total_tokens: number(usage, "total_tokens"),
        reasoning_tokens: usage
            .pointer("/completion_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn responses_usage(usage: &Value) -> Usage {
    Usage {
        prompt_tokens: number(usage, "input_tokens"),
        completion_tokens: number(usage, "output_tokens"),
        total_tokens: number(usage, "total_tokens"),
        reasoning_tokens: usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn anthropic_usage(usage: &Value) -> Usage {
    Usage {
        prompt_tokens: number(usage, "input_tokens"),
        completion_tokens: number(usage, "output_tokens"),
        total_tokens: 0,
        reasoning_tokens: 0,
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

fn json_error(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| value.as_str())
        .unwrap_or("unknown provider error")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Capture(Vec<StreamEvent>);

    impl EventSink for Capture {
        fn on_event(&mut self, event: &StreamEvent) {
            self.0.push(event.clone());
        }
    }

    #[test]
    fn chat_completions_streams_text_reasoning_and_usage() {
        let mut stream = WireStream::new(WireApi::ChatCompletions);
        let mut sink = Capture::default();
        stream
            .handle_data(
                r#"{"choices":[{"delta":{"reasoning_content":"think "},"finish_reason":null}]}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(r#"{"choices":[{"delta":{"content":"hel"}}]}"#, &mut sink)
            .unwrap();
        stream
            .handle_data(r#"{"choices":[{"delta":{"content":"lo"}}]}"#, &mut sink)
            .unwrap();
        stream
            .handle_data(
                r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":2,"total_tokens":9}}"#,
                &mut sink,
            )
            .unwrap();
        stream.handle_data("[DONE]", &mut sink).unwrap();
        let (text, usage) = stream.finish().unwrap();
        assert_eq!(text, "hello");
        assert_eq!(usage.total_tokens, 9);
        assert!(matches!(
            sink.0.first(),
            Some(StreamEvent::Reasoning { .. })
        ));
        assert!(matches!(sink.0.last(), Some(StreamEvent::Usage(_))));
    }

    #[test]
    fn responses_streams_deltas_and_completed_usage() {
        let mut stream = WireStream::new(WireApi::Responses);
        let mut sink = Capture::default();
        stream
            .handle_data(
                r#"{"type":"response.reasoning_summary_text.delta","delta":"why"}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(
                r#"{"type":"response.output_text.delta","delta":"answer"}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(
                r#"{"type":"response.completed","response":{"output":[],"usage":{"input_tokens":3,"output_tokens":5,"total_tokens":8}}}"#,
                &mut sink,
            )
            .unwrap();
        let (text, usage) = stream.finish().unwrap();
        assert_eq!(text, "answer");
        assert_eq!(usage.prompt_tokens, 3);
        assert_eq!(usage.completion_tokens, 5);
    }

    #[test]
    fn anthropic_streams_thinking_and_text() {
        let mut stream = WireStream::new(WireApi::Anthropic);
        let mut sink = Capture::default();
        stream
            .handle_data(
                r#"{"type":"message_start","message":{"usage":{"input_tokens":11}}}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(
                r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(
                r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"done"}}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":6}}"#,
                &mut sink,
            )
            .unwrap();
        stream
            .handle_data(r#"{"type":"message_stop"}"#, &mut sink)
            .unwrap();
        let (text, usage) = stream.finish().unwrap();
        assert_eq!(text, "done");
        assert_eq!(usage.prompt_tokens, 11);
        assert_eq!(usage.completion_tokens, 6);
    }

    #[test]
    fn finish_rejects_incomplete_and_failed_streams() {
        let mut stream = WireStream::new(WireApi::ChatCompletions);
        let mut sink = Capture::default();
        stream
            .handle_data(
                r#"{"choices":[{"delta":{"content":"partial"}}]}"#,
                &mut sink,
            )
            .unwrap();
        assert!(stream.finish().is_err());

        let mut failed = WireStream::new(WireApi::Anthropic);
        failed
            .handle_data(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
                &mut sink,
            )
            .unwrap();
        assert!(failed.finish().unwrap_err().to_string().contains("busy"));
    }

    #[test]
    fn finish_rejects_truncated_completions() {
        let mut stream = WireStream::new(WireApi::ChatCompletions);
        let mut sink = Capture::default();
        stream
            .handle_data(
                r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":"length"}]}"#,
                &mut sink,
            )
            .unwrap();
        stream.handle_data("[DONE]", &mut sink).unwrap();
        assert!(stream
            .finish()
            .unwrap_err()
            .to_string()
            .contains("finish_reason length"));
    }

    #[test]
    fn request_bodies_use_the_expected_top_level_fields() {
        let chat = request_body(WireApi::ChatCompletions, "m", "sys", "usr", 128);
        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["stream"], true);
        let responses = request_body(WireApi::Responses, "m", "sys", "usr", 128);
        assert_eq!(responses["instructions"], "sys");
        assert_eq!(responses["max_output_tokens"], 128);
        let anthropic = request_body(WireApi::Anthropic, "m", "sys", "usr", 128);
        assert_eq!(anthropic["system"], "sys");
        assert_eq!(anthropic["messages"][0]["role"], "user");
    }
}
