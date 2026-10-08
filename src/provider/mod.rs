//! Model adapters for mainstream LLM API shapes.
//!
//! The adapter builds a request for the selected [`WireApi`], streams the
//! response, normalizes deltas into [`StreamEvent`]s for the console/TUI
//! sink, and returns the same versioned JSON findings as before. Credentials
//! are resolved through [`auth`] and are never logged or written to reports.
//!
//! Supported wires: OpenAI-compatible chat completions, the OpenAI Responses
//! API (also used by the Codex backend), and the Anthropic Messages API.

pub mod auth;
pub mod events;
pub mod retry;
pub mod sse;
pub mod wire;

use crate::{
    context::{self, Context, ContextBudget},
    protocol::{Request, Response, VERSION},
};
use anyhow::{ensure, Context as _, Result};
use clap::Args;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use self::{
    events::{EventSink, StreamEvent},
    sse::SseLines,
    wire::{WireApi, WireStream},
};

pub use crate::context::{snapshot, Source};

pub const SYSTEM_PROMPT: &str = include_str!("../../prompts/audit.txt");
/// Upper bound on any single HTTP response body we will buffer.
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Which credential source to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AuthKind {
    /// Bearer token from an environment variable.
    ApiKey,
    /// Reuse a Codex CLI ChatGPT login against the Codex responses backend.
    Codex,
}

impl AuthKind {
    /// Stable CLI spelling, matching `clap`'s kebab-case value.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthKind::ApiKey => "api-key",
            AuthKind::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct LlmOptions {
    /// Full provider URL: a chat-completions, Responses, or Messages endpoint.
    /// Required with `--auth api-key`; optional override for Codex.
    #[arg(long)]
    pub endpoint: Option<String>,
    #[arg(long)]
    pub model: String,
    /// Wire format of the endpoint when `--auth api-key` is used.
    #[arg(long, value_enum, default_value_t = WireApi::ChatCompletions)]
    pub wire_api: WireApi,
    /// Name of the environment variable holding the bearer token.
    #[arg(long, default_value = "AUDIT_API_KEY")]
    pub api_key_env: String,
    /// Credential source.
    #[arg(long, value_enum, default_value_t = AuthKind::ApiKey)]
    pub auth: AuthKind,
    /// Path to the Codex auth.json. Defaults to `$CODEX_HOME/auth.json`.
    #[arg(long)]
    pub codex_auth_file: Option<PathBuf>,
    /// Base URL of the Codex responses backend when `--auth codex` is used.
    #[arg(long, default_value = auth::CODEX_BASE_URL)]
    pub codex_base_url: String,
    #[arg(long, default_value_t = 262_144, value_parser = clap::value_parser!(u32).range(1..=1_048_576))]
    pub max_source_bytes: u32,
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u32).range(1..=32_768))]
    pub max_tokens: u32,
    /// Optional cap on the estimated context tokens; disabled when unset.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=8_388_608))]
    pub max_source_tokens: Option<u32>,
    /// Total attempts for transient LLM transport failures (1 disables retries).
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=8))]
    pub max_attempts: u32,
    /// Base delay for exponential retry backoff.
    #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u64).range(0..=60_000))]
    pub retry_base_ms: u64,
    /// Ceiling for retry backoff and server `Retry-After` hints.
    #[arg(long, default_value_t = 8_000, value_parser = clap::value_parser!(u64).range(0..=600_000))]
    pub retry_max_ms: u64,
}

impl LlmOptions {
    /// The wire format actually used: Codex always speaks the Responses API.
    pub fn effective_wire(&self) -> WireApi {
        match self.auth {
            AuthKind::Codex => WireApi::Responses,
            AuthKind::ApiKey => self.wire_api,
        }
    }

    /// Validate configuration and, where cheap, credential availability.
    ///
    /// This never performs network I/O; token refresh happens later in the
    /// agent process so a stale token does not block argument construction.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.model.trim().is_empty(), "model is empty");
        match self.auth {
            AuthKind::ApiKey => {
                let endpoint = self
                    .endpoint
                    .as_deref()
                    .context("--endpoint is required with --auth api-key")?;
                let _ = parse_endpoint(endpoint)?;
                let key = std::env::var(&self.api_key_env).with_context(|| {
                    format!("missing API key environment variable {}", self.api_key_env)
                })?;
                ensure!(!key.trim().is_empty(), "API key is empty");
            }
            AuthKind::Codex => {
                match self.endpoint.as_deref() {
                    Some(endpoint) => {
                        let _ = parse_endpoint(endpoint)?;
                    }
                    None => {
                        let _ = parse_endpoint(&self.codex_base_url)?;
                    }
                }
                if let Some(file) = &self.codex_auth_file {
                    // The agent runs with the source root as its working
                    // directory, so a relative path would resolve differently
                    // in the parent and child processes.
                    ensure!(
                        file.is_absolute(),
                        "--codex-auth-file must be an absolute path"
                    );
                }
                let file = auth::auth_path(self.codex_auth_file.clone())?;
                ensure!(
                    file.is_file(),
                    "Codex auth file not found at {}; run `codex login` to sign in with ChatGPT",
                    file.display()
                );
            }
        }
        Ok(())
    }

    pub fn arguments(&self) -> Vec<String> {
        let mut args = vec![
            "agent".into(),
            "--model".into(),
            self.model.clone(),
            "--wire-api".into(),
            self.wire_api.as_str().into(),
            "--api-key-env".into(),
            self.api_key_env.clone(),
            "--auth".into(),
            self.auth.as_str().into(),
            "--codex-base-url".into(),
            self.codex_base_url.clone(),
            "--max-source-bytes".into(),
            self.max_source_bytes.to_string(),
            "--max-tokens".into(),
            self.max_tokens.to_string(),
            "--max-attempts".into(),
            self.max_attempts.to_string(),
            "--retry-base-ms".into(),
            self.retry_base_ms.to_string(),
            "--retry-max-ms".into(),
            self.retry_max_ms.to_string(),
        ];
        if let Some(tokens) = self.max_source_tokens {
            args.extend(["--max-source-tokens".into(), tokens.to_string()]);
        }
        if let Some(endpoint) = &self.endpoint {
            args.extend(["--endpoint".into(), endpoint.clone()]);
        }
        if let Some(file) = &self.codex_auth_file {
            args.extend(["--codex-auth-file".into(), file.display().to_string()]);
        }
        args
    }
}

/// Validate an endpoint URL: no embedded credentials, query, or fragment.
fn parse_endpoint(endpoint: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(endpoint).context("invalid endpoint URL")?;
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
    Ok(url)
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build HTTP client")
}

fn retry_policy(options: &LlmOptions) -> retry::RetryPolicy {
    retry::RetryPolicy::from_millis(
        options.max_attempts,
        options.retry_base_ms,
        options.retry_max_ms,
    )
}

/// Time left before the overall deadline, never zero for a request timeout.
fn remaining(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1))
}

fn has_time(deadline: Instant, wait: Duration) -> bool {
    Instant::now()
        .checked_add(wait)
        .is_some_and(|wake| wake < deadline)
}

fn retryable_transport(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(retry::parse_retry_after)
}

/// Send a request, retrying only transient transport failures within `deadline`.
///
/// Any HTTP response is returned as-is so the caller can inspect the status
/// (for example to re-authenticate on 401); only connection, TLS, and timeout
/// errors become `Err`. Retries stop before the overall deadline rather than
/// sleeping past it, and a final attempt always returns its result unchanged.
async fn send_retrying(
    policy: retry::RetryPolicy,
    deadline: Instant,
    mut build: impl FnMut() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    let mut attempt = 0u32;
    loop {
        let final_attempt = !policy.allows_retry(attempt);
        match build().send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if final_attempt || !retry::retryable_status(status) {
                    return Ok(response);
                }
                let wait = retry::backoff(
                    &policy,
                    attempt,
                    retry_after(response.headers()),
                    retry::seed(),
                );
                if !has_time(deadline, wait) {
                    return Ok(response);
                }
                tracing::warn!(
                    attempt = attempt + 1,
                    status,
                    delay_ms = wait.as_millis() as u64,
                    "retrying transient LLM API response"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(error) => {
                if final_attempt || !retryable_transport(&error) {
                    return Err(error)
                        .context("LLM API request failed (connection, TLS, or deadline)");
                }
                let wait = retry::backoff(&policy, attempt, None, retry::seed());
                if !has_time(deadline, wait) {
                    return Err(error)
                        .context("LLM API request failed (connection, TLS, or deadline)");
                }
                tracing::warn!(
                    attempt = attempt + 1,
                    delay_ms = wait.as_millis() as u64,
                    "retrying LLM API connection failure"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
        }
    }
}

/// Audit one case, building context from `root`. Used by the `agent` adapter.
pub async fn audit(
    options: &LlmOptions,
    request: &Request,
    root: &Path,
    timeout: Duration,
) -> Result<Response> {
    options.validate()?;
    ensure!(
        !request.instruction.trim().is_empty() && request.instruction.len() <= 65_536,
        "invalid instruction"
    );
    let context = context::build(
        root,
        &request.target,
        &ContextBudget::new(options.max_source_bytes as usize),
    )?;
    if let Some(max) = options.max_source_tokens {
        ensure!(
            context.estimated_tokens() <= max as usize,
            "estimated context of {} tokens exceeds the {max} token limit; narrow the target",
            context.estimated_tokens()
        );
    }
    audit_with(options, request, &context, timeout, &mut ()).await
}

/// Audit one case using an already-assembled context, streaming to `sink`.
pub async fn audit_with(
    options: &LlmOptions,
    request: &Request,
    context: &Context,
    timeout: Duration,
    sink: &mut dyn EventSink,
) -> Result<Response> {
    options.validate()?;
    ensure!(
        request.schema_version == VERSION,
        "unsupported request version"
    );
    ensure!(
        !request.instruction.trim().is_empty() && request.instruction.len() <= 65_536,
        "invalid instruction"
    );
    tracing::info!(
        files = context.files(),
        bytes = context.total_bytes(),
        estimated_tokens = context.estimated_tokens(),
        wire = options.effective_wire().as_str(),
        "assembled audit context"
    );
    let user = serde_json::to_string(
        &serde_json::json!({"instruction": request.instruction, "sources": &context.sources}),
    )?;
    let text = complete(options, &user, timeout, sink).await?;
    let findings: Response = serde_json::from_str(&text)
        .context("model must return a JSON response without Markdown fences")?;
    findings.validate()?;
    for finding in &findings.findings {
        context.validate_finding(finding)?;
    }
    sink.on_event(&StreamEvent::Done);
    Ok(findings)
}

/// Send one streaming request and return the accumulated answer text.
async fn complete(
    options: &LlmOptions,
    user: &str,
    timeout: Duration,
    sink: &mut dyn EventSink,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let client = client(timeout)?;
    let wire = options.effective_wire();
    let body = wire::request_body(
        wire,
        &options.model,
        SYSTEM_PROMPT,
        user,
        options.max_tokens,
    );
    let policy = retry_policy(options);

    let (text, usage) = match options.auth {
        AuthKind::ApiKey => {
            let url = options
                .endpoint
                .as_deref()
                .context("--endpoint is required with --auth api-key")?;
            let key = std::env::var(&options.api_key_env)?;
            let response = send_retrying(policy, deadline, || {
                api_key_request(&client, wire, url, &key, &body, deadline)
            })
            .await?;
            ensure!(
                response.status().is_success(),
                "LLM API returned HTTP {}",
                response.status().as_u16()
            );
            read_stream(response, wire, sink).await?
        }
        AuthKind::Codex => {
            let url = format!("{}/responses", options.codex_base_url.trim_end_matches('/'));
            let mode = auth::AuthMode::Codex {
                auth_file: options.codex_auth_file.clone(),
            };
            let credentials = auth::resolve(&mode).await?;
            let response = send_retrying(policy, deadline, || {
                codex_request(&client, &url, &credentials, &body, deadline)
            })
            .await?;
            let response = if response.status().as_u16() == 401 {
                // The token can expire between our proactive check and the
                // request; refresh once and retry a single time. This is
                // authentication recovery, not a retry of a failed model call.
                let fresh = auth::resolve_fresh(&mode).await?;
                send_retrying(policy, deadline, || {
                    codex_request(&client, &url, &fresh, &body, deadline)
                })
                .await?
            } else {
                response
            };
            ensure!(
                response.status().is_success(),
                "Codex responses returned HTTP {}",
                response.status().as_u16()
            );
            read_stream(response, WireApi::Responses, sink).await?
        }
    };

    if !usage.is_empty() {
        tracing::info!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            total_tokens = usage.total_tokens,
            reasoning_tokens = usage.reasoning_tokens,
            "model usage"
        );
    }
    Ok(text)
}

fn api_key_request(
    client: &reqwest::Client,
    wire: WireApi,
    url: &str,
    key: &str,
    body: &Value,
    deadline: Instant,
) -> reqwest::RequestBuilder {
    let request = client
        .post(url)
        .timeout(remaining(deadline))
        .header("Accept", "text/event-stream");
    let request = match wire {
        WireApi::Anthropic => request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        WireApi::ChatCompletions | WireApi::Responses => request.bearer_auth(key),
    };
    request.json(body)
}

fn codex_request(
    client: &reqwest::Client,
    url: &str,
    credentials: &auth::Credentials,
    body: &Value,
    deadline: Instant,
) -> reqwest::RequestBuilder {
    let mut request = client
        .post(url)
        .timeout(remaining(deadline))
        .header("Accept", "text/event-stream")
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs")
        .header("session_id", session_id())
        .bearer_auth(&credentials.bearer)
        .json(body);
    if let Some(account_id) = &credentials.account_id {
        request = request.header("chatgpt-account-id", account_id);
    }
    request
}

/// Read a streaming response, emitting normalized events into `sink`.
async fn read_stream(
    mut response: reqwest::Response,
    wire: WireApi,
    sink: &mut dyn EventSink,
) -> Result<(String, events::Usage)> {
    let mut lines = SseLines::new();
    let mut stream = WireStream::new(wire);
    let mut total = 0usize;
    while let Some(chunk) = response.chunk().await.context("read model stream")? {
        total += chunk.len();
        ensure!(
            total <= MAX_RESPONSE_BYTES,
            "model stream exceeds {MAX_RESPONSE_BYTES} bytes"
        );
        for data in lines.push(&chunk)? {
            stream.handle_data(&data, sink)?;
        }
    }
    for data in lines.finish()? {
        stream.handle_data(&data, sink)?;
    }
    stream.finish()
}

/// A best-effort unique identifier for the `session_id` header.
fn session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:032x}{:08x}{:08x}", nanos, std::process::id(), counter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_rejects_credentials_and_plain_http() {
        assert!(parse_endpoint("https://api.example.com/v1").is_ok());
        assert!(parse_endpoint("http://127.0.0.1:8080/v1").is_ok());
        assert!(parse_endpoint("http://api.example.com/v1").is_err());
        assert!(parse_endpoint("https://user:pass@api.example.com/v1").is_err());
        assert!(parse_endpoint("https://api.example.com/v1?x=1").is_err());
    }

    #[test]
    fn auth_kind_and_wire_spellings_match_clap() {
        assert_eq!(AuthKind::ApiKey.as_str(), "api-key");
        assert_eq!(AuthKind::Codex.as_str(), "codex");
        assert_eq!(WireApi::ChatCompletions.as_str(), "chat-completions");
        assert_eq!(WireApi::Responses.as_str(), "responses");
        assert_eq!(WireApi::Anthropic.as_str(), "anthropic");
    }
}
