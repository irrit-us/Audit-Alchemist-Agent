use anyhow::{bail, ensure, Context, Result};
use audit_harness::{
    context::{self, ContextBudget},
    dataset,
    evaluate::{aggregate, score, Report},
    output::{self, ColorChoice, OutputFormat},
    progress::Progress,
    protocol::{Request, VERSION},
    provider::{self, LlmOptions},
    runner::{self, Outcome, RunConfig, RunResult},
    tui,
};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::{
    fs::OpenOptions,
    io::{IsTerminal, Read, Write},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

#[derive(Parser)]
#[command(
    name = "alchemist",
    version,
    about = "Lightweight LLM vulnerability auditor and evaluation harness"
)]
struct Cli {
    /// Explicit standalone TOML node configuration; CLI flags override it.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(skip)]
    settings: audit_harness::config::AgentSettings,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Validate configuration without credentials, network, or MCP startup.
    CheckConfig,
    /// List built-in skills or read one skill/resource without model credentials.
    Skills {
        name: Option<String>,
        #[arg(long, requires = "name")]
        resource: Option<String>,
        /// Export the exact raw resource to a new file instead of printing JSON.
        #[arg(long, requires_all = ["name", "resource"])]
        output: Option<PathBuf>,
    },
    /// Check Bash execution and discover optional local debugging tools.
    Doctor {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Summarize a completed or live JSONL run journal without displaying payloads.
    InspectTrace { path: PathBuf },
    /// Audit a source file/directory through a plain LLM API.
    Audit {
        #[arg(long)]
        target: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(
            long,
            default_value = "Audit attacker-controlled inputs for concrete vulnerabilities."
        )]
        instruction: String,
        /// Build and report the context and its token estimate without calling the model.
        #[arg(long, num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        dry_run: bool,
        /// Console format for the live stream on stderr.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// When to use ANSI color on stderr.
        #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
        color: ColorChoice,
        /// Run the audit in an interactive terminal UI.
        #[arg(long, num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        tui: bool,
        #[command(flatten)]
        llm: LlmOptions,
        #[command(flatten)]
        limits: Limits,
    },
    /// Evaluate the built-in LLM auditor on a labeled dataset.
    Evaluate {
        #[arg(long)]
        dataset: PathBuf,
        #[command(flatten)]
        llm: LlmOptions,
        #[command(flatten)]
        limits: Limits,
    },
    /// Evaluate another executable implementing the JSON stdin/stdout protocol.
    Benchmark {
        #[arg(long)]
        dataset: PathBuf,
        #[arg(long)]
        agent: PathBuf,
        #[arg(long, allow_hyphen_values = true)]
        agent_arg: Vec<String>,
        #[command(flatten)]
        limits: Limits,
    },
    /// Validate dataset schema, paths, labels and source lines without running an agent.
    Validate {
        #[arg(long)]
        dataset: PathBuf,
    },
    /// Internal protocol adapter: read one request from stdin, return one JSON response.
    Agent {
        #[command(flatten)]
        llm: LlmOptions,
        #[arg(long, default_value_t = 60_000)]
        timeout_ms: u64,
    },
}

#[derive(Clone, Args)]
struct Limits {
    #[arg(long, default_value_t = 60_000, value_parser = clap::value_parser!(u64).range(1..=3_600_000))]
    timeout_ms: u64,
    #[arg(long, default_value_t = 1_048_576, value_parser = clap::value_parser!(u32).range(1..=16_777_216))]
    max_output_bytes: u32,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=32))]
    jobs: u16,
    /// Save JSON to a new file; existing files are never overwritten.
    #[arg(long)]
    output: Option<PathBuf>,
}

fn destination(path: &Option<PathBuf>) -> Result<Option<std::fs::File>> {
    path.as_ref()
        .map(|p| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .with_context(|| format!("create report {}", p.display()))
        })
        .transpose()
}

fn emit(value: &impl serde::Serialize, file: Option<std::fs::File>) -> Result<()> {
    let mut writer: Box<dyn Write> = match file {
        Some(file) => Box::new(file),
        None => Box::new(std::io::stdout().lock()),
    };
    serde_json::to_writer_pretty(&mut writer, value)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}

async fn benchmark(
    path: PathBuf,
    executable: PathBuf,
    args: Vec<String>,
    limits: Limits,
    isolate: bool,
) -> Result<bool> {
    let (dataset, root) = dataset::load(&path)?;
    let executable = if executable.components().count() > 1 {
        executable
            .canonicalize()
            .context("resolve agent executable")?
    } else {
        executable
    };
    let file = destination(&limits.output)?;
    let config = RunConfig {
        executable,
        args,
        root,
        timeout: Duration::from_millis(limits.timeout_ms),
        max_output_bytes: limits.max_output_bytes as usize,
    };
    let start = Instant::now();
    let progress = Progress::new(dataset.cases.len());
    let mut pending = dataset.cases.into_iter().enumerate();
    let mut tasks = JoinSet::new();
    let mut reports = Vec::new();
    let mut interrupted = false;
    loop {
        while tasks.len() < limits.jobs as usize {
            let Some((index, case)) = pending.next() else {
                break;
            };
            let mut config = config.clone();
            let workspace = if isolate {
                Some(dataset::stage_workspace(&config.root, &path)?)
            } else {
                None
            };
            if let Some(workspace) = &workspace {
                config.root = workspace.path().to_owned();
            }
            let progress = progress.clone();
            tasks.spawn(async move {
                let _workspace = workspace;
                progress.begin(&case.id);
                let request = Request {
                    schema_version: VERSION,
                    case_id: case.id.clone(),
                    target: case.target.clone(),
                    instruction: case.instruction.clone(),
                };
                let run = runner::run(&config, request).await;
                progress.finish(
                    &case.id,
                    run.outcome.as_str(),
                    run.outcome != Outcome::Success,
                    run.elapsed_ms,
                );
                (index, score(&case, run))
            });
        }
        if tasks.is_empty() {
            break;
        }
        tokio::select! {
            item = tasks.join_next() => reports.push(item.context("missing worker")?.context("agent worker failed")?),
            signal = tokio::signal::ctrl_c() => {
                signal?;
                interrupted = true;
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                break;
            }
        }
    }
    ensure!(
        !interrupted,
        "evaluation interrupted; no complete report emitted"
    );
    reports.sort_by_key(|(index, _)| *index);
    let cases: Vec<_> = reports.into_iter().map(|(_, report)| report).collect();
    let metrics = aggregate(&cases, start.elapsed().as_millis() as u64);
    progress.summary();
    let success = metrics.failed_cases == 0;
    let report = Report {
        schema_version: VERSION,
        dataset: dataset.name,
        executable: config.executable.display().to_string(),
        args: config.args,
        jobs: limits.jobs as usize,
        timeout_ms: limits.timeout_ms,
        max_output_bytes: limits.max_output_bytes as usize,
        metrics,
        cases,
    };
    emit(&report, file)?;
    Ok(success)
}

async fn execute(cli: Cli) -> Result<bool> {
    match cli.command {
        Action::CheckConfig => {
            ensure!(cli.config.is_some(), "check-config requires --config");
            emit(
                &serde_json::json!({"valid":true,"tools":cli.settings.definitions().iter().map(|v| &v["name"]).collect::<Vec<_>>(),"skills":cli.settings.catalog(),"mcp_servers":cli.settings.mcp.iter().filter(|(_, c)| c.enabled).map(|(n, _)| n).collect::<Vec<_>>()}),
                None,
            )?;
            Ok(true)
        }
        Action::Skills {
            name,
            resource,
            output,
        } => {
            let value = match name {
                Some(name) => cli.settings.load_skill(&name, resource.as_deref())?,
                None => cli.settings.catalog(),
            };
            if let Some(mut file) = destination(&output)? {
                file.write_all(
                    value["content"]
                        .as_str()
                        .context("missing skill resource")?
                        .as_bytes(),
                )?;
                file.flush()?;
            } else {
                emit(&value, None)?;
            }
            Ok(true)
        }
        Action::InspectTrace { path } => {
            emit(&audit_harness::monitor::inspect(&path)?, None)?;
            Ok(true)
        }
        Action::Doctor { root } => {
            let report = audit_harness::tools::doctor_with(
                &root,
                cli.settings.tools.bash_program.as_deref(),
            )
            .await?;
            let ready = report["ready"] == true;
            emit(&report, None)?;
            Ok(ready)
        }
        Action::Validate { dataset: path } => {
            let (dataset, _) = dataset::load(&path)?;
            emit(
                &serde_json::json!({"valid": true, "dataset": dataset.name, "cases": dataset.cases.len()}),
                None,
            )?;
            Ok(true)
        }
        Action::Benchmark {
            dataset,
            agent,
            agent_arg,
            limits,
        } => benchmark(dataset, agent, agent_arg, limits, false).await,
        Action::Evaluate {
            dataset,
            llm,
            limits,
        } => {
            llm.validate()?;
            let snapshot = llm.settings.snapshot()?;
            let mut args = llm.arguments()?;
            args.extend([
                "--config".into(),
                snapshot.path().to_string_lossy().into_owned(),
            ]);
            args.extend(["--timeout-ms".into(), limits.timeout_ms.to_string()]);
            benchmark(dataset, std::env::current_exe()?, args, limits, true).await
        }
        Action::Audit {
            target,
            root,
            instruction,
            dry_run,
            format,
            color,
            tui: use_tui,
            llm,
            limits,
        } => {
            if !dry_run {
                llm.validate()?;
            }
            let root = root.canonicalize()?;
            let context = context::initial(
                &root,
                &target,
                &ContextBudget::new(llm.max_source_bytes as usize),
            )?;
            let estimated_tokens = context.estimated_tokens();
            llm.validate_context(&context)?;
            let prompt =
                provider::prompt::prepare_with(&instruction, &target, &context, &llm.settings)?;
            let file = destination(&limits.output)?;
            if dry_run {
                emit(
                    &serde_json::json!({
                        "dry_run": true,
                        "target": target,
                        "files": context.files(),
                        "bytes": context.total_bytes(),
                        "estimated_tokens": estimated_tokens,
                        "estimated_prompt_tokens": prompt.estimated_tokens(),
                        "tools": llm.settings.definitions().iter().map(|tool| tool["name"].clone()).collect::<Vec<_>>(),
                        "skills": llm.settings.catalog(),
                        "mcp_servers":llm.settings.mcp.iter().filter(|(_, c)| c.enabled).map(|(n, _)| n).collect::<Vec<_>>(),
                        "max_tool_calls": llm.max_tool_calls,
                        "context_policy":llm.context_policy.as_str(),
                        "context_keep_turns":llm.context_keep_turns,
                        "max_tool_output_bytes":llm.max_tool_output_bytes,
                        "instruction_bytes": instruction.len(),
                        "model": llm.model,
                        "auth": llm.auth.as_str(),
                        "wire_api": llm.effective_wire().as_str(),
                    }),
                    file,
                )?;
                return Ok(true);
            }
            let case_id = "audit".to_owned();
            let request = Request {
                schema_version: VERSION,
                case_id: case_id.clone(),
                target: target.clone(),
                instruction,
            };
            let timeout = Duration::from_millis(limits.timeout_ms);
            let start = Instant::now();
            let run = if use_tui {
                if !std::io::stderr().is_terminal() {
                    bail!("--tui requires a terminal; omit --tui or choose a --format");
                }
                let app = tui::App::new(target.clone(), llm.model.clone());
                match tui::run_audit(app, llm.clone(), request, context, root.clone(), timeout)
                    .await
                {
                    Ok(response) => RunResult {
                        case_id,
                        outcome: Outcome::Success,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        error: None,
                        findings: response.findings,
                    },
                    Err(error) => RunResult {
                        case_id,
                        outcome: if error.is::<provider::AuditTimeout>() {
                            Outcome::Timeout
                        } else {
                            Outcome::ProviderError
                        },
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        exit_code: None,
                        error: Some(format!("{error:#}")),
                        findings: Vec::new(),
                    },
                }
            } else {
                let is_tty = std::io::stderr().is_terminal();
                let use_color = color.resolve(is_tty);
                let stderr = std::io::stderr();
                let mut renderer = output::Renderer::new(format, use_color, is_tty, stderr.lock());
                let result =
                    provider::audit_with(&llm, &request, &context, &root, timeout, &mut renderer)
                        .await;
                match result {
                    Ok(response) => {
                        let _ = renderer.finish(&response);
                        RunResult {
                            case_id,
                            outcome: Outcome::Success,
                            elapsed_ms: start.elapsed().as_millis() as u64,
                            exit_code: Some(0),
                            error: None,
                            findings: response.findings,
                        }
                    }
                    Err(error) => RunResult {
                        case_id,
                        outcome: if error.is::<provider::AuditTimeout>() {
                            Outcome::Timeout
                        } else {
                            Outcome::ProviderError
                        },
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        exit_code: None,
                        error: Some(format!("{error:#}")),
                        findings: Vec::new(),
                    },
                }
            };
            let success = run.outcome == Outcome::Success;
            emit(&run, file)?;
            Ok(success)
        }
        Action::Agent { llm, timeout_ms } => {
            ensure!(
                (1..=3_600_000).contains(&timeout_ms),
                "timeout must be 1..=3600000 ms"
            );
            let mut bytes = Vec::new();
            std::io::stdin().take(131_073).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 131_072, "request exceeds 128 KiB");
            let request: Request =
                serde_json::from_slice(&bytes).context("invalid agent request")?;
            let response = provider::audit(
                &llm,
                &request,
                &std::env::current_dir()?,
                Duration::from_millis(timeout_ms),
            )
            .await?;
            emit(&response, None)?;
            Ok(true)
        }
    }
}

fn parse_cli() -> Result<Cli> {
    let args: Vec<_> = std::env::args_os().collect();
    // Bootstrap locates the global path even when TOML supplies required flags.
    // The final parse remains strict and uses Clap's normal validators/precedence.
    let bootstrap = Cli::command().ignore_errors(true).get_matches_from(&args);
    let path = bootstrap.get_one::<PathBuf>("config");
    let Some(path) = path else {
        return Ok(Cli::parse_from(args));
    };
    let config = audit_harness::config::FileConfig::load(path)?;
    let mut command = Cli::command();
    for (key, value) in &config.cli {
        let known = command.get_subcommands().any(|c| {
            c.get_arguments()
                .any(|a| a.get_id().as_str() == key && a.get_long().is_some())
        });
        ensure!(
            known && key != "config" && key != "help" && key != "version",
            "unknown configured CLI option: {key}"
        );
        for sub in command.get_subcommands_mut() {
            let Some(arg) = sub
                .get_arguments()
                .find(|a| a.get_id().as_str() == key)
                .cloned()
            else {
                continue;
            };
            let values: Vec<String> = match value {
                toml::Value::Boolean(v)
                    if matches!(
                        arg.get_action(),
                        clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
                    ) =>
                {
                    vec![v.to_string()]
                }
                toml::Value::String(v)
                    if matches!(
                        arg.get_action(),
                        clap::ArgAction::Set | clap::ArgAction::Append
                    ) =>
                {
                    vec![v.clone()]
                }
                toml::Value::Integer(v) if matches!(arg.get_action(), clap::ArgAction::Set) => {
                    vec![v.to_string()]
                }
                toml::Value::Array(v) if matches!(arg.get_action(), clap::ArgAction::Append) => v
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .context("CLI list must contain strings")
                    })
                    .collect::<Result<_>>()?,
                _ => bail!("invalid TOML type for CLI option {key}"),
            };
            // Validate each supplied value even when an explicit flag overrides it.
            let probe = clap::Command::new("config").arg(
                clap::Arg::new("value")
                    .num_args(1)
                    .allow_hyphen_values(true)
                    .value_parser(arg.get_value_parser().clone()),
            );
            for value in &values {
                probe
                    .clone()
                    .try_get_matches_from(["config", value])
                    .with_context(|| format!("invalid configured CLI option {key}"))?;
            }
            *sub = sub
                .clone()
                .mut_arg(key, |arg| arg.required(false).default_values(values));
        }
    }
    let matches = command.get_matches_from(args);
    let mut cli = Cli::from_arg_matches(&matches)?;
    cli.settings = config.settings();
    match &mut cli.command {
        Action::Audit { llm, .. } | Action::Agent { llm, .. } | Action::Evaluate { llm, .. } => {
            llm.settings = cli.settings.clone()
        }
        _ => {}
    }
    Ok(cli)
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let result = match parse_cli() {
        Ok(cli) => execute(cli).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
