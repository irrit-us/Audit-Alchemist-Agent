//! Opt-in, bounded MCP stdio clients. Server instructions never enter the prompt.
use crate::config::McpServer;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    stderr: JoinHandle<()>,
    _group: crate::runner::ProcessGroup,
    #[cfg(windows)]
    _job: crate::runner::ProcessJob,
    limit: usize,
    timeout: Duration,
    id: u64,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stderr.abort();
    }
}
impl Client {
    fn spawn(config: &McpServer, root: &Path) -> Result<Self> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(config.cwd.as_deref().unwrap_or(root))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, source) in &config.env_from {
            command.env(
                key,
                std::env::var_os(source)
                    .with_context(|| format!("missing MCP environment variable {source}"))?,
            );
        }
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().context("start MCP stdio server")?;
        let group = crate::runner::ProcessGroup(child.id());
        #[cfg(windows)]
        let job = crate::runner::ProcessJob::attach(&child)?;
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().context("MCP stdout")?);
        let mut err = child.stderr.take().context("MCP stderr")?;
        let stderr = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut err, &mut tokio::io::sink()).await;
        });
        Ok(Self {
            child,
            input,
            output,
            stderr,
            _group: group,
            #[cfg(windows)]
            _job: job,
            limit: config.max_message_bytes,
            timeout: Duration::from_millis(config.timeout_ms),
            id: 0,
        })
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        ensure!(
            bytes.len() <= self.limit,
            "MCP outgoing message exceeds limit"
        );
        bytes.push(b'\n');
        let input = self.input.as_mut().context("MCP session closed")?;
        input.write_all(&bytes).await?;
        input.flush().await?;
        Ok(())
    }
    async fn request(&mut self, method: &str, params: Value, budget: Duration) -> Result<Value> {
        let timeout = self.timeout.min(budget);
        tokio::time::timeout(timeout, self.exchange(method, params))
            .await
            .context("MCP request timed out")?
    }
    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value> {
        self.id += 1;
        let id = self.id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        for _ in 0..128 {
            let mut line = Vec::new();
            (&mut self.output)
                .take(self.limit as u64 + 1)
                .read_until(b'\n', &mut line)
                .await?;
            ensure!(!line.is_empty(), "MCP server closed stdout");
            ensure!(
                line.len() <= self.limit && line.last() == Some(&b'\n'),
                "MCP message exceeds limit or lacks newline"
            );
            let message: Value = serde_json::from_slice(&line).context("invalid MCP JSON")?;
            ensure!(message["jsonrpc"] == "2.0", "invalid MCP JSON-RPC version");
            if let Some(method) = message["method"].as_str() {
                if let Some(id) = message.get("id") {
                    let response = if method == "ping" {
                        json!({"jsonrpc":"2.0","id":id,"result":{}})
                    } else {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client capability not supported"}})
                    };
                    self.send(response).await?;
                }
                continue;
            }
            ensure!(message["id"] == id, "unexpected MCP response id");
            if message.get("error").is_some() {
                bail!("MCP server returned a JSON-RPC error");
            }
            return message
                .get("result")
                .filter(|v| v.is_object())
                .cloned()
                .context("MCP result must be an object");
        }
        bail!("too many MCP notifications before response")
    }
    async fn shutdown(&mut self) {
        self.input.take();
        if tokio::time::timeout(Duration::from_millis(200), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}

#[derive(Default)]
pub struct McpTools {
    clients: BTreeMap<String, Client>,
    tools: BTreeMap<String, (String, String, Value)>,
}
impl McpTools {
    pub async fn connect(configs: &BTreeMap<String, McpServer>, root: &Path) -> Result<Self> {
        let mut out = Self::default();
        for (name, config) in configs.iter().filter(|(_, c)| c.enabled) {
            let mut client =
                Client::spawn(config, root).with_context(|| format!("MCP server {name}"))?;
            let initialized = client.request("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"audit-harness","version":env!("CARGO_PKG_VERSION")}}), client.timeout).await?;
            ensure!(
                ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
                    .contains(&initialized["protocolVersion"].as_str().unwrap_or("")),
                "unsupported MCP protocol version"
            );
            ensure!(
                initialized["capabilities"]["tools"].is_object(),
                "MCP server does not support tools"
            );
            tokio::time::timeout(
                client.timeout,
                client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})),
            )
            .await
            .context("MCP initialization timed out")??;
            let mut cursor = None;
            let mut cursors = BTreeSet::new();
            let mut names = BTreeSet::new();
            let mut schema_bytes = 0;
            loop {
                let params = cursor
                    .as_ref()
                    .map(|c| json!({"cursor":c}))
                    .unwrap_or(json!({}));
                let page = client.request("tools/list", params, client.timeout).await?;
                let tools = page["tools"]
                    .as_array()
                    .context("MCP tools/list missing tools")?;
                for tool in tools {
                    let remote = tool["name"].as_str().context("MCP tool missing name")?;
                    ensure!(
                        names.insert(remote.to_owned()) && names.len() <= 128,
                        "duplicate or too many MCP tools"
                    );
                    if config
                        .tools
                        .as_ref()
                        .is_some_and(|v| !v.iter().any(|n| n == remote))
                    {
                        continue;
                    }
                    let exposed = format!("mcp_{name}__{remote}");
                    ensure!(
                        exposed.len() <= 64
                            && exposed
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                        "MCP tool name cannot be exposed: {exposed}"
                    );
                    ensure!(
                        tool["inputSchema"]["type"] == "object",
                        "MCP tool inputSchema must be an object schema"
                    );
                    let definition = json!({"name":exposed,"description":tool["description"].as_str().unwrap_or("MCP tool"),"parameters":tool["inputSchema"]});
                    schema_bytes += serde_json::to_vec(&definition)?.len();
                    ensure!(schema_bytes <= 262_144, "MCP tool schemas exceed 256 KiB");
                    ensure!(
                        out.tools
                            .insert(exposed, (name.clone(), remote.into(), definition))
                            .is_none(),
                        "MCP tool namespace collision"
                    );
                }
                cursor = page
                    .get("nextCursor")
                    .map(|v| v.as_str().context("invalid MCP cursor").map(str::to_owned))
                    .transpose()?;
                let Some(next) = &cursor else {
                    break;
                };
                ensure!(
                    cursors.insert(next.clone()) && cursors.len() < 16,
                    "MCP pagination limit or repeated cursor"
                );
            }
            if let Some(selected) = &config.tools {
                ensure!(
                    selected.iter().all(|n| names.contains(n)),
                    "selected MCP tool not found"
                );
            }
            out.clients.insert(name.clone(), client);
        }
        Ok(out)
    }
    pub fn definitions(&self) -> Vec<Value> {
        self.tools.values().map(|(_, _, d)| d.clone()).collect()
    }
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
    pub async fn execute(&mut self, name: &str, arguments: &str, budget: Duration) -> Value {
        let Some((server, remote, _)) = self.tools.get(name) else {
            return json!({"error":"unknown MCP tool"});
        };
        let args = match serde_json::from_str::<Value>(arguments) {
            Ok(v) if v.is_object() => v,
            _ => return json!({"error":"MCP arguments must be a JSON object"}),
        };
        let Some(client) = self.clients.get_mut(server) else {
            return json!({"error":"MCP session unavailable after earlier failure"});
        };
        match client
            .request(
                "tools/call",
                json!({"name":remote,"arguments":args}),
                budget,
            )
            .await
        {
            Ok(mut result) => {
                if !result["content"].is_array()
                    || result.get("isError").is_some_and(|v| !v.is_boolean())
                    || result
                        .get("structuredContent")
                        .is_some_and(|v| !v.is_object())
                {
                    self.clients.remove(server);
                    return json!({"error":"invalid MCP tool result"});
                }
                if result["isError"] == true {
                    result["error"] = json!("MCP tool reported an error");
                }
                result
            }
            Err(error) => {
                self.clients.remove(server);
                json!({"error":format!("{error:#}")})
            }
        }
    }
    pub async fn shutdown(&mut self) {
        for client in self.clients.values_mut() {
            client.shutdown().await;
        }
        self.clients.clear();
    }
}
