//! Eavery's desktop shell.
//!
//! The frontend is a renderer: it calls the commands in
//! `docs/plan/03-architecture.md` §7 and listens to one event. Everything it
//! shows comes from `eavery-core`, which is where the decisions and the
//! history live — and where the CLI and the tests can reach them.
#![deny(unsafe_code)]

pub mod commands;
pub mod state;

use std::path::PathBuf;

use state::AppCore;
use tauri::Manager;

/// Where the database, the journals and the logs live
/// (`docs/plan/03-architecture.md` §8).
///
/// `EAVERY_DATA_DIR` overrides it, which is how the tests and a trial run on a
/// copy of a real folder stay out of the real one.
fn data_dir() -> Option<PathBuf> {
    if let Some(from_env) = std::env::var_os("EAVERY_DATA_DIR") {
        return Some(PathBuf::from(from_env));
    }
    directories::ProjectDirs::from("dev", "eavery", "Eavery")
        .map(|dirs| dirs.data_dir().to_path_buf())
}

/// Registers every command the frontend may call
/// (`docs/plan/03-architecture.md` §7).
///
/// The tests build their app through this too, so what they exercise is the
/// list the window actually has rather than a copy of it that can drift.
pub fn register<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::list_projects,
        commands::open_project,
        commands::remove_project,
        commands::set_project_engine,
        commands::list_engines,
        commands::run_health_check,
        commands::start_turn,
        commands::answer_permission,
        commands::cancel_turn,
        commands::list_turns,
        commands::list_checkpoints,
        commands::checkpoint_now,
        commands::restore_checkpoint,
        commands::diff_summary,
        commands::list_events,
        commands::list_sessions,
        commands::list_audit,
        commands::journal_size,
        commands::journal_info,
        commands::unprotected_files,
        commands::get_settings,
        commands::set_settings,
        commands::diagnostics,
    ])
}

/// Sends `tracing` to stderr and to `<data_dir>/logs/eavery.log`
/// (`docs/plan/03-architecture.md` §9). The file is what the Diagnostics
/// panel reads; stderr is for whoever launched the app from a terminal.
///
/// When the file cannot be opened the app still starts, logging to stderr
/// alone and saying so there: a log that cannot be written is not a reason to
/// refuse to run.
fn init_logging(data_dir: Option<&std::path::Path>) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;

    let filter = tracing_subscriber::EnvFilter::try_from_env("EAVERY_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let log_file = data_dir.and_then(|dir| match eavery_core::diagnostics::open_log(dir) {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!(
                "eavery: could not open the log at {}: {error}",
                eavery_core::diagnostics::log_path(dir).display()
            );
            None
        }
    });

    match log_file {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::io::stderr.and(std::sync::Mutex::new(file)))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let data_dir = data_dir();
    init_logging(data_dir.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Eavery starting");

    register(tauri::Builder::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let data_dir = data_dir.ok_or("this system has no data directory")?;
            let core = AppCore::open(data_dir, Some(app.handle().clone()))?;
            app.manage(core);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Eavery window")
        .run(|app, event| {
            // An engine is a child process, and a child process that outlives
            // the window is one nobody will ever stop (M3-T09).
            if let tauri::RunEvent::Exit = event
                && let Some(core) = app.try_state::<AppCore>()
            {
                tauri::async_runtime::block_on(core.shutdown());
            }
        });
}
