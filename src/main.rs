use anyhow::{ensure, Context, Result};
use audit_harness::{
    context::{self, ContextBudget},
    dataset,
    evaluate::{aggregate, score, Report},
    progress::Progress,
    protocol::{Request, VERSION},
    provider::{self, LlmOptions},
    runner::{self, Outcome, RunConfig},
};
use clap::{Args, Parser, Subcommand};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

#[derive(Parser)]
#[command(
    version,
    about = "Lightweight LLM vulnerability auditor and evaluation harness"
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
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
        #[arg(long)]
        dry_run: bool,
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
            let config = config.clone();
            let progress = progress.clone();
            tasks.spawn(async move {
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
        } => benchmark(dataset, agent, agent_arg, limits).await,
        Action::Evaluate {
            dataset,
            llm,
            limits,
        } => {
            llm.validate()?;
            let mut args = llm.arguments();
            args.extend(["--timeout-ms".into(), limits.timeout_ms.to_string()]);
            benchmark(dataset, std::env::current_exe()?, args, limits).await
        }
        Action::Audit {
            target,
            root,
            instruction,
            llm,
            limits,
            dry_run,
        } => {
            llm.validate()?;
            let root = root.canonicalize()?;
            let context = context::build(
                &root,
                &target,
                &ContextBudget::new(llm.max_source_bytes as usize),
            )?;
            let estimated_tokens = context.estimated_tokens();
            if let Some(max) = llm.max_source_tokens {
                ensure!(
                    estimated_tokens <= max as usize,
                    "estimated context of {estimated_tokens} tokens exceeds the {max} token limit; narrow the target"
                );
            }
            if dry_run {
                let file = destination(&limits.output)?;
                emit(
                    &serde_json::json!({
                        "dry_run": true,
                        "target": target,
                        "files": context.files(),
                        "bytes": context.total_bytes(),
                        "estimated_tokens": estimated_tokens,
                        "instruction_bytes": instruction.len(),
                        "model": llm.model,
                        "auth": llm.auth.as_str(),
                    }),
                    file,
                )?;
                return Ok(true);
            }
            let file = destination(&limits.output)?;
            let mut args = llm.arguments();
            args.extend(["--timeout-ms".into(), limits.timeout_ms.to_string()]);
            let config = RunConfig {
                executable: std::env::current_exe()?,
                args,
                root,
                timeout: Duration::from_millis(limits.timeout_ms),
                max_output_bytes: limits.max_output_bytes as usize,
            };
            let request = Request {
                schema_version: VERSION,
                case_id: "audit".into(),
                target,
                instruction,
            };
            let run = tokio::select! {
                run = runner::run(&config, request) => run,
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    anyhow::bail!("audit interrupted; no complete report emitted");
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

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    match execute(Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
