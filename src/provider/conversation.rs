//! Native tool calls and replayable conversation history for each provider.
use super::{
    events::{EventSink, Usage},
    wire::{self, WireApi, WireStream},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub struct Turn {
    pub text: String,
    pub usage: Usage,
    pub calls: Vec<ToolCall>,
    assistant: Vec<Value>,
}

pub struct Conversation {
    pub body: Value,
    wire: WireApi,
}

impl Conversation {
    pub fn new(wire: WireApi, model: &str, system: &str, user: &str, max_tokens: u32) -> Self {
        Self::with_tools(
            wire,
            model,
            system,
            user,
            max_tokens,
            crate::tools::definitions(),
        )
    }

    pub fn with_tools(
        wire: WireApi,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
        tools: Vec<Value>,
    ) -> Self {
        let mut body = wire::request_body(wire, model, system, user, max_tokens);
        body["tools"] = Value::Array(tools.into_iter().map(|tool| match wire {
            WireApi::ChatCompletions => json!({"type":"function","function":tool}),
            WireApi::Responses => json!({"type":"function","name":tool["name"],"description":tool["description"],"parameters":tool["parameters"],"strict":false}),
            WireApi::Anthropic => json!({"name":tool["name"],"description":tool["description"],"input_schema":tool["parameters"]}),
        }).collect());
        if wire == WireApi::Responses {
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        Self { body, wire }
    }

    pub fn append(&mut self, turn: &Turn, results: &[String]) -> Result<()> {
        ensure!(
            turn.calls.len() == results.len(),
            "tool results do not match calls"
        );
        let key = if self.wire == WireApi::Responses {
            "input"
        } else {
            "messages"
        };
        let history = self.body[key]
            .as_array_mut()
            .context("missing conversation history")?;
        history.extend(turn.assistant.iter().cloned());
        let mut anthropic = Vec::new();
        for (call, result) in turn.calls.iter().zip(results) {
            match self.wire {
                WireApi::ChatCompletions => {
                    history.push(json!({"role":"tool","tool_call_id":call.id,"content":result}))
                }
                WireApi::Responses => history
                    .push(json!({"type":"function_call_output","call_id":call.id,"output":result})),
                WireApi::Anthropic => anthropic
                    .push(json!({"type":"tool_result","tool_use_id":call.id,"content":result})),
            }
        }
        if !anthropic.is_empty() {
            history.push(json!({"role":"user","content":anthropic}));
        }
        Ok(())
    }
}

pub struct TurnDecoder {
    wire: WireApi,
    stream: WireStream,
    chat_calls: BTreeMap<usize, ToolCall>,
    blocks: BTreeMap<usize, Value>,
    partial_json: BTreeMap<usize, String>,
    output: Vec<Value>,
}

impl TurnDecoder {
    pub fn new(wire: WireApi) -> Self {
        Self {
            wire,
            stream: WireStream::new(wire),
            chat_calls: BTreeMap::new(),
            blocks: BTreeMap::new(),
            partial_json: BTreeMap::new(),
            output: vec![],
        }
    }

    pub fn handle_data(&mut self, data: &str, sink: &mut dyn EventSink) -> Result<()> {
        self.stream.handle_data(data, sink)?;
        if data == "[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_str(data)?;
        match self.wire {
            WireApi::ChatCompletions => {
                if let Some(calls) = event
                    .pointer("/choices/0/delta/tool_calls")
                    .and_then(Value::as_array)
                {
                    for call in calls {
                        let index =
                            call["index"].as_u64().context("tool call missing index")? as usize;
                        ensure!(index < 64, "too many tool calls in one turn");
                        let accumulated = self.chat_calls.entry(index).or_default();
                        if let Some(id) = call["id"].as_str() {
                            accumulated.id.push_str(id);
                        }
                        if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                            accumulated.name.push_str(name);
                        }
                        if let Some(args) =
                            call.pointer("/function/arguments").and_then(Value::as_str)
                        {
                            accumulated.arguments.push_str(args);
                        }
                    }
                }
            }
            WireApi::Responses => match event["type"].as_str().unwrap_or("") {
                "response.output_item.done" => self.output.push(event["item"].clone()),
                "response.completed" => {
                    if let Some(output) =
                        event.pointer("/response/output").and_then(Value::as_array)
                    {
                        if !output.is_empty() {
                            self.output = output.clone();
                        }
                    }
                }
                _ => {}
            },
            WireApi::Anthropic => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                match event["type"].as_str().unwrap_or("") {
                    "content_block_start" => {
                        self.blocks.insert(index, event["content_block"].clone());
                    }
                    "content_block_delta" => {
                        let delta = &event["delta"];
                        if delta["type"] == "input_json_delta" {
                            self.partial_json
                                .entry(index)
                                .or_default()
                                .push_str(delta["partial_json"].as_str().unwrap_or(""));
                        } else {
                            let key = match delta["type"].as_str().unwrap_or("") {
                                "text_delta" => "text",
                                "thinking_delta" => "thinking",
                                "signature_delta" => "signature",
                                _ => "",
                            };
                            if !key.is_empty() {
                                if let Some(block) = self.blocks.get_mut(&index) {
                                    let combined = format!(
                                        "{}{}",
                                        block[key].as_str().unwrap_or(""),
                                        delta[key].as_str().unwrap_or("")
                                    );
                                    block[key] = json!(combined);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Turn> {
        ensure!(
            self.stream.error.is_none(),
            "model response failed: {}",
            self.stream.error.as_deref().unwrap_or("")
        );
        ensure!(self.stream.completed, "stream ended before completion");
        if self.wire == WireApi::Responses {
            let final_messages: Vec<_> = self
                .output
                .iter()
                .filter(|item| item["type"] == "message" && item["phase"] == "final_answer")
                .collect();
            if !final_messages.is_empty() {
                self.stream.text = final_messages
                    .iter()
                    .filter_map(|item| item["content"].as_array())
                    .flatten()
                    .filter_map(|part| part["text"].as_str())
                    .collect();
            }
        }
        let mut calls = Vec::new();
        let assistant = match self.wire {
            WireApi::ChatCompletions => {
                calls.extend(self.chat_calls.into_values());
                let mut message = json!({"role":"assistant","content":self.stream.text});
                if !self.stream.reasoning.is_empty() {
                    message["reasoning_content"] = json!(self.stream.reasoning);
                }
                if !calls.is_empty() {
                    message["tool_calls"] = json!(calls.iter().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>());
                }
                vec![message]
            }
            WireApi::Responses => {
                for item in &self.output {
                    if item["type"] == "function_call" {
                        calls.push(ToolCall {
                            id: item["call_id"].as_str().unwrap_or("").into(),
                            name: item["name"].as_str().unwrap_or("").into(),
                            arguments: item["arguments"].as_str().unwrap_or("").into(),
                        });
                    }
                }
                // Retain reasoning items and phase metadata exactly as received.
                self.output
            }
            WireApi::Anthropic => {
                for (index, args) in self.partial_json {
                    self.blocks
                        .get_mut(&index)
                        .context("tool input without a content block")?["input"] =
                        serde_json::from_str(&args).context("invalid streamed tool input")?;
                }
                let blocks: Vec<_> = self.blocks.into_values().collect();
                for block in &blocks {
                    if block["type"] == "tool_use" {
                        calls.push(ToolCall {
                            id: block["id"].as_str().unwrap_or("").into(),
                            name: block["name"].as_str().unwrap_or("").into(),
                            arguments: serde_json::to_string(&block["input"])?,
                        });
                    }
                }
                vec![json!({"role":"assistant","content":blocks})]
            }
        };
        ensure!(calls.len() <= 64, "too many tool calls in one turn");
        let mut ids = BTreeSet::new();
        for call in &calls {
            ensure!(
                !call.id.is_empty() && !call.name.is_empty() && ids.insert(&call.id),
                "invalid or duplicate tool call id/name"
            );
        }
        ensure!(
            !self.stream.text.trim().is_empty() || !calls.is_empty(),
            "model response contained no output text or tool calls"
        );
        Ok(Turn {
            text: self.stream.text,
            usage: self.stream.usage,
            calls,
            assistant,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_use_final_phase_text_without_losing_commentary_history() {
        let mut decoder = TurnDecoder::new(WireApi::Responses);
        let answer = r#"{"schema_version":1,"findings":[]}"#;
        decoder.handle_data(&json!({"type":"response.completed","response":{"output":[
            {"type":"message","phase":"commentary","content":[{"type":"output_text","text":"Checking the source."}]},
            {"type":"message","phase":"final_answer","content":[{"type":"output_text","text":answer}]}
        ]}}).to_string(), &mut ()).unwrap();
        let turn = decoder.finish().unwrap();
        assert_eq!(turn.text, answer);
        assert_eq!(turn.assistant[0]["phase"], "commentary");
        assert_eq!(turn.assistant[1]["phase"], "final_answer");
    }

    #[test]
    fn duplicate_call_ids_and_truncated_anthropic_turns_fail() {
        let mut decoder = TurnDecoder::new(WireApi::Responses);
        let item = json!({"type":"function_call","call_id":"same","name":"bash","arguments":"{}"});
        decoder
            .handle_data(
                &json!({"type":"response.completed","response":{"output":[item.clone(),item]}})
                    .to_string(),
                &mut (),
            )
            .unwrap();
        assert!(decoder.finish().is_err());

        let mut decoder = TurnDecoder::new(WireApi::Anthropic);
        for event in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"one","name":"bash","input":{}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}),
            json!({"type":"message_stop"}),
        ] {
            decoder.handle_data(&event.to_string(), &mut ()).unwrap();
        }
        assert!(decoder.finish().is_err());
    }
}
