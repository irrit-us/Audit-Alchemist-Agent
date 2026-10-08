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

/// Give each tool-using evaluation case its own writable copy, without the
/// dataset's labels. This prevents normal PoC writes from contaminating cases;
/// it is not an OS sandbox and cannot hide host files from arbitrary shell code.
pub fn stage_workspace(root: &Path, manifest: &Path) -> Result<tempfile::TempDir> {
    let root = root.canonicalize()?;
    let manifest = manifest.canonicalize()?;
    let workspace = tempfile::tempdir().context("create evaluation workspace")?;
    let mut pending = vec![root.clone()];
    let mut entries = 0usize;
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        entries += 1;
        ensure!(
            entries <= 10_000,
            "evaluation workspace exceeds 10000 entries; use a smaller dataset root"
        );
        if path == manifest {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let destination = workspace.path().join(path.strip_prefix(&root)?);
        if metadata.is_dir() {
            std::fs::create_dir_all(destination)?;
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| crate::context::tools::SKIP_DIRS.contains(&name))
                {
                    continue;
                }
                pending.push(entry.path());
                ensure!(
                    entries + pending.len() <= 10_000,
                    "evaluation workspace exceeds 10000 entries; use a smaller dataset root"
                );
            }
        } else if metadata.is_file() {
            let remaining = (64 * 1024 * 1024u64).saturating_sub(bytes);
            let mut input = File::open(&path)?.take(remaining + 1);
            let mut output = File::create(&destination)?;
            let copied = std::io::copy(&mut input, &mut output)?;
            ensure!(
                copied <= remaining,
                "evaluation workspace exceeds 64 MiB; use a smaller dataset root"
            );
            bytes += copied;
            std::fs::set_permissions(destination, metadata.permissions())?;
        }
    }
    Ok(workspace)
}

impl Dataset {
    pub fn validate(&self, root: &Path) -> Result<()> {
        // Canonicalize both sides of containment checks (Windows uses verbatim
        // path prefixes for canonicalized paths).
        let root = root.canonicalize().context("canonicalize dataset root")?;
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
            ensure!(target.starts_with(&root), "target escapes dataset root");
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
                    source.starts_with(&root) && source.is_file(),
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
