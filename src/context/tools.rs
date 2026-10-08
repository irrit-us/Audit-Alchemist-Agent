//! Read-only, bounded tool invocation over an untrusted source tree.
//!
//! These helpers provide source access for the broader model-facing toolset:
//! resolving a path inside a canonical root, reading UTF-8 source
//! with a hard byte cap, listing source files, and searching lines. Every
//! operation is deterministic, refuses paths that escape the root, does not
//! follow symlinks discovered during traversal, and fails rather than silently
//! truncating. Shell execution and editing live in `crate::tools`.

use crate::protocol::relative_path;
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Dependency and build directories that are never useful audit context.
pub const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".venv", "vendor"];

/// File extensions treated as auditable source code.
pub const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "c", "h", "cc", "cpp", "hpp", "js", "jsx", "ts", "tsx", "go", "java", "sol", "rb",
    "php", "sh", "cs",
];

/// True when `path` carries a supported source extension.
pub fn is_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension))
}

/// A single UTF-8 source file, addressed by its normalized root-relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}

/// A source file discovered by a traversal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileEntry {
    pub path: String,
}

/// Bounds for a directory traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkLimits {
    pub max_entries: usize,
    pub max_files: usize,
}

impl Default for WalkLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_files: 128,
        }
    }
}

/// Bounds for a line search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_files: usize,
    pub max_bytes: usize,
    pub max_matches: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_files: 128,
            max_bytes: 262_144,
            max_matches: 1_000,
        }
    }
}

/// A single matching source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// A canonicalized, read-only view of a source tree.
#[derive(Debug, Clone)]
pub struct SourceRoot {
    root: PathBuf,
}

impl SourceRoot {
    /// Canonicalize `root` and require that it is an existing directory.
    pub fn open(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("canonicalize source root {}", root.display()))?;
        ensure!(root.is_dir(), "source root must be a directory");
        Ok(Self { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Convert an absolute path that lies under the root into portable POSIX form.
    pub fn relative(&self, path: &Path) -> Result<String> {
        let relative = path
            .strip_prefix(&self.root)
            .context("path escapes source root")?
            .to_str()
            .context("source path is not UTF-8")?
            .replace('\\', "/");
        relative_path(&relative)?;
        Ok(relative)
    }

    /// Validate a caller-provided relative path and resolve it inside the root.
    ///
    /// Symlinks are followed by `canonicalize`, so a link that points outside
    /// the root is rejected by the containment check.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        if relative == "." {
            return Ok(self.root.clone());
        }
        relative_path(relative)?;
        let path = self
            .root
            .join(relative)
            .canonicalize()
            .with_context(|| format!("target not found: {relative}"))?;
        ensure!(path.starts_with(&self.root), "target escapes source root");
        Ok(path)
    }

    /// Read one UTF-8 file, failing when it exceeds `max_bytes`.
    pub fn read(&self, relative: &str, max_bytes: usize) -> Result<SourceFile> {
        let path = self.resolve(relative)?;
        ensure!(path.is_file(), "not a file: {relative}");
        let content = read_utf8(&path, max_bytes)?;
        Ok(SourceFile {
            path: self.relative(&path)?,
            content,
        })
    }

    /// Recursively collect supported source files under `target`.
    ///
    /// Traversal is deterministic, prunes [`SKIP_DIRS`], never descends into a
    /// symlink, and enforces entry and file caps.
    pub fn walk(&self, target: &str, limits: WalkLimits) -> Result<Vec<FileEntry>> {
        let start = self.resolve(target)?;
        let mut pending = vec![start];
        let mut files = Vec::new();
        let mut entries = 0usize;
        while let Some(path) = pending.pop() {
            entries += 1;
            ensure!(
                entries <= limits.max_entries,
                "target contains too many directory entries"
            );
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                for entry in fs::read_dir(&path)? {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| SKIP_DIRS.contains(&name))
                    {
                        continue;
                    }
                    pending.push(entry.path());
                    ensure!(
                        pending.len() <= limits.max_entries,
                        "too many pending directory entries"
                    );
                }
            } else if metadata.is_file() && is_source_path(&path) {
                files.push(FileEntry {
                    path: self.relative(&path)?,
                });
                ensure!(
                    files.len() <= limits.max_files,
                    "target exceeds {} source files",
                    limits.max_files
                );
            }
        }
        files.sort();
        Ok(files)
    }

    /// Search supported source files under `target` for a literal substring.
    pub fn search(
        &self,
        target: &str,
        needle: &str,
        limits: SearchLimits,
    ) -> Result<Vec<LineMatch>> {
        self.search_many(target, &[needle], limits)
    }

    /// Search for any of 1..=64 literal substrings in a single traversal.
    ///
    /// Each matching line is returned once in path/line order, even if several
    /// needles match. Limits apply to the entire operation, not each needle.
    /// An incomplete search is an error, never an apparently complete result.
    pub fn search_many(
        &self,
        target: &str,
        needles: &[&str],
        limits: SearchLimits,
    ) -> Result<Vec<LineMatch>> {
        ensure!(
            !needles.is_empty() && needles.len() <= 64,
            "search requires 1..=64 literal needles"
        );
        ensure!(
            needles.iter().all(|needle| !needle.is_empty()),
            "search needle is empty"
        );
        let entries = self.walk(
            target,
            WalkLimits {
                max_entries: limits.max_files.saturating_mul(64).max(1),
                max_files: limits.max_files,
            },
        )?;
        let mut matches = Vec::new();
        let mut used = 0usize;
        for entry in entries {
            let file = self
                .read(&entry.path, limits.max_bytes - used)
                .with_context(|| {
                    format!(
                        "search could not read {}; narrow the target or raise the byte limit",
                        entry.path
                    )
                })?;
            used += file.content.len();
            for (index, text) in file.content.lines().enumerate() {
                if needles.iter().any(|needle| text.contains(needle)) {
                    matches.push(LineMatch {
                        path: file.path.clone(),
                        line: index as u32 + 1,
                        text: text.to_owned(),
                    });
                    ensure!(
                        matches.len() <= limits.max_matches,
                        "search exceeds {} matches; narrow the target or needles",
                        limits.max_matches
                    );
                }
            }
        }
        Ok(matches)
    }
}

/// Read a file as UTF-8, failing when it exceeds `max_bytes`.
pub fn read_utf8(path: &Path, max_bytes: usize) -> Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= max_bytes, "file exceeds byte limit");
    String::from_utf8(bytes).context("source is not UTF-8")
}
