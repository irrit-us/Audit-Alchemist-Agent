//! Explicit, versioned node configuration. No ambient file discovery.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub cli: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub prompts: Prompts,
    #[serde(default)]
    pub skills: Skills,
    #[serde(default)]
    pub tools: Tools,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Prompts {
    pub system: Option<String>,
    pub system_file: Option<PathBuf>,
    pub append: Option<String>,
    pub append_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Skills {
    pub enabled: Option<Vec<String>>,
    pub custom: Vec<CustomSkill>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomSkill {
    pub name: String,
    pub description: String,
    pub content: Option<String>,
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub resources: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    pub enabled: Option<Vec<String>>,
    pub bash_program: Option<PathBuf>,
    pub bash_timeout_ms: u64,
}
impl Default for Tools {
    fn default() -> Self {
        Self {
            enabled: None,
            bash_program: None,
            bash_timeout_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    #[serde(default)]
    pub enabled: bool,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Child variable name -> parent environment variable name; never secret values.
    #[serde(default)]
    pub env_from: BTreeMap<String, String>,
    pub tools: Option<Vec<String>>,
    #[serde(default = "mcp_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "mcp_bytes")]
    pub max_message_bytes: usize,
}
fn mcp_timeout() -> u64 {
    10_000
}
fn mcp_bytes() -> usize {
    1_048_576
}

#[derive(Debug, Clone, Default)]
pub struct AgentSettings {
    pub prompts: Prompts,
    pub skills: Skills,
    pub tools: Tools,
    pub mcp: BTreeMap<String, McpServer>,
}

fn read_text(path: &Path, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "configuration resource exceeds {limit} bytes"
    );
    String::from_utf8(bytes).context("configuration resource must be UTF-8")
}
fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}
fn resolve_text(
    text: &mut Option<String>,
    file: &mut Option<PathBuf>,
    base: &Path,
    limit: usize,
) -> Result<()> {
    ensure!(
        text.is_none() || file.is_none(),
        "choose inline content or a file, not both"
    );
    if let Some(path) = file.take() {
        *text = Some(read_text(&absolute(base, &path), limit)?);
    }
    if let Some(text) = text {
        ensure!(
            !text.trim().is_empty() && text.len() <= limit,
            "empty or oversized configured content"
        );
    }
    Ok(())
}
pub fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn unique(names: &[String]) -> bool {
    names.iter().collect::<BTreeSet<_>>().len() == names.len()
}

impl FileConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("resolve --config")?;
        let mut config: Self =
            toml::from_str(&read_text(&path, 1_048_576)?).context("invalid TOML configuration")?;
        ensure!(
            config.schema_version == 1,
            "unsupported configuration schema_version"
        );
        let base = path.parent().context("configuration has no parent")?;
        resolve_text(
            &mut config.prompts.system,
            &mut config.prompts.system_file,
            base,
            65_536,
        )?;
        resolve_text(
            &mut config.prompts.append,
            &mut config.prompts.append_file,
            base,
            65_536,
        )?;
        ensure!(
            config.skills.custom.len() <= 64 && config.mcp.len() <= 16,
            "too many configured skills or MCP servers"
        );
        let mut names: BTreeSet<String> = crate::skills::catalog()
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().into())
            .collect();
        for skill in &mut config.skills.custom {
            ensure!(
                identifier(&skill.name) && names.insert(skill.name.clone()),
                "invalid or duplicate skill name"
            );
            ensure!(
                !skill.description.trim().is_empty() && skill.description.len() <= 256,
                "skill description must be 1..256 bytes"
            );
            resolve_text(&mut skill.content, &mut skill.file, base, 8192)?;
            ensure!(
                skill.content.is_some(),
                "custom skill requires content or file"
            );
            ensure!(skill.resources.len() <= 32, "too many skill resources");
            for (path, content) in &skill.resources {
                crate::protocol::relative_path(path)?;
                ensure!(
                    path != "SKILL.md" && content.len() <= 8192,
                    "invalid skill resource"
                );
            }
        }
        if let Some(enabled) = &config.skills.enabled {
            ensure!(
                unique(enabled) && enabled.iter().all(|n| names.contains(n)),
                "unknown or duplicate enabled skill"
            );
        }
        if let Some(enabled) = &config.tools.enabled {
            let available = crate::tools::definitions();
            ensure!(
                unique(enabled)
                    && enabled
                        .iter()
                        .all(|n| available.iter().any(|t| t["name"] == *n)),
                "unknown or duplicate enabled tool"
            );
        }
        ensure!(
            (1..=3_600_000).contains(&config.tools.bash_timeout_ms),
            "bash_timeout_ms must be 1..3600000"
        );
        if let Some(path) = &mut config.tools.bash_program {
            *path = absolute(base, path);
        }
        for (name, server) in &mut config.mcp {
            ensure!(
                identifier(name) && !server.command.trim().is_empty(),
                "invalid MCP server name/command"
            );
            ensure!(
                (1..=3_600_000).contains(&server.timeout_ms),
                "invalid MCP timeout_ms"
            );
            ensure!(
                (1024..=2_097_152).contains(&server.max_message_bytes),
                "invalid MCP max_message_bytes"
            );
            if let Some(tools) = &server.tools {
                ensure!(unique(tools), "duplicate MCP tool selection");
            }
            if let Some(cwd) = &mut server.cwd {
                *cwd = absolute(base, cwd);
            }
            if Path::new(&server.command).components().count() > 1 {
                server.command = absolute(base, Path::new(&server.command))
                    .to_string_lossy()
                    .into();
            }
            for (key, source) in &server.env_from {
                ensure!(
                    !key.is_empty()
                        && !key.contains(['=', '\0'])
                        && !source.is_empty()
                        && !source.contains(['=', '\0']),
                    "invalid MCP environment mapping"
                );
            }
        }
        // File-owned paths are relative to the file; explicit CLI paths remain cwd-relative.
        for key in ["root", "dataset", "output", "trace_dir", "codex_auth_file"] {
            if let Some(value) = config.cli.get_mut(key) {
                let path = value.as_str().context("configured path must be a string")?;
                *value =
                    toml::Value::String(absolute(base, Path::new(path)).to_string_lossy().into());
            }
        }
        ensure!(
            toml::to_string(&config)?.len() <= 1_048_576,
            "resolved configuration exceeds 1 MiB"
        );
        Ok(config)
    }
    pub fn settings(&self) -> AgentSettings {
        AgentSettings {
            prompts: self.prompts.clone(),
            skills: self.skills.clone(),
            tools: self.tools.clone(),
            mcp: self.mcp.clone(),
        }
    }
}

impl AgentSettings {
    pub fn system_prompt(&self) -> String {
        let mut text = self
            .prompts
            .system
            .clone()
            .unwrap_or_else(|| crate::provider::prompt::SYSTEM_PROMPT.into());
        if let Some(append) = &self.prompts.append {
            text.push_str("\n\n");
            text.push_str(append);
        }
        text
    }
    pub fn tool_enabled(&self, name: &str) -> bool {
        self.tools
            .enabled
            .as_ref()
            .is_none_or(|v| v.iter().any(|n| n == name))
    }
    pub fn definitions(&self) -> Vec<Value> {
        crate::tools::definitions()
            .into_iter()
            .filter(|v| self.tool_enabled(v["name"].as_str().unwrap()))
            .map(|mut v| {
                if v["name"] == "bash" {
                    v["description"] = json!(v["description"].as_str().unwrap().replace("defaults to 30000", &format!("defaults to {}", self.tools.bash_timeout_ms)));
                }
                if v["name"] == "load_skill" && !self.skills.custom.is_empty() {
                    v["description"] = json!("Load guidance by exact catalog name. Omit resource for SKILL.md; request an exact available_resources path as needed. save_to exports an explicit resource to a new root-relative file. Run scripts through Bash.");
                }
                v
            }).collect()
    }
    fn skill_enabled(&self, name: &str) -> bool {
        self.skills
            .enabled
            .as_ref()
            .is_none_or(|v| v.iter().any(|n| n == name))
    }
    pub fn catalog(&self) -> Value {
        if !self.tool_enabled("load_skill") {
            return json!([]);
        }
        let mut all = crate::skills::catalog().as_array().unwrap().clone();
        all.extend(
            self.skills
                .custom
                .iter()
                .map(|s| json!({"name":s.name,"description":s.description})),
        );
        all.retain(|s| self.skill_enabled(s["name"].as_str().unwrap()));
        json!(all)
    }
    pub fn load_skill(&self, name: &str, resource: Option<&str>) -> Result<Value> {
        ensure!(
            self.tool_enabled("load_skill") && self.skill_enabled(name),
            "skill is disabled"
        );
        if let Some(skill) = self.skills.custom.iter().find(|s| s.name == name) {
            let resource = resource.unwrap_or("SKILL.md");
            let content = if resource == "SKILL.md" {
                skill.content.as_ref()
            } else {
                skill.resources.get(resource)
            }
            .context("unknown skill resource")?;
            return Ok(
                json!({"name":name,"resource":resource,"content":content,"available_resources":skill.resources.keys().collect::<Vec<_>>()}),
            );
        }
        crate::skills::load(name, resource)
    }
    pub fn snapshot(&self) -> Result<tempfile::NamedTempFile> {
        use std::io::Write;
        let config = FileConfig {
            schema_version: 1,
            cli: BTreeMap::new(),
            prompts: self.prompts.clone(),
            skills: self.skills.clone(),
            tools: self.tools.clone(),
            mcp: self.mcp.clone(),
        };
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(toml::to_string(&config)?.as_bytes())?;
        file.flush()?;
        Ok(file)
    }
}
