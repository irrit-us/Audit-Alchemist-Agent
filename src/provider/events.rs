//! Provider-neutral streaming events.
//!
//! Every supported wire format is normalized into the same events so the
//! console renderers and the TUI do not need to know which provider produced
//! them. Tokens are reported as deltas; the final assistant text is the
//! concatenation of [`StreamEvent::Text`] payloads.

use serde::Serialize;

/// Token accounting for one request, when the provider reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub reasoning_tokens: u64,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.prompt_tokens == 0
            && self.completion_tokens == 0
            && self.total_tokens == 0
            && self.reasoning_tokens == 0
    }

    /// Merge a partial update. Fields arrive in separate events depending on
    /// the provider, so each field keeps the largest observed value.
    pub fn merge(&mut self, other: Usage) {
        self.prompt_tokens = self.prompt_tokens.max(other.prompt_tokens);
        self.completion_tokens = self.completion_tokens.max(other.completion_tokens);
        self.total_tokens = self.total_tokens.max(other.total_tokens);
        self.reasoning_tokens = self.reasoning_tokens.max(other.reasoning_tokens);
        if self.total_tokens == 0 && !other.is_empty() {
            self.total_tokens = self.prompt_tokens.saturating_add(self.completion_tokens);
        }
    }
}

/// A normalized streaming event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// A reasoning/thinking delta, when the model exposes one.
    Reasoning { text: String },
    /// An assistant answer delta.
    Text { text: String },
    /// Updated token accounting.
    Usage(Usage),
    /// The stream completed normally.
    Done,
}

/// Consumes events as they are produced.
pub trait EventSink {
    fn on_event(&mut self, event: &StreamEvent);
}

/// Discards every event; used when no console rendering is requested.
impl EventSink for () {
    fn on_event(&mut self, _event: &StreamEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_merge_keeps_largest_fields_and_fills_totals() {
        let mut usage = Usage {
            prompt_tokens: 10,
            ..Usage::default()
        };
        usage.merge(Usage {
            completion_tokens: 4,
            ..Usage::default()
        });
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 14);
        assert!(!usage.is_empty());
        assert!(Usage::default().is_empty());
    }

    #[test]
    fn events_serialize_with_a_type_tag() {
        let text = serde_json::to_value(StreamEvent::Text { text: "hi".into() }).unwrap();
        assert_eq!(text["type"], "text");
        assert_eq!(text["text"], "hi");
        let done = serde_json::to_value(StreamEvent::Done).unwrap();
        assert_eq!(done["type"], "done");
        let usage = serde_json::to_value(StreamEvent::Usage(Usage {
            total_tokens: 3,
            ..Usage::default()
        }))
        .unwrap();
        assert_eq!(usage["type"], "usage");
        assert_eq!(usage["total_tokens"], 3);
    }
}
