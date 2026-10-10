use crate::protocol::{Finding, Request, Response};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub root: PathBuf,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Timeout,
    SpawnError,
    IoError,
    OutputLimit,
    NonzeroExit,
    InvalidResponse,
    ProviderError,
}

impl Outcome {
    /// Stable, lowercase label for progress logs and reports.
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Timeout => "timeout",
            Outcome::SpawnError => "spawn_error",
            Outcome::IoError => "io_error",
            Outcome::OutputLimit => "output_limit",
            Outcome::NonzeroExit => "nonzero_exit",
            Outcome::InvalidResponse => "invalid_response",
            Outcome::ProviderError => "provider_error",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub case_id: String,
    pub outcome: Outcome,
    pub elapsed_ms: u64,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub findings: Vec<Finding>,
}

#[derive(Debug)]
struct OutputLimit;
impl std::fmt::Display for OutputLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent output exceeded byte limit")
    }
}
impl std::error::Error for OutputLimit {}

/// Bounded, lossy tail of a failed child's stderr for operational diagnostics.
fn stderr_tail(bytes: &[u8]) -> String {
    const TAIL: usize = 2000;
    let slice = if bytes.len() > TAIL {
        &bytes[bytes.len() - TAIL..]
    } else {
        bytes
    };
    String::from_utf8_lossy(slice).trim().to_string()
}

async fn bounded_read(mut reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut reader)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        bail!(OutputLimit);
    }
    Ok(bytes)
}

/// Kill ordinary descendants on drop. Process groups/jobs are not a sandbox.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct ProcessGroup(pub(crate) Option<u32>);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            // SAFETY: negative PID addresses only the new process group created for this run.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) struct ProcessJob(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProcessJob {
    pub(crate) fn attach(child: &tokio::process::Child) -> Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: create an unnamed owned job; initialized info has the required
        // size and the child's borrowed process handle remains valid here.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error()).context("create process job");
            }
            let job = Self(std::os::windows::io::OwnedHandle::from_raw_handle(handle));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error()).context("configure process job");
            }
            let process = child.raw_handle().context("child process already exited")?;
            if AssignProcessToJobObject(job.0.as_raw_handle(), process) == 0 {
                return Err(std::io::Error::last_os_error()).context("attach process job");
            }
            Ok(job)
        }
    }
}

pub async fn run(config: &RunConfig, request: Request) -> RunResult {
    let start = Instant::now();
    let mut result = RunResult {
        case_id: request.case_id.clone(),
        outcome: Outcome::SpawnError,
        elapsed_ms: 0,
        exit_code: None,
        error: None,
        findings: vec![],
    };
    tracing::info!(case_id = %request.case_id, "starting agent");
    let mut command = Command::new(&config.executable);
    command
        .args(&config.args)
        .current_dir(&config.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            result.error = Some(error.to_string());
            result.elapsed_ms = start.elapsed().as_millis() as u64;
            return result;
        }
    };
    let group = ProcessGroup(child.id());
    #[cfg(windows)]
    let _job = match ProcessJob::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            result.error = Some(format!("{error:#}"));
            result.elapsed_ms = start.elapsed().as_millis() as u64;
            return result;
        }
    };
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let exchange = async {
        let input = async {
            let mut bytes = serde_json::to_vec(&request)?;
            bytes.push(b'\n');
            stdin.write_all(&bytes).await.context("write request")?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, anyhow::Error>(())
        };
        let (_, stdout, stderr, status) = tokio::try_join!(
            input,
            bounded_read(stdout, config.max_output_bytes),
            bounded_read(stderr, config.max_output_bytes),
            async { child.wait().await.context("wait for agent") }
        )?;
        Ok::<_, anyhow::Error>((stdout, stderr, status))
    };
    match tokio::time::timeout(config.timeout, exchange).await {
        Err(_) => {
            result.outcome = Outcome::Timeout;
            result.error = Some("agent exceeded wall-clock deadline".into());
        }
        Ok(Err(error)) => {
            result.outcome = if error.is::<OutputLimit>() {
                Outcome::OutputLimit
            } else {
                Outcome::IoError
            };
            result.error = Some(format!("{error:#}"));
        }
        Ok(Ok((stdout, stderr, status))) => {
            result.exit_code = status.code();
            if !status.success() {
                result.outcome = Outcome::NonzeroExit;
                result.error = Some(format!("agent exited with {status}"));
                let detail = stderr_tail(&stderr);
                if !detail.is_empty() {
                    // Operational diagnostics stay on stderr; the report stays
                    // generic so a child error cannot leak provider details.
                    tracing::warn!(case_id = %request.case_id, "agent stderr: {detail}");
                }
            } else {
                let parsed = serde_json::from_slice::<Response>(&stdout)
                    .context("parse agent response")
                    .and_then(|response| {
                        response.validate()?;
                        Ok(response)
                    });
                match parsed {
                    Ok(response) => {
                        result.outcome = Outcome::Success;
                        result.findings = response.findings;
                    }
                    Err(error) => {
                        result.outcome = Outcome::InvalidResponse;
                        result.error = Some(format!("{error:#}"));
                    }
                }
            }
        }
    }
    drop(group);
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result.elapsed_ms = start.elapsed().as_millis() as u64;
    tracing::info!(case_id = %result.case_id, outcome = ?result.outcome, elapsed_ms = result.elapsed_ms, "agent finished");
    result
}
