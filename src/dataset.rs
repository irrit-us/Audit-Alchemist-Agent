use crate::protocol::{relative_path, FindingKey, VERSION};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub schema_version: u32,
    pub name: String,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub target: String,
    pub instruction: String,
    pub expected: Vec<FindingKey>,
}

pub fn load(path: &Path) -> Result<(Dataset, PathBuf)> {
    let file = File::open(path).with_context(|| format!("open dataset {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "dataset exceeds 4 MiB");
    let dataset: Dataset = serde_json::from_slice(&bytes).context("parse dataset JSON")?;
    let root = path
        .canonicalize()?
        .parent()
        .context("dataset has no parent")?
        .to_owned();
    dataset.validate(&root)?;
    Ok((dataset, root))
}

impl Dataset {
    pub fn validate(&self, root: &Path) -> Result<()> {
        ensure!(
            self.schema_version == VERSION,
            "unsupported dataset schema version"
        );
        ensure!(!self.name.trim().is_empty(), "dataset name is empty");
        ensure!(
            !self.cases.is_empty() && self.cases.len() <= 10_000,
            "dataset requires 1..=10000 cases"
        );
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            ensure!(
                !case.id.is_empty()
                    && case.id.len() <= 128
                    && case
                        .id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                "invalid case ID"
            );
            ensure!(ids.insert(&case.id), "duplicate case ID: {}", case.id);
            ensure!(
                !case.instruction.trim().is_empty() && case.instruction.len() <= 65_536,
                "invalid instruction for {}",
                case.id
            );
            relative_path(&case.target)?;
            let target = root
                .join(&case.target)
                .canonicalize()
                .with_context(|| format!("missing target {}", case.target))?;
            ensure!(target.starts_with(root), "target escapes dataset root");
            ensure!(
                target.is_file() || target.is_dir(),
                "target must be a file or directory"
            );
            ensure!(case.expected.len() <= 10_000, "too many expected findings");
            let mut keys = BTreeSet::new();
            for key in &case.expected {
                key.validate()?;
                ensure!(
                    keys.insert(key),
                    "duplicate expected finding in {}",
                    case.id
                );
                let source = root
                    .join(&key.path)
                    .canonicalize()
                    .context("expected finding source missing")?;
                ensure!(
                    source.starts_with(root) && source.is_file(),
                    "expected finding source escapes root or is not a file"
                );
                ensure!(
                    source == target || (target.is_dir() && source.starts_with(&target)),
                    "expected finding is outside case target"
                );
                let file = File::open(&source)?;
                let mut bytes = Vec::new();
                file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() <= 4 * 1024 * 1024,
                    "expected source exceeds 4 MiB"
                );
                let lines =
                    bytes.split(|b| *b == b'\n').count() - usize::from(bytes.ends_with(b"\n"));
                ensure!(
                    key.line as usize <= lines,
                    "expected finding line is outside source"
                );
            }
        }
        Ok(())
    }
}
