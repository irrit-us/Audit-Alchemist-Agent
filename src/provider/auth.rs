//! Credential resolution for the audit adapters.
//!
//! Two sources are supported:
//!
//! - an API key read from an environment variable (the original behavior);
//! - **Sign In With ChatGPT** (SIWC): reuse the login that the OpenAI Codex CLI
//!   already wrote to `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`).
//!   The short-lived access token is refreshed through the public OAuth token
//!   endpoint when it is at or near expiry.
//!
//! The on-disk format and endpoints follow the publicly documented Codex
//! credential layout: a top-level `OPENAI_API_KEY`, a `tokens` object with
//! `id_token`, `access_token`, `refresh_token`, and `account_id`, and a
//! `last_refresh` timestamp. The account identifier is read either from
//! `tokens.account_id` or from the `https://api.openai.com/auth` JWT claim
//! (`chatgpt_account_id`).
//!
//! Tokens are never logged or written into reports. Refreshes are serialized
//! with a lock file and persisted atomically so concurrent evaluation workers
//! sharing one login do not corrupt the credential store. Using a ChatGPT
//! subscription outside the official client remains the operator's
//! responsibility; see the project README.

mod token;

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use token::{credentials_from, update_tokens, usable, TokenResponse};

pub use token::Credentials;

/// Public Codex OAuth client identifier.
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Public Codex OAuth token endpoint.
pub const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Default base URL of the ChatGPT Codex responses backend.
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// Give up waiting for a contended credential lock after this long.
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// Consider a lock abandoned after this long.
const LOCK_STALE: Duration = Duration::from_secs(60);

/// Where the bearer credential comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    /// Read a bearer token from the named environment variable.
    ApiKey { env: String },
    /// Reuse (and refresh) an existing Codex CLI ChatGPT login.
    Codex { auth_file: Option<PathBuf> },
}

/// Resolve credentials, reusing a valid cached token when possible.
pub async fn resolve(mode: &AuthMode) -> Result<Credentials> {
    match mode {
        AuthMode::ApiKey { env } => api_key(env),
        AuthMode::Codex { auth_file } => codex_credentials(auth_file.clone(), false).await,
    }
}

/// Resolve credentials, forcing a token refresh for the Codex mode.
pub async fn resolve_fresh(mode: &AuthMode) -> Result<Credentials> {
    match mode {
        AuthMode::ApiKey { env } => api_key(env),
        AuthMode::Codex { auth_file } => codex_credentials(auth_file.clone(), true).await,
    }
}

fn api_key(name: &str) -> Result<Credentials> {
    let key =
        env::var(name).with_context(|| format!("missing API key environment variable {name}"))?;
    ensure!(!key.trim().is_empty(), "API key is empty");
    Ok(Credentials {
        bearer: key,
        account_id: None,
    })
}

/// Locate the Codex auth file, honoring `CODEX_HOME` and per-user home.
pub fn auth_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("auth.json"));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .context("cannot locate home directory for Codex auth.json")?;
    Ok(PathBuf::from(home).join(".codex").join("auth.json"))
}

async fn codex_credentials(path: Option<PathBuf>, force: bool) -> Result<Credentials> {
    let path = auth_path(path)?;
    let _lock = FileLock::acquire(&path).await?;
    let mut value = read_auth(&path)?;

    if !force {
        if let Some(credentials) = usable(&value) {
            return Ok(credentials);
        }
    }

    let refresh_token = value
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .context("Codex login has no refresh token; run `codex login` to sign in with ChatGPT")?
        .to_owned();

    let refreshed = refresh_access_token(&refresh_token).await?;
    update_tokens(&mut value, &refreshed);
    write_auth(&path, &value)?;
    credentials_from(&value).context("refreshed login lacks a usable access token")
}

async fn refresh_access_token(refresh_token: &str) -> Result<TokenResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build OAuth client")?;
    let response = client
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .context("refresh ChatGPT token (connection failed)")?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "ChatGPT token refresh returned HTTP {}; run `codex login` to sign in again",
            status.as_u16()
        );
    }
    response
        .json::<TokenResponse>()
        .await
        .context("parse ChatGPT token refresh response")
}

fn read_auth(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| {
        format!(
            "read Codex auth file {}; run `codex login` to sign in with ChatGPT",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).context("parse Codex auth.json")
}

/// Write the auth file through a sibling temp file and rename.
fn write_auth(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().context("auth file has no parent directory")?;
    let name = path
        .file_name()
        .context("auth file has no name")?
        .to_string_lossy()
        .into_owned();
    let temp = parent.join(format!("{name}.tmp.{}", std::process::id()));
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .with_context(|| format!("create {}", temp.display()))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);

    if let Err(first) = fs::rename(&temp, path) {
        // Windows does not replace an existing destination on rename; the
        // credential lock keeps this window exclusive to this process.
        #[cfg(windows)]
        {
            let _ = fs::remove_file(path);
            fs::rename(&temp, path).with_context(|| {
                let _ = fs::remove_file(&temp);
                format!("replace {} after rename failed: {first}", path.display())
            })?;
        }
        #[cfg(not(windows))]
        {
            let _ = fs::remove_file(&temp);
            return Err(first).with_context(|| format!("replace {}", path.display()));
        }
    }
    Ok(())
}

/// A best-effort advisory lock so concurrent workers share one refresh.
struct FileLock {
    path: PathBuf,
}

impl FileLock {
    async fn acquire(target: &Path) -> Result<Self> {
        let file_name = target
            .file_name()
            .context("auth file has no name")?
            .to_string_lossy()
            .into_owned();
        let path = target.with_file_name(format!("{file_name}.lock"));
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if abandoned(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        bail!("timed out waiting for Codex credential lock");
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("create lock {}", path.display()))
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn abandoned(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > LOCK_STALE)
}
