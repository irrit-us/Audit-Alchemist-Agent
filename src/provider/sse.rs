//! Incremental Server-Sent Events line reader.
//!
//! Feeds provider streams from arbitrary byte chunks and returns the payload of
//! each complete `data:` line. Line and character boundaries may split across
//! chunks; only complete lines are decoded. Non-`data:` lines (comments,
//! `event:`, keepalives) are ignored. The literal `[DONE]` sentinel is returned
//! as-is so each wire handler can decide what it means.

use anyhow::{Context, Result};

/// A byte-fed SSE reader.
#[derive(Default)]
pub struct SseLines {
    buffer: Vec<u8>,
}

impl SseLines {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk and return every complete `data:` payload it completes.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.buffer.extend_from_slice(bytes);
        let mut payloads = Vec::new();
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = &line[..line.len() - 1];
            let line = std::str::from_utf8(line).context("stream is not UTF-8")?;
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(data) = data_payload(line) {
                payloads.push(data.to_owned());
            }
        }
        Ok(payloads)
    }

    /// Flush any trailing line that was not newline-terminated.
    pub fn finish(mut self) -> Result<Vec<String>> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        let line =
            String::from_utf8(std::mem::take(&mut self.buffer)).context("stream is not UTF-8")?;
        Ok(match data_payload(line.trim_end_matches(['\r', '\n'])) {
            Some(data) => vec![data.to_owned()],
            None => Vec::new(),
        })
    }
}

fn data_payload(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data:")?.trim_start();
    (!data.is_empty()).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_complete_data_lines_and_ignores_other_lines() {
        let mut lines = SseLines::new();
        let payloads = lines
            .push(b": keepalive\nevent: message\ndata: {\"a\":1}\n\ndata: [DONE]\n")
            .unwrap();
        assert_eq!(payloads, vec!["{\"a\":1}".to_owned(), "[DONE]".to_owned()]);
    }

    #[test]
    fn handles_chunk_boundaries_and_crlf() {
        let mut lines = SseLines::new();
        assert!(lines.push(b"data: {\"a\"").unwrap().is_empty());
        let payloads = lines.push(b":2}\r\n").unwrap();
        assert_eq!(payloads, vec!["{\"a\":2}".to_owned()]);
        assert!(lines.finish().unwrap().is_empty());
    }

    #[test]
    fn flushes_a_trailing_line() {
        let mut lines = SseLines::new();
        lines.push(b"data: bare").unwrap();
        assert_eq!(lines.finish().unwrap(), vec!["bare".to_owned()]);
    }

    #[test]
    fn rejects_non_utf8() {
        let mut lines = SseLines::new();
        assert!(lines.push(b"data: \xff\n").is_err());
    }
}
