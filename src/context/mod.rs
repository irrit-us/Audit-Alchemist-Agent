//! Context management: assemble a finite, deterministic source snapshot for a
//! single audit target and validate that findings refer to it.
//!
//! The model only ever sees what this module assembles. Budgets are explicit
//! and enforced by failing, never by silently truncating source, so a report
//! can always be traced back to the exact bytes the model received.

pub mod tools;

use crate::protocol::Finding;
use anyhow::{ensure, Context as _, Result};
use serde::Serialize;
use std::path::Path;

use self::tools::{SourceRoot, WalkLimits};

/// Maximum source files in one snapshot regardless of caller budget.
pub const MAX_FILES: usize = 128;
/// Maximum directory entries visited while resolving one target.
pub const MAX_ENTRIES: usize = 10_000;

/// A source file and its normalized root-relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    pub path: String,
    pub content: String,
}

/// Explicit limits for context assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub max_bytes: usize,
    pub max_files: usize,
    pub max_entries: usize,
}

impl ContextBudget {
    /// A byte budget with the standard file and entry caps.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            max_files: MAX_FILES,
            max_entries: MAX_ENTRIES,
        }
    }
}

/// The finite source context supplied to one model call.
#[derive(Debug, Clone, Serialize)]
pub struct Context {
    pub sources: Vec<Source>,
    pub total_bytes: usize,
}

impl Context {
    /// Find an included source by its normalized path.
    pub fn locate(&self, path: &str) -> Option<&Source> {
        self.sources.iter().find(|source| source.path == path)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn files(&self) -> usize {
        self.sources.len()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Rough token estimate for the source text, for budgeting and logs.
    ///
    /// This is deliberately a heuristic, not a tokenizer: see [`estimate_tokens`].
    pub fn estimated_tokens(&self) -> usize {
        self.sources
            .iter()
            .map(|source| estimate_tokens(&source.content))
            .sum()
    }

    /// Require that a finding points at an included file and an existing line.
    pub fn validate_finding(&self, finding: &Finding) -> Result<()> {
        let source = self
            .locate(&finding.path)
            .context("finding references a file outside the source snapshot")?;
        ensure!(
            finding.line as usize <= source.content.lines().count(),
            "finding references a line outside its source"
        );
        Ok(())
    }
}

/// Build the context for `target`, enforcing `budget` and failing on overflow.
pub fn build(root: &Path, target: &str, budget: &ContextBudget) -> Result<Context> {
    let root = SourceRoot::open(root)?;
    let entries = root.walk(
        target,
        WalkLimits {
            max_entries: budget.max_entries,
            max_files: budget.max_files,
        },
    )?;
    ensure!(!entries.is_empty(), "no supported source files in target");
    let mut sources = Vec::with_capacity(entries.len());
    let mut total_bytes = 0usize;
    for entry in entries {
        let remaining = budget.max_bytes - total_bytes;
        let file = root
            .read(&entry.path, remaining)
            .context("source snapshot exceeds byte limit; narrow the target")?;
        total_bytes += file.content.len();
        sources.push(Source {
            path: file.path,
            content: file.content,
        });
    }
    Ok(Context {
        sources,
        total_bytes,
    })
}

/// The source list for one target, preserving the original helper signature.
pub fn snapshot(root: &Path, target: &str, max_bytes: usize) -> Result<Vec<Source>> {
    Ok(build(root, target, &ContextBudget::new(max_bytes))?.sources)
}

/// A rough token count for a block of source text.
///
/// Most tokenizers emit roughly one token per four UTF-8 bytes of code and
/// prose, so `ceil(bytes / 4)` is a stable, dependency-free approximation.
/// It is intentionally conservative for budgeting but is not a substitute for
/// a real tokenizer; callers must treat it as an estimate. A byte bound is
/// still the hard limit on what is sent.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_is_ceil_of_quarter_bytes() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // Multi-byte characters are counted by encoded byte length.
        assert_eq!(estimate_tokens("\u{00e9}"), 1);
        assert_eq!(estimate_tokens("\u{4e00}"), 1);
    }
}
