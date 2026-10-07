use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

pub const VERSION: u32 = 1;

/// Ground truth is deliberately absent from the agent's request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub case_id: String,
    pub target: String,
    pub instruction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct FindingKey {
    pub cwe: String,
    pub path: String,
    pub line: u32,
}

impl FindingKey {
    pub fn validate(&self) -> Result<()> {
        let digits = self.cwe.strip_prefix("CWE-").unwrap_or("");
        ensure!(
            !digits.is_empty()
                && digits.bytes().all(|c| c.is_ascii_digit())
                && !digits.starts_with('0')
                && digits.len() <= 6,
            "CWE must have the canonical form CWE-<positive integer>"
        );
        relative_path(&self.path)?;
        ensure!(self.line > 0, "finding line must be positive");
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub cwe: String,
    pub path: String,
    pub line: u32,
    pub severity: Severity,
    pub title: String,
    pub evidence: String,
}

impl Finding {
    pub fn key(&self) -> FindingKey {
        FindingKey {
            cwe: self.cwe.clone(),
            path: self.path.clone(),
            line: self.line,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub schema_version: u32,
    pub findings: Vec<Finding>,
}

impl Response {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == VERSION,
            "unsupported response schema version"
        );
        ensure!(self.findings.len() <= 10_000, "too many findings");
        for finding in &self.findings {
            finding.key().validate()?;
            ensure!(
                !finding.title.trim().is_empty() && finding.title.len() <= 1024,
                "invalid finding title"
            );
            ensure!(
                !finding.evidence.trim().is_empty() && finding.evidence.len() <= 16_384,
                "invalid finding evidence"
            );
        }
        Ok(())
    }
}

/// Portable, normalized paths make cross-run matching unambiguous.
pub fn relative_path(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 4096,
        "invalid path length"
    );
    if value.contains('\\') || value.contains(':') || value.contains('\0') {
        bail!("path must use portable relative POSIX syntax");
    }
    ensure!(
        value
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".."),
        "path must be normalized and relative"
    );
    ensure!(
        Path::new(value)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "path must be relative"
    );
    Ok(())
}
