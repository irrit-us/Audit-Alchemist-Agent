//! Model-facing workspace tools. Bash runs with host permissions, not in a sandbox.

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
        tool("bash", "Run Bash in the audit root to explore code (rg/grep/git), build and run local PoCs, or invoke other installed tools. Commands run with host permissions; cwd and shell variables reset each call, files persist. No interactive stdin. Output is capped with an explicit truncation marker; narrow commands when truncated. Use timeout_ms for slow tests (capped by the run deadline).", json!({"command":string(),"timeout_ms":integer()}), json!(["command"])),
        tool("read_file", "Read UTF-8 text with 1-based line labels. Use offset and limit to page through files. Read the finding's source line with this tool before citing it. Maximum file size 1 MiB; output cap 32 KiB. Paths are relative to the audit root.", json!({"path":string(),"offset":integer(),"limit":integer()}), json!(["path"])),
        tool("write_file", "Create or overwrite a UTF-8 file relative to the audit root, including PoCs and test fixtures. Parent directories must exist (use Bash mkdir -p). Read existing files before overwriting. Maximum content 1 MiB.", json!({"path":string(),"content":string()}), json!(["path","content"])),
        tool("edit_file", "Replace exactly one occurrence of old_text with new_text in a UTF-8 file. Read first; on missing or ambiguous matches, re-read and provide a unique exact match. Paths are root-relative.", json!({"path":string(),"old_text":string(),"new_text":string()}), json!(["path","old_text","new_text"])),
        tool("list_files", "List supported source paths under a root-relative directory or file (use . for root). Prunes dependency/build directories. At most 128 files; narrow the target on overflow, or use Bash rg --files for broader exploration.", json!({"target":string()}), json!(["target"])),
        tool("search", "Search source lines for any of 1..64 case-sensitive literal needles in one scan. Returns each line once with path and line. Use . for root. Caps: 128 files, 1 MiB scanned, 200 matches; narrow target on overflow. For regex or other file types use Bash rg/grep.", json!({"target":string(),"needles":{"type":"array","items":string(),"minItems":1,"maxItems":64}}), json!(["target","needles"])),
    ]
}

pub struct WorkspaceTools {
    root: SourceRoot,
    // Only lines actually supplied through initial context/read_file may be cited.
    observed: BTreeMap<String, BTreeSet<u32>>,
}

impl WorkspaceTools {
    pub fn new(root: &Path, initial: &SourceContext) -> Result<Self> {
        let mut observed = BTreeMap::new();
        for source in &initial.sources {
            observed.insert(
                source.path.clone(),
                (1..=source.content.lines().count() as u32).collect(),
            );
        }
        Ok(Self {
            root: SourceRoot::open(root)?,
            observed,
        })
    }

    pub fn validate_finding(&self, finding: &Finding) -> Result<()> {
        ensure!(
            self.observed
                .get(&finding.path)
                .is_some_and(|lines| lines.contains(&finding.line)),
            "finding must cite a source line supplied in the initial context or read_file: {}:{}",
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
        match name {
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
                let timeout = timeout.min(Duration::from_millis(args.timeout_ms.unwrap_or(30_000)));
                bash(self.root.path(), &args.command, timeout).await
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
                let file = self.root.read(&args.path, FILE_BYTES)?;
                let lines: Vec<_> = file.content.lines().collect();
                let offset = args.offset.unwrap_or(1);
                let limit = args.limit.unwrap_or(200);
                ensure!(offset > 0 && limit > 0, "offset and limit must be positive");
                ensure!(
                    offset <= lines.len().max(1),
                    "offset exceeds {} lines",
                    lines.len()
                );
                let mut content = String::new();
                // Include escaped path and envelope overhead, leaving room for
                // the loop's remaining_tool_calls field. Never register a line
                // whose text could be removed by the outer result cap.
                let overhead =
                    serde_json::to_string(&json!({"path":file.path,"total_lines":lines.len()}))?
                        .len()
                        + 256;
                let content_budget = OUTPUT_BYTES.saturating_sub(overhead);
                let mut encoded_bytes = 0;
                let mut count = 0;
                for (index, line) in lines.iter().enumerate().skip(offset - 1).take(limit) {
                    let numbered = format!("{}: {}\n", index + 1, line);
                    let encoded_line_bytes = serde_json::to_string(&numbered)?.len();
                    if encoded_bytes + encoded_line_bytes > content_budget {
                        ensure!(
                            count > 0,
                            "line {} exceeds output cap; inspect it with Bash",
                            index + 1
                        );
                        break;
                    }
                    content.push_str(&numbered);
                    encoded_bytes += encoded_line_bytes;
                    count += 1;
                    self.observed
                        .entry(file.path.clone())
                        .or_default()
                        .insert(index as u32 + 1);
                }
                let next = offset - 1 + count;
                Ok(
                    json!({"path":file.path,"content":content,"total_lines":lines.len(),"next_offset":if next < lines.len() { Some(next + 1) } else { None },"truncated":next < lines.len()}),
                )
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
                self.observed.remove(&self.root.relative(&path)?);
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

async fn capture(mut pipe: impl tokio::io::AsyncRead + Unpin) -> Result<(String, bool)> {
    // Drain both pipes to avoid deadlocks; retain a bounded prefix and tail.
    let mut head = Vec::new();
    let mut tail = std::collections::VecDeque::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let n = pipe.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        for byte in &buffer[..n] {
            if head.len() < OUTPUT_BYTES / 2 {
                head.push(*byte);
            } else {
                if tail.len() == OUTPUT_BYTES / 2 {
                    tail.pop_front();
                }
                tail.push_back(*byte);
            }
        }
    }
    let truncated = total > OUTPUT_BYTES;
    let mut text = String::from_utf8_lossy(&head).into_owned();
    if truncated {
        text.push_str("\n[output truncated; narrow the command]\n");
    }
    text.push_str(&String::from_utf8_lossy(
        &tail.into_iter().collect::<Vec<_>>(),
    ));
    Ok((text, truncated))
}

async fn bash(root: &Path, script: &str, timeout: Duration) -> Result<Value> {
    let mut command = Command::new(bash_program());
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
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(capture(stdout), capture(stderr), async {
            child.wait().await.context("wait for Bash")
        })
    })
    .await;
    drop(group);
    match result {
        Ok(Ok(((stdout, out_cut), (stderr, err_cut), status))) => Ok(
            json!({"exit_code":status.code(),"stdout":stdout,"stderr":stderr,"truncated":out_cut || err_cut}),
        ),
        result => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            match result {
                Err(_) => {
                    bail!("Bash exceeded its timeout; use a smaller test or request more time")
                }
                Ok(Err(error)) => Err(error),
                _ => unreachable!(),
            }
        }
    }
}
