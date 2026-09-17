//! Health checks (`docs/plan/04-acp-engines.md` §9).
//!
//! The question a health check answers is "would this engine work if the user
//! pressed Send right now", and the answer has to arrive quickly and without
//! spending the user's subscription. So the default check stops at
//! `session/new`: that already establishes installed, responding and signed
//! in. Only a deep check sends a prompt, and only on the occasions §9 lists —
//! the first run for an engine, "Check again", a new version.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eavery_core::engine::{Engine, EngineError};
use eavery_core::event::Decision;
use eavery_core::model::{EngineStatus, SessionMode};

use crate::discovery::{NotInstalled, Resolved, Resolver};
use crate::instructions;
use crate::spec::EngineSpec;

/// How long a deep check waits for the model to answer.
const DEEP_PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `Ready` answer stays good for. §9: never run the deep check for
/// every engine at every launch, and do not re-check a ready engine on every
/// screen.
pub const CACHE_TTL: Duration = Duration::from_secs(600);

/// The one-word prompt a deep check sends.
const DEEP_PROMPT: &str = "Reply with the single word OK.";

#[derive(Clone, Debug, Default)]
pub struct HealthOptions {
    /// Send a prompt as well. Off by default: it costs the user money and a
    /// wait, and answers a question the shallow check has usually answered.
    pub deep: bool,
    /// Arguments appended to the engine's own, for engines that need one at
    /// run time — the fake engine's `--script`.
    pub extra_args: Vec<String>,
    /// Extra environment for the child: a provider key, a model name.
    pub env: Vec<(String, String)>,
}

impl HealthOptions {
    pub fn deep() -> Self {
        Self {
            deep: true,
            ..Self::default()
        }
    }
}

/// Runs the check and reports what the user would see.
///
/// Never returns an error: every outcome is a state with copy attached
/// (`07-ui-vocabulary.md` §5). A health check that fails is information, not a
/// failure of Eavery.
pub async fn run_health_check(
    spec: &EngineSpec,
    resolver: &Resolver,
    options: &HealthOptions,
) -> EngineStatus {
    match resolver.resolve(spec) {
        Ok(resolved) => check_resolved(spec, &resolved, options).await,
        Err(error) => not_installed(spec, error),
    }
}

/// The same check, for a caller that has already resolved the executable and
/// wants to report where it found it. Resolution touches the disk once per
/// directory on the search path, and the Settings screen lists every engine.
pub async fn check_resolved(
    spec: &EngineSpec,
    resolved: &Resolved,
    options: &HealthOptions,
) -> EngineStatus {
    // The adapter drives a CLI whose login is the whole point of it. Without
    // that CLI there is nothing to sign in as, and a handshake would only
    // spend fifteen seconds arriving at the same answer
    // (`08-onboarding-packaging.md` §2).
    if resolved.companion_missing
        && let Some(command) = spec.sign_in_command
    {
        return EngineStatus::NeedsSignIn {
            command: command.to_owned(),
        };
    }
    spawn_and_check(spec, resolved, options).await
}

/// The status for an engine that could not be found at all.
pub fn not_installed(spec: &EngineSpec, error: NotInstalled) -> EngineStatus {
    if error.needs_node {
        tracing::info!(
            engine = spec.id,
            "the adapter needs Node and there is no npx"
        );
        return EngineStatus::NeedsNode;
    }
    EngineStatus::NotInstalled {
        instructions: spec.sign_in_instructions.to_owned(),
        searched: error.searched,
    }
}

async fn spawn_and_check(
    spec: &EngineSpec,
    resolved: &Resolved,
    options: &HealthOptions,
) -> EngineStatus {
    let workspace = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(error) => {
            return EngineStatus::Unavailable {
                reason: format!("could not make a temporary folder to test in: {error}"),
            };
        }
    };

    let mut launch = resolved.launch_spec().cwd(workspace.path());
    launch.args.extend(options.extra_args.iter().cloned());
    launch.env.extend(options.env.iter().cloned());

    let engine = eavery_acp::AcpEngine::new(launch);
    let status = check_with(spec, &engine, workspace.path(), options).await;
    engine.shutdown().await;
    status
}

async fn check_with(
    spec: &EngineSpec,
    engine: &eavery_acp::AcpEngine,
    workspace: &std::path::Path,
    options: &HealthOptions,
) -> EngineStatus {
    // Step 2: spawn and initialize. `AcpEngine` applies the 15 second
    // handshake timeout §9 asks for.
    let info = match engine.start().await {
        Ok(info) => info,
        Err(error) => return failed_to_start(spec, error),
    };

    // Step 4: a session in a temporary folder. This is where an engine that
    // has never been signed in says so.
    let session = match engine.open_session(workspace, &[], None).await {
        Ok(session) => session,
        Err(error) => {
            if !info.auth_methods.is_empty()
                && let Some(command) = spec.sign_in_command
            {
                tracing::info!(
                    engine = spec.id,
                    auth_methods = ?info.auth_methods,
                    "the engine wants a sign-in"
                );
                return EngineStatus::NeedsSignIn {
                    command: command.to_owned(),
                };
            }
            return EngineStatus::Unavailable {
                reason: format!("could not open a session: {error}"),
            };
        }
    };

    if options.deep
        && let Some(reason) = deep_prompt(engine, &session.session_id).await
    {
        return EngineStatus::Unavailable { reason };
    }

    ready(spec, info, session.modes, session.current_mode)
}

fn ready(
    spec: &EngineSpec,
    info: eavery_core::model::EngineInfo,
    modes: Vec<SessionMode>,
    current_mode: Option<String>,
) -> EngineStatus {
    if let Some(hint) = spec.plan_mode_hint
        && crate::spec::pick_mode(&modes, Some(hint)).is_none()
    {
        // Not fatal — the client-side plan gate holds either way (`06` §2.1) —
        // but it is exactly the fact M1-T05 and M1-T06 exist to record.
        tracing::warn!(
            engine = spec.id,
            hint,
            modes = ?modes.iter().map(|mode| &mode.id).collect::<Vec<_>>(),
            "the engine offers no mode matching the plan-mode hint"
        );
    }
    EngineStatus::Ready {
        info,
        modes,
        current_mode,
    }
}

/// Step 5's deep half: one prompt, a minute to answer, cancelled if it runs
/// long. Returns a reason when it did not work, and nothing when it did.
async fn deep_prompt(engine: &eavery_acp::AcpEngine, session: &str) -> Option<String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // The events are not the point of the check; the answer arriving is.
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    // A health check must never approve anything: nobody is watching, and the
    // engine is being asked to say one word.
    let permission = Arc::new(|_view| {
        Box::pin(async { Decision::RejectOnce }) as futures::future::BoxFuture<'static, Decision>
    });

    match tokio::time::timeout(
        DEEP_PROMPT_TIMEOUT,
        engine.prompt(session, DEEP_PROMPT, tx, permission),
    )
    .await
    {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(format!("{error}")),
        Err(_) => {
            // §9: cancel if it is still running. The outstanding prompt is
            // left to return on its own; `shutdown` ends the process either
            // way.
            let _ = engine.cancel(session).await;
            Some(format!(
                "did not answer within {}s",
                DEEP_PROMPT_TIMEOUT.as_secs()
            ))
        }
    }
}

fn failed_to_start(spec: &EngineSpec, error: EngineError) -> EngineStatus {
    match &error {
        EngineError::Spawn { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            EngineStatus::NotInstalled {
                instructions: spec.sign_in_instructions.to_owned(),
                searched: Vec::new(),
            }
        }
        EngineError::NotInstalled { searched, .. } => EngineStatus::NotInstalled {
            instructions: spec.sign_in_instructions.to_owned(),
            searched: searched.clone(),
        },
        EngineError::NeedsSignIn { command, .. } => EngineStatus::NeedsSignIn {
            command: command.clone(),
        },
        EngineError::Crashed { stderr_tail, .. } => EngineStatus::Unavailable {
            reason: match stderr_tail.last() {
                Some(last) => format!("{error} — {last}"),
                None => error.to_string(),
            },
        },
        _ => EngineStatus::Unavailable {
            reason: error.to_string(),
        },
    }
}

/// The everyday sentence for a status (`docs/plan/07-ui-vocabulary.md` §5).
///
/// It lives here rather than in the UI because the CLI and the desktop app
/// have to say the same thing, and because §5 is a table of copy, not of
/// components. The UI still renders it through `t()`.
pub fn describe(spec: &EngineSpec, status: &EngineStatus) -> String {
    let engine = spec.display_name;
    match status {
        EngineStatus::NotInstalled { instructions, .. } => {
            format!("{engine} isn't installed on this computer. {instructions}")
        }
        EngineStatus::NeedsNode => {
            format!(
                "{engine} needs Node.js, which isn't installed. {}",
                instructions::NEEDS_NODE
            )
        }
        EngineStatus::NeedsSignIn { command } => format!(
            "{engine} needs you to sign in. Open Terminal and run `{command}`, then check again."
        ),
        EngineStatus::Installing { percent } => format!("Downloading {engine}… {percent}%"),
        EngineStatus::SigningIn => {
            format!("Finish signing in to {engine} in your browser, then come back here.")
        }
        EngineStatus::Ready { .. } => "Ready".to_owned(),
        EngineStatus::Unavailable { .. } => {
            format!("{engine} isn't available right now. You can switch to another assistant.")
        }
    }
}

/// Remembers health checks for [`CACHE_TTL`], so opening Settings does not
/// re-spawn every engine on the machine.
#[derive(Debug, Default)]
pub struct HealthCache {
    entries: tokio::sync::Mutex<HashMap<String, Cached>>,
}

#[derive(Clone, Debug)]
struct Cached {
    status: EngineStatus,
    deep: bool,
    at: Instant,
}

impl HealthCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached answer when there is a fresh one, otherwise a new check.
    ///
    /// A deep request is only satisfied by a deep answer; a shallow one takes
    /// whichever is there, because a deep check has already proved everything
    /// a shallow one would.
    pub async fn get_or_run(
        &self,
        spec: &EngineSpec,
        resolver: &Resolver,
        options: &HealthOptions,
    ) -> EngineStatus {
        if let Some(cached) = self.entries.lock().await.get(spec.id)
            && cached.at.elapsed() < CACHE_TTL
            && (cached.deep || !options.deep)
        {
            return cached.status.clone();
        }

        let status = run_health_check(spec, resolver, options).await;
        self.entries.lock().await.insert(
            spec.id.to_owned(),
            Cached {
                status: status.clone(),
                deep: options.deep,
                at: Instant::now(),
            },
        );
        status
    }

    /// Forgets one engine — after an install, a sign-in, or "Check again".
    pub async fn invalidate(&self, engine_id: &str) {
        self.entries.lock().await.remove(engine_id);
    }

    pub async fn clear(&self) {
        self.entries.lock().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec;

    #[test]
    fn every_status_has_copy_and_never_says_the_engine_id() {
        let engine = spec::find("claude").unwrap();
        let statuses = [
            EngineStatus::NotInstalled {
                instructions: "Install it.".to_owned(),
                searched: vec![],
            },
            EngineStatus::NeedsNode,
            EngineStatus::NeedsSignIn {
                command: "claude".to_owned(),
            },
            EngineStatus::Installing { percent: 40 },
            EngineStatus::SigningIn,
            EngineStatus::Unavailable {
                reason: "exited with 1".to_owned(),
            },
        ];
        for status in statuses {
            let copy = describe(engine, &status);
            assert!(!copy.is_empty());
            assert!(
                copy.contains(engine.display_name),
                "{copy} does not name the engine"
            );
        }
    }

    /// The developer-facing reason belongs in Diagnostics, not in the sentence
    /// a user reads (`07` §5).
    #[test]
    fn the_unavailable_reason_stays_out_of_the_everyday_copy() {
        let engine = spec::find("goose").unwrap();
        let copy = describe(
            engine,
            &EngineStatus::Unavailable {
                reason: "thread 'main' panicked at src/main.rs:1".to_owned(),
            },
        );
        assert!(!copy.contains("panicked"), "{copy}");
    }
}
