use crate::protocol::{relative_path, Request, Response, VERSION};
use anyhow::{ensure, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub const SYSTEM_PROMPT: &str = include_str!("../prompts/audit.txt");
const MAX_FILES: usize = 128;

#[derive(Debug, Clone, Args)]
pub struct LlmOptions {
    /// Full URL of a chat-completions compatible API (HTTPS, or localhost HTTP).
    #[arg(long)]
    pub endpoint: String,
    #[arg(long)]
    pub model: String,
    /// Name of the environment variable holding the bearer token.
    #[arg(long, default_value = "AUDIT_API_KEY")]
    pub api_key_env: String,
    #[arg(long, default_value_t = 262_144, value_parser = clap::value_parser!(u32).range(1..=1_048_576))]
    pub max_source_bytes: u32,
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u32).range(1..=32768))]
    pub max_tokens: u32,
}

impl LlmOptions {
    pub fn validate(&self) -> Result<reqwest::Url> {
        let url = reqwest::Url::parse(&self.endpoint).context("invalid endpoint URL")?;
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "endpoint must not contain credentials, query, or fragment"
        );
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && local),
            "endpoint requires HTTPS, except loopback HTTP for local models"
        );
        ensure!(!self.model.trim().is_empty(), "model is empty");
        let key = std::env::var(&self.api_key_env).with_context(|| {
            format!("missing API key environment variable {}", self.api_key_env)
        })?;
        ensure!(!key.trim().is_empty(), "API key is empty");
        Ok(url)
    }

    pub fn arguments(&self) -> Vec<String> {
        vec![
            "agent".into(),
            "--endpoint".into(),
            self.endpoint.clone(),
            "--model".into(),
            self.model.clone(),
            "--api-key-env".into(),
            self.api_key_env.clone(),
            "--max-source-bytes".into(),
            self.max_source_bytes.to_string(),
            "--max-tokens".into(),
            self.max_tokens.to_string(),
        ]
    }
}

#[derive(Debug, Serialize)]
pub struct Source {
    pub path: String,
    pub content: String,
}

pub fn snapshot(root: &Path, target: &str, max_bytes: usize) -> Result<Vec<Source>> {
    relative_path(target)?;
    let root = root.canonicalize()?;
    let target = root
        .join(target)
        .canonicalize()
        .context("target not found")?;
    ensure!(target.starts_with(&root), "target escapes root");
    let mut pending = vec![target];
    let mut files: Vec<PathBuf> = Vec::new();
    let mut entries = 0;
    while let Some(path) = pending.pop() {
        entries += 1;
        ensure!(
            entries <= 10_000,
            "target contains too many directory entries"
        );
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                if matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | ".venv" | "vendor")
                ) {
                    continue;
                }
                pending.push(entry.path());
                ensure!(
                    pending.len() <= 10_000,
                    "too many pending directory entries"
                );
            }
        } else if metadata.is_file() && supported(&path) {
            files.push(path);
            ensure!(
                files.len() <= MAX_FILES,
                "target exceeds {MAX_FILES} source files"
            );
        }
    }
    files.sort();
    ensure!(!files.is_empty(), "no supported source files in target");
    let mut sources = Vec::new();
    let mut used = 0;
    for path in files {
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take((max_bytes - used) as u64 + 1)
            .read_to_end(&mut bytes)?;
        used += bytes.len();
        ensure!(
            used <= max_bytes,
            "source snapshot exceeds byte limit; narrow the target"
        );
        let content = String::from_utf8(bytes).context("source is not UTF-8")?;
        let path = path
            .strip_prefix(&root)?
            .to_str()
            .context("source path is not UTF-8")?
            .replace('\\', "/");
        relative_path(&path)?;
        sources.push(Source { path, content });
    }
    Ok(sources)
}

fn supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "py"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "go"
                | "java"
                | "sol"
                | "rb"
                | "php"
                | "sh"
                | "cs"
        )
    )
}

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct Message {
    content: String,
}

pub async fn audit(
    options: &LlmOptions,
    request: &Request,
    root: &Path,
    timeout: Duration,
) -> Result<Response> {
    let url = options.validate()?;
    ensure!(
        request.schema_version == VERSION,
        "unsupported request version"
    );
    ensure!(
        !request.instruction.trim().is_empty() && request.instruction.len() <= 65_536,
        "invalid instruction"
    );
    let sources = snapshot(root, &request.target, options.max_source_bytes as usize)?;
    let user = serde_json::to_string(
        &serde_json::json!({"instruction": request.instruction, "sources": sources}),
    )?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let key = std::env::var(&options.api_key_env)?;
    let mut response = client.post(url).bearer_auth(key).json(&serde_json::json!({
        "model": options.model, "temperature": 0, "max_tokens": options.max_tokens,
        "messages": [{"role": "system", "content": SYSTEM_PROMPT}, {"role": "user", "content": user}]
    })).send().await.map_err(|_| anyhow::anyhow!("LLM API request failed (connection, TLS, or deadline)"))?;
    ensure!(
        response.status().is_success(),
        "LLM API returned HTTP {}",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read LLM API body")? {
        ensure!(
            bytes.len() + chunk.len() <= 2 * 1024 * 1024,
            "LLM API response exceeds 2 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let completion: Completion =
        serde_json::from_slice(&bytes).context("invalid chat-completions response")?;
    ensure!(
        completion.choices.len() == 1,
        "expected exactly one completion choice"
    );
    let choice = &completion.choices[0];
    ensure!(
        choice.finish_reason.as_deref() == Some("stop"),
        "LLM did not finish normally"
    );
    let findings: Response = serde_json::from_str(&choice.message.content)
        .context("model must return a JSON response without Markdown fences")?;
    findings.validate()?;
    for finding in &findings.findings {
        let source = sources
            .iter()
            .find(|s| s.path == finding.path)
            .context("finding references a file outside the source snapshot")?;
        ensure!(
            finding.line as usize <= source.content.lines().count(),
            "finding references a line outside its source"
        );
    }
    Ok(findings)
}
