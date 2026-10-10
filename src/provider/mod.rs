//! Model adapters for mainstream LLM API shapes.
//!
//! The adapter runs a bounded tool loop for the selected [`WireApi`], streams each
//! response, normalizes deltas into [`StreamEvent`]s for the console/TUI
//! sink, and returns the same versioned JSON findings as before. Credentials
//! are resolved through [`auth`] and are never logged or written to reports.
//!
//! Supported wires: OpenAI-compatible chat completions, the OpenAI Responses
//! API (also used by the Codex backend), and the Anthropic Messages API.

pub mod auth;
pub mod conversation;
pub mod events;
pub mod prompt;
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
    conversation::{Conversation, EmptyCompletion, Turn, TurnDecoder},
    events::{EventSink, StreamEvent},
    sse::SseLines,
    wire::WireApi,
};

pub use crate::context::{snapshot, Source};

pub use prompt::SYSTEM_PROMPT;
/// Default upper bound on a streamed model response. Reasoning models emit a
/// large SSE event stream, so the default leaves headroom above the assembled text.
const DEFAULT_MAX_STREAM_BYTES: u32 = 8 * 1024 * 1024;

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

/// Provider reasoning-effort hint for chat-completions reasoning models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct LlmOptions {
    #[arg(skip)]
    pub settings: crate::config::AgentSettings,
    #[command(flatten)]
    pub monitoring: crate::monitor::MonitorOptions,
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
    /// Hard byte cap on streamed model output (SSE framing included).
    #[arg(long, default_value_t = DEFAULT_MAX_STREAM_BYTES, value_parser = clap::value_parser!(u32).range(1_048_576..=33_554_432))]
    pub max_stream_bytes: u32,
    /// Optional reasoning-effort hint for reasoning models (chat-completions).
    #[arg(long, value_enum)]
    pub reasoning_effort: Option<ReasoningEffort>,
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
    /// Maximum tool executions across the audit (a batch counts each call).
    #[arg(long, default_value_t = 32, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub max_tool_calls: u32,
    /// Hard byte cap on the serialized model request, including tool history.
    #[arg(long, default_value_t = 2_097_152, value_parser = clap::value_parser!(u32).range(4096..=16_777_216))]
    pub max_context_bytes: u32,
    /// Archive old tool outputs under pressure, or retain history and fail at the cap.
    #[arg(long, value_enum, default_value_t = crate::context::history::ContextPolicy::Prune)]
    pub context_policy: crate::context::history::ContextPolicy,
    /// Complete recent tool turns protected from context pruning.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=32))]
    pub context_keep_turns: u32,
    /// Maximum JSON bytes per model-visible tool result, including truncation metadata.
    #[arg(long, default_value_t = 32768, value_parser = clap::value_parser!(u32).range(1024..=131072))]
    pub max_tool_output_bytes: u32,
    /// Bounded retries when the final report fails JSON or evidence validation.
    /// `0` restores fail-fast behavior; repairs never fabricate a report.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(0..=8))]
    pub max_output_repairs: u32,
}

impl LlmOptions {
    /// Enforce source bounds even for callers supplying an assembled context.
    /// Raw source tokens keep their existing meaning; prompt overhead is
    /// reported separately by `prompt::AuditPrompt::estimated_tokens`.
    pub fn validate_context(&self, context: &Context) -> Result<()> {
        ensure!(
            context.files() <= context::MAX_FILES,
            "audit context must contain at most {} source files",
            context::MAX_FILES
        );
        let bytes = context.sources.iter().try_fold(0usize, |bytes, source| {
            bytes
                .checked_add(source.content.len())
                .context("source byte count overflow")
        })?;
        ensure!(
            bytes <= self.max_source_bytes as usize,
            "source snapshot exceeds byte limit; narrow the target"
        );
        if let Some(max) = self.max_source_tokens {
            ensure!(
                context.estimated_tokens() <= max as usize,
                "estimated context of {} tokens exceeds the {max} token limit; narrow the target",
                context.estimated_tokens()
            );
        }
        Ok(())
    }

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

    pub fn arguments(&self) -> Result<Vec<String>> {
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
            "--max-stream-bytes".into(),
            self.max_stream_bytes.to_string(),
            "--max-attempts".into(),
            self.max_attempts.to_string(),
            "--retry-base-ms".into(),
            self.retry_base_ms.to_string(),
            "--retry-max-ms".into(),
            self.retry_max_ms.to_string(),
            "--max-tool-calls".into(),
            self.max_tool_calls.to_string(),
            "--max-context-bytes".into(),
            self.max_context_bytes.to_string(),
            "--context-policy".into(),
            self.context_policy.as_str().into(),
            "--context-keep-turns".into(),
            self.context_keep_turns.to_string(),
            "--max-tool-output-bytes".into(),
            self.max_tool_output_bytes.to_string(),
            "--max-output-repairs".into(),
            self.max_output_repairs.to_string(),
        ];
        if let Some(tokens) = self.max_source_tokens {
            args.extend(["--max-source-tokens".into(), tokens.to_string()]);
        }
        if let Some(effort) = self.reasoning_effort {
            args.extend(["--reasoning-effort".into(), effort.as_str().into()]);
        }
        if let Some(endpoint) = &self.endpoint {
            args.extend(["--endpoint".into(), endpoint.clone()]);
        }
        if let Some(file) = &self.codex_auth_file {
            args.extend(["--codex-auth-file".into(), file.display().to_string()]);
        }
        if let Some(path) = &self.monitoring.trace_dir {
            // Resolve before evaluation changes the child working directory.
            let absolute = std::path::absolute(path).context("resolve trace directory")?;
            args.extend(["--trace-dir".into(), absolute.display().to_string()]);
        }
        if self.monitoring.debug_trace {
            args.push("--debug-trace".into());
        }
        Ok(args)
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
    sink: &mut dyn EventSink,
    mut build: impl FnMut() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    let mut attempt = 0u32;
    loop {
        let final_attempt = !policy.allows_retry(attempt);
        match build().send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                sink.on_event(&crate::monitor::operation(
                    "http_response",
                    serde_json::json!({"attempt":attempt+1,"status":status}),
                ));
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
                sink.on_event(&crate::monitor::operation("retry", serde_json::json!({
                    "attempt":attempt + 1,"next_attempt":attempt + 2,"delay_ms":wait.as_millis() as u64,
                    "reason":"transient_http","status":status
                })));
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
                sink.on_event(&crate::monitor::operation("retry", serde_json::json!({
                    "attempt":attempt + 1,"next_attempt":attempt + 2,"delay_ms":wait.as_millis() as u64,
                    "reason":"transport"
                })));
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
    let context = context::initial(
        root,
        &request.target,
        &ContextBudget::new(options.max_source_bytes as usize),
    )?;
    audit_with(options, request, &context, root, timeout, &mut ()).await
}

/// Audit one case using an already-assembled context, streaming to `sink`.
pub async fn audit_with(
    options: &LlmOptions,
    request: &Request,
    context: &Context,
    root: &Path,
    timeout: Duration,
    sink: &mut dyn EventSink,
) -> Result<Response> {
    let mut monitor =
        crate::monitor::Monitor::new(&options.monitoring, &options.api_key_env, sink)?;
    monitor.start(
        &options.model,
        options.effective_wire().as_str(),
        timeout.as_millis() as u64,
        &request.case_id,
        &request.target,
    );
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut queue = crate::monitor::QueueSink(sender);
    let began = Instant::now();
    let mut result = {
        let session = audit_session(options, request, context, root, timeout, &mut queue);
        tokio::pin!(session);
        let timer = tokio::time::sleep(timeout);
        tokio::pin!(timer);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(5),
            Duration::from_secs(5),
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                result = &mut session => break result,
                _ = &mut timer => break Err(anyhow::Error::new(AuditTimeout)),
                Some(event) = receiver.recv() => monitor.on_event(&event),
                _ = heartbeat.tick() => monitor.heartbeat(),
            }
        }
    };
    if began.elapsed() >= timeout {
        result = Err(AuditTimeout.into());
    }
    while let Ok(event) = receiver.try_recv() {
        monitor.on_event(&event);
    }
    let outcome = match &result {
        Ok(_) => "success",
        Err(error) if error.is::<AuditTimeout>() => "timeout",
        Err(_) => "error",
    };
    if let Err(error) = &result {
        debug_capture(options, &mut monitor, "error", || format!("{error:#}"));
    }
    monitor.finish(outcome)?;
    result
}

#[derive(Debug)]
pub struct AuditTimeout;
impl std::fmt::Display for AuditTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("audit exceeded wall-clock deadline")
    }
}
impl std::error::Error for AuditTimeout {}

async fn audit_session(
    options: &LlmOptions,
    request: &Request,
    context: &Context,
    root: &Path,
    timeout: Duration,
    sink: &mut dyn EventSink,
) -> Result<Response> {
    options.validate()?;
    ensure!(
        request.schema_version == VERSION,
        "unsupported request version"
    );
    options.validate_context(context)?;
    let prompt = prompt::prepare_with(
        &request.instruction,
        &request.target,
        context,
        &options.settings,
    )?;
    tracing::info!(
        files = context.files(),
        bytes = context.total_bytes(),
        estimated_tokens = context.estimated_tokens(),
        estimated_prompt_tokens = prompt.estimated_tokens(),
        wire = options.effective_wire().as_str(),
        "assembled audit context"
    );
    let deadline = Instant::now() + timeout;
    let client = client(timeout)?;
    let mut archive = crate::context::history::ResultArchive::default();
    let mcp_started = Instant::now();
    let servers: Vec<_> = options
        .settings
        .mcp
        .iter()
        .filter(|(_, c)| c.enabled)
        .map(|(n, _)| n)
        .collect();
    if !servers.is_empty() {
        sink.on_event(&crate::monitor::operation(
            "mcp_start",
            serde_json::json!({"servers":servers}),
        ));
    }
    let mut mcp = crate::mcp::McpTools::connect(&options.settings.mcp, root).await?;
    if !servers.is_empty() {
        sink.on_event(&crate::monitor::operation("mcp_ready", serde_json::json!({"tool_count":mcp.definitions().len(),"elapsed_ms":mcp_started.elapsed().as_millis() as u64})));
    }
    let mut definitions = options.settings.definitions();
    definitions.extend(mcp.definitions());
    let mut conversation = Conversation::with_tools(
        options.effective_wire(),
        &options.model,
        &prompt.system,
        &prompt.user,
        options.max_tokens,
        definitions,
    );
    if let Some(effort) = options.reasoning_effort {
        conversation.set_reasoning_effort(effort.as_str());
    }
    let mut tools =
        crate::tools::WorkspaceTools::configured(root, context, options.settings.clone())?;
    let mut used = 0usize;
    let mut turn_number = 0u32;
    let mut tools_disabled = false;
    let mut repairs = 0u32;
    let mut empty_retries = 0u32;
    let can_archive = matches!(
        options.context_policy,
        crate::context::history::ContextPolicy::Prune
    ) && options.settings.tool_enabled("bash");
    loop {
        if Instant::now() >= deadline {
            return Err(AuditTimeout.into());
        }
        // Force a final report turn once the tool budget is spent. The prompt
        // already says to stop calling tools at zero; removing the tool
        // definitions makes that instruction enforceable rather than an error.
        if used >= options.max_tool_calls as usize && !tools_disabled {
            conversation.disable_tools();
            tools_disabled = true;
            sink.on_event(&crate::monitor::operation(
                "final_turn_forced",
                serde_json::json!({"used_tool_calls":used,"max_tool_calls":options.max_tool_calls}),
            ));
        }
        let request_bytes = if can_archive {
            let reduction = conversation.reduce_context(
                options.max_context_bytes as usize,
                options.context_keep_turns,
                &mut archive,
            )?;
            if reduction.pruned_results > 0 {
                sink.on_event(&crate::monitor::operation(
                    "context_pruned",
                    serde_json::to_value(&reduction)?,
                ));
            }
            reduction.after_bytes
        } else {
            crate::context::history::serialized_bytes(&conversation.body)?
        };
        ensure!(
            request_bytes <= options.max_context_bytes as usize,
            "conversation exceeds --max-context-bytes; protected or retained context does not fit; narrow the target or raise the limit"
        );
        turn_number += 1;
        sink.on_event(&crate::monitor::operation(
            "turn_start",
            serde_json::json!({
                "turn":turn_number,"request_bytes":request_bytes,
                "remaining_tool_calls":options.max_tool_calls as usize - used,
                "estimated_request_tokens":request_bytes.div_ceil(4),
                "context_limit_bytes":options.max_context_bytes
            }),
        ));
        debug_capture(options, sink, "request", || conversation.body.to_string());
        let started = Instant::now();
        let turn = match complete(options, &client, &conversation.body, deadline, sink).await {
            Ok(turn) => turn,
            // A provider that completes a stream with no text or tool calls is a
            // transient response defect; retry the unchanged request before
            // spending the run. Tool mutations are still never replayed.
            Err(error) if error.is::<EmptyCompletion>() && empty_retries < options.max_attempts => {
                empty_retries += 1;
                sink.on_event(&crate::monitor::operation(
                    "empty_completion_retry",
                    serde_json::json!({"attempt":empty_retries,"max_attempts":options.max_attempts}),
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        sink.on_event(&StreamEvent::Usage(turn.usage));
        sink.on_event(&crate::monitor::operation(
            "turn_end",
            serde_json::json!({
                "turn":turn_number,"elapsed_ms":started.elapsed().as_millis() as u64,
                "text_bytes":turn.text.len(),"tool_calls":turn.calls.len(),"usage":turn.usage
            }),
        ));
        debug_capture(options, sink, "response", || turn.text.clone());
        if turn.calls.is_empty() {
            match validate_final(&turn.text, &tools) {
                Ok(findings) => {
                    mcp.shutdown().await;
                    sink.on_event(&StreamEvent::Done);
                    return Ok(findings);
                }
                Err(error) if repairs < options.max_output_repairs => {
                    repairs += 1;
                    conversation.append_feedback(&turn, &repair_message(&error))?;
                    sink.on_event(&crate::monitor::operation(
                        "output_repair",
                        serde_json::json!({
                            "attempt":repairs,
                            "max_repairs":options.max_output_repairs,
                            "error":format!("{error:#}")
                        }),
                    ));
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        ensure!(
            used + turn.calls.len() <= options.max_tool_calls as usize,
            "agent exhausted --max-tool-calls without a final report"
        );
        let mut results = Vec::new();
        // Preserve call order: a later call may run a PoC created by an earlier one.
        for call in &turn.calls {
            if Instant::now() >= deadline {
                return Err(AuditTimeout.into());
            }
            used += 1;
            tracing::info!(tool = %call.name, call = used, "executing audit tool");
            sink.on_event(&StreamEvent::ToolStart {
                name: call.name.clone(),
                call_id: call.id.clone(),
            });
            debug_capture(options, sink, "tool_arguments", || call.arguments.clone());
            let tool_started = Instant::now();
            let mut result = if mcp.contains(&call.name) {
                mcp.execute(&call.name, &call.arguments, remaining(deadline))
                    .await
            } else {
                tools
                    .execute(&call.name, &call.arguments, remaining(deadline))
                    .await
            };
            result["remaining_tool_calls"] =
                serde_json::json!(options.max_tool_calls as usize - used);
            sink.on_event(&StreamEvent::ToolEnd {
                name: call.name.clone(),
                call_id: call.id.clone(),
                is_error: result.get("error").is_some()
                    || result
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .is_some_and(|code| code != 0),
                elapsed_ms: tool_started.elapsed().as_millis() as u64,
            });
            sink.on_event(&crate::monitor::operation(
                "tool_result",
                serde_json::json!({
                    "call_id":call.id,"name":call.name,"exit_code":result.get("exit_code"),
                    "timed_out":result.get("timed_out"),"truncated":result.get("truncated"),
                    "result_bytes":crate::context::history::serialized_bytes(&result)?
                }),
            ));
            debug_capture(options, sink, "tool_result", || result.to_string());
            let original = serde_json::to_string(&result)?;
            let saved = if can_archive && original.len() > options.max_tool_output_bytes as usize {
                archive.save(&original)?
            } else {
                None
            };
            let projected = crate::context::history::project_result(
                &original,
                options.max_tool_output_bytes as usize,
                saved.as_deref(),
            )?;
            if projected.len() != original.len() {
                sink.on_event(&crate::monitor::operation("tool_output_projected", serde_json::json!({"call_id":call.id,"original_bytes":original.len(),"projected_bytes":projected.len(),"archived":saved.is_some()})));
            }
            results.push(projected);
        }
        conversation.append(&turn, &results)?;
    }
}

/// Parse and validate the final report. Errors are returned to the model as
/// bounded repair feedback; the run still fails if the budget is exhausted.
fn validate_final(text: &str, tools: &crate::tools::WorkspaceTools) -> Result<Response> {
    let findings: Response = serde_json::from_str(text)
        .context("model must return a JSON response without Markdown fences")?;
    findings.validate()?;
    for finding in &findings.findings {
        tools.validate_finding(finding)?;
    }
    Ok(findings)
}

fn repair_message(error: &anyhow::Error) -> String {
    format!(
        "Your previous response was rejected during validation: {error:#}. \
         Return exactly one JSON object matching the required schema, with no prose \
         or Markdown fences. Keep supported findings and correct only the rejected fields."
    )
}

fn debug_capture(
    options: &LlmOptions,
    sink: &mut dyn EventSink,
    stage: &str,
    content: impl FnOnce() -> String,
) {
    if options.monitoring.debug_trace {
        let key = std::env::var(&options.api_key_env)
            .ok()
            .filter(|s| !s.is_empty());
        sink.on_event(&StreamEvent::Debug {
            stage: stage.into(),
            content: crate::monitor::debug_payload(&content(), key.as_deref()),
        });
    }
}

/// Send one streaming request and return the accumulated answer text.
async fn complete(
    options: &LlmOptions,
    client: &reqwest::Client,
    body: &Value,
    deadline: Instant,
    sink: &mut dyn EventSink,
) -> Result<Turn> {
    let wire = options.effective_wire();
    let policy = retry_policy(options);

    let turn = match options.auth {
        AuthKind::ApiKey => {
            let url = options
                .endpoint
                .as_deref()
                .context("--endpoint is required with --auth api-key")?;
            let key = std::env::var(&options.api_key_env)?;
            let response = send_retrying(policy, deadline, sink, || {
                api_key_request(client, wire, url, &key, body, deadline)
            })
            .await?;
            ensure!(
                response.status().is_success(),
                "LLM API returned HTTP {}",
                response.status().as_u16()
            );
            read_stream(response, wire, sink, options.max_stream_bytes as usize).await?
        }
        AuthKind::Codex => {
            let url = options.endpoint.clone().unwrap_or_else(|| {
                format!("{}/responses", options.codex_base_url.trim_end_matches('/'))
            });
            let mode = auth::AuthMode::Codex {
                auth_file: options.codex_auth_file.clone(),
            };
            let credentials = auth::resolve(&mode).await?;
            let response = send_retrying(policy, deadline, sink, || {
                codex_request(client, &url, &credentials, body, deadline)
            })
            .await?;
            let response = if response.status().as_u16() == 401 {
                // The token can expire between our proactive check and the
                // request; refresh once and retry a single time. This is
                // authentication recovery, not a retry of a failed model call.
                let fresh = auth::resolve_fresh(&mode).await?;
                send_retrying(policy, deadline, sink, || {
                    codex_request(client, &url, &fresh, body, deadline)
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
            read_stream(
                response,
                WireApi::Responses,
                sink,
                options.max_stream_bytes as usize,
            )
            .await?
        }
    };

    let usage = turn.usage;
    if !usage.is_empty() {
        tracing::info!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            total_tokens = usage.total_tokens,
            reasoning_tokens = usage.reasoning_tokens,
            "model usage"
        );
    }
    Ok(turn)
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
    max_bytes: usize,
) -> Result<Turn> {
    let mut lines = SseLines::new();
    let mut stream = TurnDecoder::new(wire);
    let mut total = 0usize;
    let started = Instant::now();
    while let Some(chunk) = response.chunk().await.context("read model stream")? {
        if total == 0 && !chunk.is_empty() {
            sink.on_event(&crate::monitor::operation(
                "stream_start",
                serde_json::json!({"after_headers_ms":started.elapsed().as_millis() as u64}),
            ));
        }
        total += chunk.len();
        ensure!(total <= max_bytes, "model stream exceeds {max_bytes} bytes");
        for data in lines.push(&chunk)? {
            stream.handle_data(&data, sink)?;
        }
    }
    for data in lines.finish()? {
        stream.handle_data(&data, sink)?;
    }
    sink.on_event(&crate::monitor::operation(
        "stream_end",
        serde_json::json!({"bytes":total,"elapsed_ms":started.elapsed().as_millis() as u64}),
    ));
    stream.finish()
}

/// A best-effort unique identifier for the `session_id` header.
pub(crate) fn session_id() -> String {
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
    fn evaluation_forwards_an_absolute_trace_path_and_debug_flag() {
        use clap::Parser;
        #[derive(Parser)]
        struct Options {
            #[command(flatten)]
            llm: LlmOptions,
        }
        let options = Options::parse_from([
            "test",
            "--model",
            "fixture",
            "--trace-dir",
            "relative-traces",
            "--debug-trace",
            "--context-policy",
            "fail",
            "--context-keep-turns",
            "5",
            "--max-tool-output-bytes",
            "2048",
        ]);
        let args = options.llm.arguments().unwrap();
        let path = args
            .windows(2)
            .find(|pair| pair[0] == "--trace-dir")
            .unwrap();
        assert_eq!(
            Path::new(&path[1]),
            std::path::absolute("relative-traces").unwrap()
        );
        assert!(args.iter().any(|arg| arg == "--debug-trace"));
        for (flag, value) in [
            ("--context-policy", "fail"),
            ("--context-keep-turns", "5"),
            ("--max-tool-output-bytes", "2048"),
        ] {
            assert!(args
                .windows(2)
                .any(|pair| pair[0] == flag && pair[1] == value));
        }
    }

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
