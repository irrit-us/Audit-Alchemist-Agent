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

/// The provider completed a stream without text or tool calls. Treated as a
/// transient response defect so the caller can retry the unchanged request.
#[derive(Debug)]
pub struct EmptyCompletion;

impl std::fmt::Display for EmptyCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("model response contained no output text or tool calls")
    }
}

impl std::error::Error for EmptyCompletion {}

pub struct Conversation {
    pub body: Value,
    wire: WireApi,
    result_slots: Vec<ResultSlot>,
    tool_turns: u32,
}

struct ResultSlot {
    pointer: String,
    turn: u32,
    protected: bool,
    pruned: bool,
}

#[derive(serde::Serialize)]
pub struct ContextReduction {
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub pruned_results: usize,
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
        Self {
            body,
            wire,
            result_slots: Vec::new(),
            tool_turns: 0,
        }
    }

    /// Apply a provider reasoning-effort hint. Only chat-completions accepts
    /// it; the Responses and Anthropic wires carry effort differently.
    pub fn set_reasoning_effort(&mut self, effort: &str) {
        if self.wire == WireApi::ChatCompletions {
            self.body["reasoning_effort"] = json!(effort);
        }
    }

    /// Keep only the named tools in the request schema. Used to withdraw
    /// mutating tools during settlement while leaving read-only verification.
    pub fn retain_tools(&mut self, names: &[&str]) {
        let Some(tools) = self.body.get_mut("tools").and_then(Value::as_array_mut) else {
            return;
        };
        tools.retain(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| tool.pointer("/function/name").and_then(Value::as_str));
            name.is_some_and(|name| names.contains(&name))
        });
    }

    /// Stop offering tools so a budget-exhausted run must return its report.
    /// The system prompt already tells the model to report when
    /// `remaining_tool_calls` is zero; this makes the contract enforceable
    /// instead of failing the run when the model still asks for a tool.
    pub fn disable_tools(&mut self) {
        if let Some(body) = self.body.as_object_mut() {
            body.remove("tools");
        }
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
        self.tool_turns += 1;
        let mut anthropic = Vec::new();
        for (call, result) in turn.calls.iter().zip(results) {
            let pointer = match self.wire {
                WireApi::ChatCompletions => format!("/{key}/{}/content", history.len()),
                WireApi::Responses => format!("/{key}/{}/output", history.len()),
                WireApi::Anthropic => format!(
                    "/{key}/{}/content/{}/content",
                    history.len(),
                    anthropic.len()
                ),
            };
            self.result_slots.push(ResultSlot {
                pointer,
                turn: self.tool_turns,
                protected: call.name == "load_skill",
                pruned: false,
            });
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

    /// Append a rejected final answer and a corrective user message. Used only
    /// for bounded validation feedback, never for tool results, so the model
    /// can re-emit without losing the rejected claim.
    pub fn append_feedback(&mut self, turn: &Turn, message: &str) -> Result<()> {
        let key = if self.wire == WireApi::Responses {
            "input"
        } else {
            "messages"
        };
        let history = self
            .body
            .get_mut(key)
            .and_then(Value::as_array_mut)
            .context("missing conversation history")?;
        history.extend(turn.assistant.iter().cloned());
        match self.wire {
            WireApi::ChatCompletions => history.push(json!({"role":"user","content":message})),
            WireApi::Responses => history
                .push(json!({"role":"user","content":[{"type":"input_text","text":message}]})),
            WireApi::Anthropic => history.push(json!({"role":"user","content":message})),
        }
        Ok(())
    }

    /// Change only old result bodies. Calls, IDs, reasoning and initial context
    /// remain byte-for-byte intact; recent tool turns and loaded skills survive.
    pub fn reduce_context(
        &mut self,
        max_bytes: usize,
        keep_turns: u32,
        archive: &mut crate::context::history::ResultArchive,
    ) -> Result<ContextReduction> {
        use crate::context::history::{project_result, serialized_bytes};
        let before_bytes = serialized_bytes(&self.body)?;
        let mut report = ContextReduction {
            before_bytes,
            after_bytes: before_bytes,
            pruned_results: 0,
        };
        if before_bytes <= max_bytes * 9 / 10 {
            return Ok(report);
        }
        let cutoff = self.tool_turns.saturating_sub(keep_turns.max(1));
        for slot in &mut self.result_slots {
            if report.after_bytes <= max_bytes * 8 / 10 {
                break;
            }
            if slot.protected || slot.pruned || slot.turn > cutoff {
                continue;
            }
            let original = self
                .body
                .pointer(&slot.pointer)
                .and_then(Value::as_str)
                .context("missing paired tool result")?;
            if original.len() <= 2048 {
                continue;
            }
            let saved = archive.save(original)?;
            let Some(path) = saved else {
                continue;
            };
            let replacement = project_result(original, 1024, Some(&path))?;
            let old_bytes = serialized_bytes(&original)?;
            let new_bytes = serialized_bytes(&replacement)?;
            if new_bytes >= old_bytes {
                continue;
            }
            *self
                .body
                .pointer_mut(&slot.pointer)
                .context("missing tool result")? = json!(replacement);
            report.after_bytes = report.after_bytes - old_bytes + new_bytes;
            report.pruned_results += 1;
            slot.pruned = true;
        }
        Ok(report)
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
        if self.stream.text.trim().is_empty() && calls.is_empty() {
            return Err(EmptyCompletion.into());
        }
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
    fn reasoning_effort_is_chat_only_and_json_mode_is_default() {
        let mut chat =
            Conversation::with_tools(WireApi::ChatCompletions, "m", "s", "u", 100, vec![]);
        assert_eq!(chat.body["response_format"]["type"], "json_object");
        assert!(chat.body.get("reasoning_effort").is_none());
        chat.set_reasoning_effort("low");
        assert_eq!(chat.body["reasoning_effort"], "low");

        for wire in [WireApi::Responses, WireApi::Anthropic] {
            let mut conversation = Conversation::with_tools(wire, "m", "s", "u", 100, vec![]);
            conversation.set_reasoning_effort("low");
            assert!(conversation.body.get("reasoning_effort").is_none());
        }
    }

    #[test]
    fn pruning_preserves_call_pairs_reasoning_skills_and_recent_turns_on_all_wires() {
        use crate::context::history::{serialized_bytes, ResultArchive};
        for wire in [
            WireApi::ChatCompletions,
            WireApi::Responses,
            WireApi::Anthropic,
        ] {
            let mut conversation =
                Conversation::with_tools(wire, "fixture", "system", "source", 100, vec![]);
            for turn in 0..4 {
                let calls = vec![ToolCall {
                    id: format!("c{turn}"),
                    name: if turn == 1 { "load_skill" } else { "bash" }.into(),
                    arguments: "{}".into(),
                }];
                let assistant = match wire {
                    WireApi::ChatCompletions => vec![
                        json!({"role":"assistant","reasoning_content":"reasoning","tool_calls":[{"id":calls[0].id,"type":"function","function":{"name":calls[0].name,"arguments":"{}"}}]}),
                    ],
                    WireApi::Responses => vec![
                        json!({"type":"reasoning","id":format!("r{turn}"),"encrypted_content":"opaque"}),
                        json!({"type":"function_call","call_id":calls[0].id,"name":calls[0].name,"arguments":"{}"}),
                    ],
                    WireApi::Anthropic => vec![
                        json!({"role":"assistant","content":[{"type":"thinking","thinking":"reasoning","signature":"signed"},{"type":"tool_use","id":calls[0].id,"name":calls[0].name,"input":{}}]}),
                    ],
                };
                conversation
                    .append(
                        &Turn {
                            text: String::new(),
                            usage: Default::default(),
                            calls,
                            assistant,
                        },
                        &[json!({"stdout":"x".repeat(10000),"exit_code":0}).to_string()],
                    )
                    .unwrap();
            }
            let before = conversation.body.clone();
            let original = before
                .pointer(&conversation.result_slots[0].pointer)
                .unwrap()
                .as_str()
                .unwrap();
            let mut archive = ResultArchive::default();
            let reduction = conversation.reduce_context(30000, 2, &mut archive).unwrap();
            assert_eq!(reduction.pruned_results, 1);
            assert_eq!(
                reduction.after_bytes,
                serialized_bytes(&conversation.body).unwrap()
            );
            assert!(reduction.after_bytes < reduction.before_bytes - 8000);
            let projected: Value = serde_json::from_str(
                conversation
                    .body
                    .pointer(&conversation.result_slots[0].pointer)
                    .unwrap()
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(projected["context_archive"]["path"].as_str().unwrap())
                    .unwrap(),
                original
            );
            // Restore that single result and the complete provider request must match.
            *conversation
                .body
                .pointer_mut(&conversation.result_slots[0].pointer)
                .unwrap() = json!(original);
            assert_eq!(conversation.body, before);
            assert_eq!(
                conversation
                    .reduce_context(30000, 2, &mut archive)
                    .unwrap()
                    .pruned_results,
                0
            );
        }
    }

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
