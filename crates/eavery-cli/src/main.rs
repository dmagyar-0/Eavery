//! `eavery-cli`: the headless driver of the Eavery core.
//!
//! Every core feature is built here first and only then in the GUI (working
//! rule 6). A Rust binary is far easier to debug than a Tauri webview, and
//! these paths run in CI without a display.
#![deny(unsafe_code)]

mod render;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use eavery_acp::{AcpEngine, LaunchSpec};
use eavery_core::engine::{Engine, RawAgentEvent, StopReason};
use eavery_core::event::{Decision, PermissionView};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(
    name = "eavery-cli",
    about = "Drive Eavery's core from a terminal.",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Send one request to an engine and print what it does.
    Prompt(PromptArgs),
}

#[derive(Parser, Debug)]
struct PromptArgs {
    /// Which engine to drive. Only `fake` exists until M1 adds the engine
    /// table.
    #[arg(long, default_value = "fake")]
    engine: String,

    /// The fake engine's script (`docs/plan/11-testing-ci.md` §2).
    #[arg(long)]
    script: Option<PathBuf>,

    /// The folder the engine works in. Defaults to the current directory.
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Answer every permission request this way instead of asking. Without it,
    /// permissions are answered from the terminal; with no terminal attached,
    /// they are rejected.
    #[arg(long, value_parser = ["allow", "reject"])]
    answer: Option<String>,

    /// The request to send.
    request: String,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("EAVERY_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("eavery: could not start: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("eavery: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Prompt(args) => prompt(args).await,
    }
}

async fn prompt(args: PromptArgs) -> Result<ExitCode> {
    let cwd = match &args.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir().context("reading the current directory")?,
    };
    let cwd = std::fs::canonicalize(&cwd)
        .with_context(|| format!("{} is not a folder", cwd.display()))?;

    let engine = Arc::new(AcpEngine::new(launch_spec(&args, &cwd)?));

    let info = engine.start().await.context("starting the engine")?;
    render::engine_started(&info);

    let session = engine
        .open_session(&cwd, &[], None)
        .await
        .context("opening a session")?;
    render::session_opened(&session);

    let (tx, mut rx) = mpsc::unbounded_channel::<RawAgentEvent>();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            render::event(&event);
        }
    });

    let handler = permission_handler(args.answer.clone());

    let stop = engine
        .prompt(&session.session_id, &args.request, tx, handler)
        .await;

    // The sink is dropped when `prompt` returns, so this ends on its own.
    let _ = printer.await;

    let code = match stop {
        Ok(stop) => {
            render::finished(stop);
            match stop {
                // A turn that ran to the end, however it ended, is a turn that
                // worked. Only a broken engine is a failed command.
                StopReason::EndTurn
                | StopReason::MaxTokens
                | StopReason::MaxTurnRequests
                | StopReason::Refusal => ExitCode::SUCCESS,
                StopReason::Cancelled => ExitCode::from(130),
            }
        }
        Err(error) => {
            render::engine_error(&error);
            ExitCode::FAILURE
        }
    };

    engine.shutdown().await;
    Ok(code)
}

fn launch_spec(args: &PromptArgs, cwd: &std::path::Path) -> Result<LaunchSpec> {
    if args.engine != "fake" {
        bail!(
            "only the `fake` engine exists so far; real engine discovery arrives with M1. \
             See docs/plan/10-task-breakdown.md."
        );
    }
    let script = args
        .script
        .as_ref()
        .context("the fake engine needs --script (see docs/plan/11-testing-ci.md §2)")?;
    if !script.exists() {
        bail!("no script at {}", script.display());
    }

    let mut spec = LaunchSpec::new("fake", fake_agent_path()?).cwd(cwd);
    spec.args.push("--script".to_owned());
    spec.args.push(script.to_string_lossy().into_owned());
    Ok(spec)
}

/// The fake agent ships beside this binary, which is where Cargo and every
/// installer both put it.
fn fake_agent_path() -> Result<PathBuf> {
    let name = format!("eavery-fake-agent{}", std::env::consts::EXE_SUFFIX);
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        let beside = dir.join(&name);
        if beside.exists() {
            return Ok(beside);
        }
    }
    // Fall back to PATH, which is how it is found when it has been installed.
    Ok(PathBuf::from(name))
}

/// Answers permission requests: from `--answer` when given, otherwise from the
/// terminal.
fn permission_handler(fixed: Option<String>) -> eavery_core::engine::PermissionHandler {
    Arc::new(move |view: PermissionView| {
        let fixed = fixed.clone();
        Box::pin(async move {
            let decision = match fixed.as_deref() {
                Some("allow") => Decision::AllowOnce,
                Some("reject") => Decision::RejectOnce,
                _ => ask_in_terminal(&view).await,
            };
            render::permission_answered(&view, decision);
            decision
        })
    })
}

/// Asks on the terminal. Reading a line blocks, so it runs on the blocking
/// pool: the event stream keeps printing while the answer is pending.
async fn ask_in_terminal(view: &PermissionView) -> Decision {
    if !std::io::stdin().is_terminal() {
        // Nobody is there to say yes. Saying it for them is exactly what must
        // never happen.
        render::permission_unattended(view);
        return Decision::RejectOnce;
    }

    render::permission_prompt(view);
    let answer = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await;

    match answer {
        Ok(Ok(line)) => match line.trim().to_lowercase().as_str() {
            "a" | "allow" | "y" | "yes" => Decision::AllowOnce,
            "r" | "reject" | "n" | "no" => Decision::RejectOnce,
            // Anything else, including an empty line, is not consent.
            _ => Decision::RejectOnce,
        },
        _ => Decision::RejectOnce,
    }
}

/// Flushes stdout after each line so the stream is watchable when it is piped.
fn println_flush(line: impl AsRef<str>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", line.as_ref());
    let _ = out.flush();
}
