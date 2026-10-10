//! Model-facing workspace tools. Bash runs with host permissions, not in a sandbox.

mod read;

use crate::{
    context::{
        tools::{SearchLimits, SourceRoot, WalkLimits},
        Context as SourceContext,
    },
    protocol::{relative_path, Finding},
};
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub const OUTPUT_BYTES: usize = 32_768;
const FILE_BYTES: usize = 1_048_576;

pub fn definitions() -> Vec<Value> {
    let string = || json!({"type":"string"});
    let integer = || json!({"type":"integer","minimum":1});
    let tool = |name, description, properties, required| {
        json!({
            "name":name,"description":description,
            "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}
        })
    };
    vec![
        tool("load_skill", "Load built-in guidance by exact catalog name. Omit resource for SKILL.md, then request an exact available_resources path if needed. Use save_to with an explicit resource to write the bundled script/reference to a new root-relative file without returning its content. Parent directories must exist; existing files are never overwritten. Run saved scripts through Bash. Content is compiled into the binary.", json!({"name":string(),"resource":string(),"save_to":string()}), json!(["name"])),
        tool("bash", "Execute command as Bash in the audit root with host permissions. Shell state resets each call; files persist. No interactive stdin. Output is bounded and truncation marked. timeout_ms defaults to 30000, capped by the run deadline.", json!({"command":string(),"timeout_ms":integer()}), json!(["command"])),
        tool("read_file", "Read UTF-8 text with 1-based line labels. Use offset and limit to page through files. Read the finding's source line with this tool before citing it. Streams the requested page without loading the whole file. Scan cap 64 MiB per call; output cap 32 KiB. total_lines is null until EOF. Paths are relative to the audit root.", json!({"path":string(),"offset":integer(),"limit":integer()}), json!(["path"])),
        tool("write_file", "Create or overwrite a UTF-8 file relative to the audit root, including PoCs and test fixtures. Parent directories must exist (use Bash mkdir -p). Read existing files before overwriting. Maximum content 1 MiB.", json!({"path":string(),"content":string()}), json!(["path","content"])),
        tool("edit_file", "Replace exactly one occurrence of old_text with new_text in a UTF-8 file. Read first; on missing or ambiguous matches, re-read and provide a unique exact match. Paths are root-relative.", json!({"path":string(),"old_text":string(),"new_text":string()}), json!(["path","old_text","new_text"])),
        tool("list_files", "List supported source paths under a root-relative directory or file (use . for root). Prunes dependency/build directories. At most 128 files; narrow the target on overflow, or use Bash rg --files for broader exploration.", json!({"target":string()}), json!(["target"])),
        tool("search", "Search source lines for any of 1..64 case-sensitive literal needles in one scan. Returns each line once with path and line. Use . for root. Caps: 128 files, 1 MiB scanned, 200 matches; narrow target on overflow. For regex or other file types use Bash rg/grep.", json!({"target":string(),"needles":{"type":"array","items":string(),"minItems":1,"maxItems":64}}), json!(["target","needles"])),
    ]
}

pub struct WorkspaceTools {
    root: SourceRoot,
    settings: crate::config::AgentSettings,
    // Only lines actually supplied through initial context/read_file may be cited.
    observed: BTreeMap<String, BTreeSet<u32>>,
    // Content fingerprint of each observed line, checked before a citation is
    // accepted so a Bash or external edit cannot stale a finding.
    fingerprints: BTreeMap<String, BTreeMap<u32, u64>>,
}

impl WorkspaceTools {
    pub fn new(root: &Path, initial: &SourceContext) -> Result<Self> {
        Self::configured(root, initial, Default::default())
    }

    pub fn configured(
        root: &Path,
        initial: &SourceContext,
        settings: crate::config::AgentSettings,
    ) -> Result<Self> {
        let mut observed = BTreeMap::new();
        let mut fingerprints = BTreeMap::new();
        for source in &initial.sources {
            let mut lines = BTreeSet::new();
            let mut hashes = BTreeMap::new();
            for (index, text) in source.content.lines().enumerate() {
                let line = index as u32 + 1;
                lines.insert(line);
                hashes.insert(line, fingerprint(text.as_bytes()));
            }
            observed.insert(source.path.clone(), lines);
            fingerprints.insert(source.path.clone(), hashes);
        }
        Ok(Self {
            root: SourceRoot::open(root)?,
            settings,
            observed,
            fingerprints,
        })
    }

    pub fn validate_finding(&self, finding: &Finding) -> Result<()> {
        let expected = self
            .fingerprints
            .get(&finding.path)
            .and_then(|lines| lines.get(&finding.line))
            .copied();
        ensure!(
            expected.is_some(),
            "finding must cite a source line supplied in the initial context or read_file: {}:{}",
            finding.path,
            finding.line
        );
        // Re-read the cited line so a change made by Bash or another process
        // after the read cannot be cited as if it were the inspected source.
        let page = read::read(&self.root, &finding.path, finding.line as usize, 1)
            .with_context(|| format!("re-read cited source {}:{}", finding.path, finding.line))?;
        ensure!(
            page.texts.first().map(|text| fingerprint(text.as_bytes())) == expected,
            "cited source changed after it was read; re-read {}:{} before reporting",
            finding.path,
            finding.line
        );
        Ok(())
    }

    pub async fn execute(&mut self, name: &str, arguments: &str, timeout: Duration) -> Value {
        match self.invoke(name, arguments, timeout).await {
            Ok(result) => result,
            Err(error) => json!({"error":format!("{error:#}")}),
        }
    }

    async fn invoke(&mut self, name: &str, arguments: &str, timeout: Duration) -> Result<Value> {
        ensure!(self.settings.tool_enabled(name), "tool is disabled");
        match name {
            "load_skill" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    name: String,
                    resource: Option<String>,
                    save_to: Option<String>,
                }
                let args: Args = serde_json::from_str(arguments)?;
                ensure!(
                    args.save_to.is_none() || args.resource.is_some(),
                    "save_to requires an explicit resource"
                );
                let skill = self
                    .settings
                    .load_skill(&args.name, args.resource.as_deref())?;
                if let Some(relative) = args.save_to {
                    let path = self.write_path(&relative)?;
                    let content = skill["content"]
                        .as_str()
                        .context("missing built-in resource")?;
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)
                        .context("save skill resource; destination must be a new file")?;
                    file.write_all(content.as_bytes())?;
                    file.flush()?;
                    Ok(
                        json!({"name":args.name,"resource":args.resource,"path":relative,"bytes_written":content.len(),"available_resources":skill["available_resources"]}),
                    )
                } else {
                    Ok(skill)
                }
            }
            "bash" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    command: String,
                    timeout_ms: Option<u64>,
                }
                let args: Args = serde_json::from_str(arguments)?;
                ensure!(!args.command.trim().is_empty(), "command is empty");
                ensure!(args.timeout_ms != Some(0), "timeout_ms must be positive");
                let timeout = timeout.min(Duration::from_millis(
                    args.timeout_ms
                        .unwrap_or(self.settings.tools.bash_timeout_ms),
                ));
                bash_with(
                    self.root.path(),
                    &args.command,
                    timeout,
                    self.settings.tools.bash_program.as_deref(),
                )
                .await
            }
            "read_file" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    path: String,
                    offset: Option<usize>,
                    limit: Option<usize>,
                }
                let args: Args = serde_json::from_str(arguments)?;
                let read::Page {
                    path,
                    offset,
                    lines,
                    texts,
                    result,
                } = read::read(
                    &self.root,
                    &args.path,
                    args.offset.unwrap_or(1),
                    args.limit.unwrap_or(200),
                )?;
                self.observed
                    .entry(path.clone())
                    .or_default()
                    .extend((offset..offset + lines).map(|line| line as u32));
                let hashes = self.fingerprints.entry(path).or_default();
                for (index, text) in texts.iter().enumerate() {
                    hashes.insert((offset + index) as u32, fingerprint(text.as_bytes()));
                }
                Ok(result)
            }

            "write_file" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    path: String,
                    content: String,
                }
                let args: Args = serde_json::from_str(arguments)?;
                ensure!(args.content.len() <= FILE_BYTES, "content exceeds 1 MiB");
                let path = self.write_path(&args.path)?;
                fs::write(&path, &args.content)?;
                let relative = self.root.relative(&path)?;
                self.observed.remove(&relative);
                self.fingerprints.remove(&relative);
                Ok(json!({"path":args.path,"bytes_written":args.content.len()}))
            }
            "edit_file" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    path: String,
                    old_text: String,
                    new_text: String,
                }
                let args: Args = serde_json::from_str(arguments)?;
                ensure!(!args.old_text.is_empty(), "old_text is empty");
                let file = self.root.read(&args.path, FILE_BYTES)?;
                ensure!(
                    file.content.matches(&args.old_text).count() == 1,
                    "old_text must match exactly once; re-read the file and use a unique match"
                );
                let updated = file.content.replacen(&args.old_text, &args.new_text, 1);
                ensure!(updated.len() <= FILE_BYTES, "edited file exceeds 1 MiB");
                fs::write(self.root.resolve(&args.path)?, updated)?;
                self.observed.remove(&file.path);
                self.fingerprints.remove(&file.path);
                Ok(json!({"path":file.path,"replacements":1}))
            }
            "list_files" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    target: String,
                }
                let args: Args = serde_json::from_str(arguments)?;
                let files = self.root.walk(&args.target, WalkLimits::default())?;
                Ok(json!({"paths":files.into_iter().map(|f| f.path).collect::<Vec<_>>()}))
            }
            "search" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    target: String,
                    needles: Vec<String>,
                }
                let args: Args = serde_json::from_str(arguments)?;
                let needles: Vec<_> = args.needles.iter().map(String::as_str).collect();
                let matches = self.root.search_many(
                    &args.target,
                    &needles,
                    SearchLimits {
                        max_files: 128,
                        max_bytes: FILE_BYTES,
                        max_matches: 200,
                    },
                )?;
                Ok(
                    json!({"matches":matches.into_iter().map(|m| json!({"path":m.path,"line":m.line,"text":m.text})).collect::<Vec<_>>()}),
                )
            }
            _ => bail!("unknown tool {name}; use a tool from the supplied definitions"),
        }
    }

    fn write_path(&self, relative: &str) -> Result<PathBuf> {
        relative_path(relative)?;
        let path = self.root.path().join(relative);
        if path.symlink_metadata().is_ok() {
            return self.root.resolve(relative);
        }
        let parent = path
            .parent()
            .context("missing parent directory")?
            .canonicalize()
            .context("parent directory must exist; create it with Bash mkdir -p")?;
        ensure!(
            parent.starts_with(self.root.path()),
            "write path escapes audit root"
        );
        Ok(parent.join(path.file_name().context("missing file name")?))
    }
}

/// FNV-1a 64-bit change detector for observed source lines. It is deliberately
/// not a cryptographic authenticator; it detects accidental or tool-driven
/// mutation between the read and the citation.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn bounded_output(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut head = limit / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - limit / 2;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n[output truncated: {} bytes omitted; narrow the command or read a smaller range]\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

fn bash_program() -> PathBuf {
    if let Some(path) = std::env::var_os("AUDIT_BASH") {
        return path.into();
    }
    #[cfg(windows)]
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        let git_bash = PathBuf::from(programs).join("Git/bin/bash.exe");
        if git_bash.is_file() {
            return git_bash;
        }
    }
    "bash".into()
}

#[derive(Default)]
struct CaptureBuffer {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl CaptureBuffer {
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len());
        let prefix = bytes
            .len()
            .min((OUTPUT_BYTES / 2).saturating_sub(self.head.len()));
        self.head.extend_from_slice(&bytes[..prefix]);
        self.tail.extend(&bytes[prefix..]);
        let excess = self.tail.len().saturating_sub(OUTPUT_BYTES / 2);
        self.tail.drain(..excess);
    }

    fn render(&self) -> (String, bool) {
        let truncated = self.total > OUTPUT_BYTES;
        let mut bytes = self.head.clone();
        if truncated {
            // Trim a cut UTF-8 character at either truncation boundary.
            if let Err(error) = std::str::from_utf8(&bytes) {
                if error.error_len().is_none() {
                    bytes.truncate(error.valid_up_to());
                }
            }
            bytes.extend_from_slice(b"\n[output truncated; narrow the command]\n");
            bytes.extend(
                self.tail
                    .iter()
                    .copied()
                    .skip_while(|byte| byte & 0xc0 == 0x80),
            );
        } else {
            // Decode once: a character can straddle the head/tail boundary.
            bytes.extend(self.tail.iter());
        }
        (String::from_utf8_lossy(&bytes).into_owned(), truncated)
    }
}

async fn capture(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    output: &mut CaptureBuffer,
) -> Result<()> {
    let mut buffer = [0u8; 8192];
    loop {
        let n = pipe.read(&mut buffer).await?;
        if n == 0 {
            return Ok(());
        }
        output.push(&buffer[..n]);
    }
}

async fn bash_with(
    root: &Path,
    script: &str,
    timeout: Duration,
    program: Option<&Path>,
) -> Result<Value> {
    let mut command = Command::new(program.map(Path::to_path_buf).unwrap_or_else(bash_program));
    // Pass the original script as one argument: Bash owns parsing and shell options.
    command
        .args(["--noprofile", "--norc", "-c", script])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .context("start Bash; install Bash or set AUDIT_BASH to its executable")?;
    let group = crate::runner::ProcessGroup(child.id());
    #[cfg(windows)]
    let _job = crate::runner::ProcessJob::attach(&child)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut out = CaptureBuffer::default();
    let mut err = CaptureBuffer::default();
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            capture(stdout, &mut out),
            capture(stderr, &mut err),
            async { child.wait().await.context("wait for Bash") }
        )
    })
    .await;
    drop(group);
    #[cfg(windows)]
    drop(_job);
    let (stdout, out_cut) = out.render();
    let (stderr, err_cut) = err.render();
    let mut report = json!({"exit_code":null,"stdout":stdout,"stderr":stderr,"truncated":out_cut || err_cut,"timed_out":false});
    match result {
        Ok(Ok(((), (), status))) => report["exit_code"] = json!(status.code()),
        result => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            match result {
                Err(_) => {
                    report["timed_out"] = json!(true);
                    report["error"] = json!("Bash exceeded its timeout; partial output is included. Use a smaller test or request more time");
                }
                Ok(Err(error)) => report["error"] = json!(format!("{error:#}")),
                _ => unreachable!(),
            }
        }
    }
    Ok(report)
}

/// No model request or credential access; executes only a fixed diagnostic script.
pub async fn doctor(root: &Path) -> Result<Value> {
    doctor_with(root, None).await
}

pub async fn doctor_with(root: &Path, program: Option<&Path>) -> Result<Value> {
    let root = root
        .canonicalize()
        .context("resolve diagnostic working directory")?;
    let command = r#"printf 'bash=%s\n' "$BASH_VERSION"; for tool in rg git python python3 forge cast node gdb lldb tmux; do if command -v "$tool" >/dev/null 2>&1; then printf '%s=available\n' "$tool"; else printf '%s=missing\n' "$tool"; fi; done"#;
    let result = match bash_with(&root, command, Duration::from_secs(5), program).await {
        Ok(value) => value,
        Err(error) => json!({"error":format!("{error:#}")}),
    };
    let ready = result["exit_code"] == 0 && result.get("error").is_none();
    Ok(
        json!({"ready":ready,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "bash":result,"optional_tools_required":false,
        "note":"Missing optional tools do not disable auditing. AUDIT_BASH can select a Bash executable."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_preserves_unicode_across_chunks_and_retention_boundary() {
        let original = format!("{}雪ending", "x".repeat(OUTPUT_BYTES / 2 - 1));
        let mut buffer = CaptureBuffer::default();
        for chunk in original.as_bytes().chunks(7) {
            buffer.push(chunk);
        }
        assert_eq!(buffer.render(), (original, false));
    }

    #[test]
    fn capture_retains_bounded_head_and_tail() {
        let mut buffer = CaptureBuffer::default();
        for _ in 0..100 {
            buffer.push(&[b'x'; 8192]);
        }
        buffer.push(b"final diagnostic");
        assert_eq!(buffer.head.len() + buffer.tail.len(), OUTPUT_BYTES);
        let (text, cut) = buffer.render();
        assert!(cut);
        assert!(text.contains("output truncated"));
        assert!(text.ends_with("final diagnostic"));
    }
}
