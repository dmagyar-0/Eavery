//! `eavery-cli`: the headless driver of the Eavery core.
//!
//! Every core feature is built here first and only then in the GUI (working
//! rule 6). A Rust binary is far easier to debug than a Tauri webview, and
//! these paths run in CI without a display.
#![deny(unsafe_code)]

mod project;
mod render;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use eavery_acp::{AcpEngine, LaunchSpec};
use eavery_core::engine::{Engine, RawAgentEvent, StopReason};
use eavery_core::event::{Decision, PermissionView};
use eavery_core::model::EngineStatus;
use eavery_engines::discovery::Resolver;
use eavery_engines::health::{self, HealthOptions};
use eavery_engines::spec::EngineSpec;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(
    name = "eavery-cli",
    about = "Drive Eavery's core from a terminal.",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    /// Where Eavery keeps its database and its journals. Defaults to
    /// `$EAVERY_DATA_DIR`, then to the platform's data directory
    /// (`docs/plan/03-architecture.md` §8).
    #[arg(long, global = true, value_name = "PATH")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Send one request to an engine and print what it does. No Project, no
    /// history: this is the engine-level tool the ACP work was built with.
    Prompt(PromptArgs),
    /// Say which engines are installed on this computer and whether they work.
    Engines(EnginesArgs),
    /// Open folders as Projects and list the ones already open.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Run one turn in a Project: protect, do the work, protect, report.
    Run(RunArgs),
    /// The checkpoints of a Project, newest first.
    History(HistoryArgs),
    /// Go back to a checkpoint. Without `--to`, to the point before the last
    /// turn.
    Undo(UndoArgs),
    /// What changed between two checkpoints, or since one.
    Diff(DiffArgs),
}

#[derive(Subcommand, Debug)]
enum ProjectCommand {
    /// Open a folder as a Project, protecting it from now on.
    Open {
        /// The folder.
        path: PathBuf,
        /// What to call it. Defaults to the folder's own name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Every Project that has been opened.
    List,
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// The Project: its id, or the folder itself.
    #[arg(long)]
    pub project: String,

    /// Which engine to drive.
    #[arg(long, default_value = "fake")]
    pub engine: String,

    #[command(flatten)]
    pub launch: LaunchArgs,

    /// Answer every permission request this way instead of asking.
    #[arg(long, value_parser = ["allow", "reject"])]
    pub answer: Option<String>,

    /// The request to send.
    pub request: String,
}

#[derive(Parser, Debug)]
struct HistoryArgs {
    #[arg(long)]
    project: String,

    /// How many checkpoints to show.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Parser, Debug)]
struct UndoArgs {
    #[arg(long)]
    project: String,

    /// The checkpoint to go back to. The first few characters are enough.
    #[arg(long)]
    to: Option<String>,
}

#[derive(Parser, Debug)]
struct DiffArgs {
    #[arg(long)]
    project: String,

    /// The checkpoint to compare from.
    from: String,

    /// The checkpoint to compare with. Left out, the folder as it is now.
    to: Option<String>,
}

/// How to find and start an engine, shared by `prompt` and `engines`.
#[derive(Args, Debug, Default)]
pub struct LaunchArgs {
    /// The fake engine's script (`docs/plan/11-testing-ci.md` §2).
    #[arg(long)]
    pub script: Option<PathBuf>,

    /// Use this executable instead of searching for one. The command-line
    /// equivalent of Settings → Assistants → path.
    #[arg(long, value_name = "PATH")]
    pub engine_path: Option<PathBuf>,

    /// Extra environment for the engine child only, `NAME=VALUE`. Repeatable.
    /// This is how goose is told which provider and model to use.
    #[arg(long, value_name = "NAME=VALUE")]
    pub env: Vec<String>,
}

#[derive(Parser, Debug)]
struct PromptArgs {
    /// Which engine to drive: an id from the engine table (`eavery-cli
    /// engines` lists them).
    #[arg(long, default_value = "fake")]
    engine: String,

    #[command(flatten)]
    launch: LaunchArgs,

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

#[derive(Parser, Debug)]
struct EnginesArgs {
    /// Check only this engine, and exit non-zero unless it is ready.
    #[arg(long)]
    engine: Option<String>,

    /// Also send a one-word prompt, which costs a request and a wait
    /// (`docs/plan/04-acp-engines.md` §9).
    #[arg(long)]
    deep: bool,

    /// Include engines that are hidden by default: the fake one, and any
    /// marked experimental.
    #[arg(long)]
    all: bool,

    /// Print the statuses as JSON instead of a table.
    #[arg(long)]
    json: bool,

    #[command(flatten)]
    launch: LaunchArgs,
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
    let data_dir = project::data_dir(cli.data_dir.as_ref())?;
    match cli.command {
        Command::Prompt(args) => prompt(args).await,
        Command::Engines(args) => engines(args).await,
        Command::Project(ProjectCommand::Open { path, name }) => {
            project::open(&data_dir, &path, name.as_deref()).await
        }
        Command::Project(ProjectCommand::List) => project::list(&data_dir),
        Command::Run(args) => project::run(&data_dir, &args).await,
        Command::History(args) => project::history(&data_dir, &args.project, args.limit).await,
        Command::Undo(args) => project::undo(&data_dir, &args.project, args.to.as_deref()).await,
        Command::Diff(args) => {
            project::diff(&data_dir, &args.project, &args.from, args.to.as_deref()).await
        }
    }
}

async fn prompt(args: PromptArgs) -> Result<ExitCode> {
    let cwd = match &args.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir().context("reading the current directory")?,
    };
    let cwd = eavery_core::paths::canonicalize(&cwd)
        .with_context(|| format!("{} is not a folder", cwd.display()))?;

    let engine = Arc::new(AcpEngine::new(launch_spec(
        &args.engine,
        &args.launch,
        &cwd,
    )?));

    let info = engine.start().await.context("starting the engine")?;
    render::engine_started(&info);

    let session = engine
        .open_session(&cwd, &[], None)
        .await
        .context("opening a session")?;
    render::session_opened(&session);

    // One task owns the transcript. The engine's events and the permission
    // handler's lines are two independent producers, and letting both write to
    // stdout puts the transcript out of the order the engine sent it in.
    let (event_tx, event_rx) = mpsc::unbounded_channel::<RawAgentEvent>();
    let (line_tx, line_rx) = mpsc::unbounded_channel::<Line>();
    let printer = tokio::spawn(transcript(event_rx, line_rx));

    let handler = permission_handler(args.answer.clone(), line_tx.clone());
    let stop = engine
        .prompt(&session.session_id, &args.request, event_tx, handler)
        .await;

    // Both producers are finished: the sink went with `prompt`, and so did the
    // handler it held. Closing this last one ends the transcript task.
    drop(line_tx);
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

pub fn launch_spec(
    engine: &str,
    launch_args: &LaunchArgs,
    cwd: &std::path::Path,
) -> Result<LaunchSpec> {
    let spec = engine_spec(engine)?;
    // The arguments are checked before anything is looked for: a command that
    // was typed wrong says so, whatever happens to be installed.
    let extra_args = extra_args(spec, launch_args)?;
    let env = parse_env(&launch_args.env)?;

    let resolver = resolver_for(launch_args, spec);
    let resolved = resolver
        .resolve(spec)
        .map_err(|error| not_installed(spec, &error))?;

    tracing::debug!(
        engine = spec.id,
        program = %resolved.program.display(),
        via = ?resolved.via,
        "found the engine"
    );
    if resolved.companion_missing {
        // Not fatal: the adapter may still find a login elsewhere. Worth
        // saying, because "it started and then said it was not signed in" is
        // otherwise a mystery.
        tracing::warn!(
            engine = spec.id,
            "the CLI this adapter drives is not installed; it may have no login"
        );
    }

    let mut launch = resolved.launch_spec().cwd(cwd);
    launch.args.extend(extra_args);
    launch.env.extend(env);
    Ok(launch)
}

fn engine_spec(id: &str) -> Result<&'static EngineSpec> {
    eavery_engines::find(id).ok_or_else(|| {
        let known: Vec<&str> = eavery_engines::ENGINES.iter().map(|spec| spec.id).collect();
        anyhow!(
            "no engine called `{id}`. Known engines: {}",
            known.join(", ")
        )
    })
}

fn resolver_for(args: &LaunchArgs, spec: &EngineSpec) -> Resolver {
    // The PATH probe happens here, on the first resolution, rather than at
    // process start: a `--help` should not spawn a login shell.
    let resolver = Resolver::current();
    match &args.engine_path {
        Some(path) => resolver.with_explicit_path(spec.id, path),
        None => resolver,
    }
}

/// Arguments an engine needs at run time rather than from the table. Only the
/// fake engine has any: its script.
fn extra_args(spec: &EngineSpec, args: &LaunchArgs) -> Result<Vec<String>> {
    if spec.id != "fake" {
        if args.script.is_some() {
            bail!("--script only applies to the fake engine");
        }
        return Ok(Vec::new());
    }
    let script = args
        .script
        .as_ref()
        .context("the fake engine needs --script (see docs/plan/11-testing-ci.md §2)")?;
    if !script.exists() {
        bail!("no script at {}", script.display());
    }
    // The engine runs in the project folder, so a path relative to where the
    // user typed the command would resolve against the wrong directory.
    let script = eavery_core::paths::canonicalize(script)
        .with_context(|| format!("resolving {}", script.display()))?;
    Ok(vec![
        "--script".to_owned(),
        script.to_string_lossy().into_owned(),
    ])
}

fn parse_env(pairs: &[String]) -> Result<Vec<(String, String)>> {
    pairs
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .with_context(|| format!("--env wants NAME=VALUE, got `{pair}`"))
        })
        .collect()
}

/// The "not installed" message, with everywhere that was looked. A user who is
/// told an engine is missing is always told where Eavery looked for it.
fn not_installed(spec: &EngineSpec, error: &eavery_engines::NotInstalled) -> anyhow::Error {
    if error.needs_node {
        return anyhow!(
            "{} needs Node.js, which is not installed. {}",
            spec.display_name,
            eavery_engines::instructions::NEEDS_NODE
        );
    }
    anyhow!(
        "{} is not installed. {}\nLooked in:\n  {}",
        spec.display_name,
        spec.sign_in_instructions,
        error.searched.join("\n  ")
    )
}

async fn engines(args: EnginesArgs) -> Result<ExitCode> {
    let env = parse_env(&args.launch.env)?;

    let wanted: Vec<&'static EngineSpec> = match &args.engine {
        Some(id) => vec![engine_spec(id)?],
        None => eavery_engines::ENGINES
            .iter()
            .filter(|spec| args.all || spec.visible())
            .filter(|spec| args.all || !spec.experimental)
            // Checking the fake engine means spawning it, and without a script
            // it can only fail. Nobody listing their assistants wants that row.
            .filter(|spec| spec.id != "fake" || args.launch.script.is_some())
            .collect(),
    };

    let mut checks = Vec::new();
    for spec in wanted {
        checks.push((
            spec,
            HealthOptions {
                deep: args.deep,
                extra_args: extra_args(spec, &args.launch)?,
                env: env.clone(),
            },
        ));
    }

    let mut statuses = Vec::new();
    for (spec, options) in checks {
        let resolver = resolver_for(&args.launch, spec);
        let (where_, status) = match resolver.resolve(spec) {
            Ok(resolved) => (
                Some((resolved.program.clone(), resolved.via)),
                health::check_resolved(spec, &resolved, &options).await,
            ),
            Err(error) => (None, health::not_installed(spec, error)),
        };
        statuses.push((spec, where_, status));
    }

    if args.json {
        println_flush(render::engines_json(&statuses)?);
    } else {
        for line in render::engines_table(&statuses) {
            println_flush(line);
        }
    }

    // `--engine` is the question "can I use this one right now", and the shell
    // should be able to act on the answer.
    let ready = statuses
        .iter()
        .all(|(_, _, status)| matches!(status, EngineStatus::Ready { .. }));
    Ok(if args.engine.is_some() && !ready {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// One line of the transcript.
enum Line {
    Say(String),
    /// Print it, then say so, so a terminal question is on screen before the
    /// answer is read.
    Ask(String, tokio::sync::oneshot::Sender<()>),
}

/// The only writer of the event transcript.
///
/// `biased` is what does the work: the engine's events are already queued by
/// the time the permission handler that follows them runs, so draining them
/// first puts the transcript back in the engine's order.
async fn transcript(
    mut events: mpsc::UnboundedReceiver<RawAgentEvent>,
    mut lines: mpsc::UnboundedReceiver<Line>,
) {
    let mut events_open = true;
    let mut lines_open = true;
    while events_open || lines_open {
        tokio::select! {
            biased;
            event = events.recv(), if events_open => match event {
                Some(event) => render::event(&event),
                None => events_open = false,
            },
            line = lines.recv(), if lines_open => match line {
                Some(Line::Say(text)) => println_flush(text),
                Some(Line::Ask(text, printed)) => {
                    println_flush(text);
                    let _ = printed.send(());
                }
                None => lines_open = false,
            },
        }
    }
}

/// Answers permission requests: from `--answer` when given, otherwise from the
/// terminal.
fn permission_handler(
    fixed: Option<String>,
    lines: mpsc::UnboundedSender<Line>,
) -> eavery_core::engine::PermissionHandler {
    Arc::new(move |view: PermissionView| {
        let fixed = fixed.clone();
        let lines = lines.clone();
        Box::pin(async move {
            let decision = match fixed.as_deref() {
                Some("allow") => Decision::AllowOnce,
                Some("reject") => Decision::RejectOnce,
                _ => ask_in_terminal(&view, &lines).await,
            };
            let _ = lines.send(Line::Say(render::permission_answered(&view, decision)));
            decision
        })
    })
}

/// Answers one permission request, for the commands that print directly
/// rather than through the transcript queue: from `--answer` when it was
/// given, otherwise from the terminal.
///
/// The turn engine calls this in the middle of a turn, from the same task that
/// prints the events, so the question is already in its place in the
/// transcript by the time it is asked.
pub async fn answer_permission(view: &PermissionView, fixed: Option<&str>) -> Decision {
    match fixed {
        Some("allow") => return Decision::AllowOnce,
        Some("reject") => return Decision::RejectOnce,
        _ => {}
    }
    if !std::io::stdin().is_terminal() {
        // Nobody is there to say yes. Saying it for them is exactly what must
        // never happen.
        println_flush(render::permission_unattended(view));
        return Decision::RejectOnce;
    }
    println_flush("         [a]llow / [r]eject:");
    read_answer().await
}

/// Reads one line from the terminal. Reading blocks, so it runs on the
/// blocking pool: whatever else is streaming keeps streaming.
async fn read_answer() -> Decision {
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

/// Asks on the terminal, through the transcript queue that `prompt` uses.
async fn ask_in_terminal(view: &PermissionView, lines: &mpsc::UnboundedSender<Line>) -> Decision {
    if !std::io::stdin().is_terminal() {
        // Nobody is there to say yes. Saying it for them is exactly what must
        // never happen.
        let _ = lines.send(Line::Say(render::permission_unattended(view)));
        return Decision::RejectOnce;
    }

    // Wait for the question to reach the screen before reading the answer.
    let (printed, on_screen) = tokio::sync::oneshot::channel();
    if lines
        .send(Line::Ask(render::permission_prompt(view), printed))
        .is_err()
    {
        return Decision::RejectOnce;
    }
    let _ = on_screen.await;
    read_answer().await
}

/// Flushes stdout after each line so the stream is watchable when it is piped.
fn println_flush(line: impl AsRef<str>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", line.as_ref());
    let _ = out.flush();
}
