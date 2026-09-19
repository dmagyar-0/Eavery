//! The engine table (`docs/plan/04-acp-engines.md` §2–3). Data, not code.
//!
//! One row per engine. Everything that varies between engines — how to find
//! it, whether it needs Node, which mode is its most restrictive — is a field
//! here, so the rest of Eavery never branches on an engine id.

use eavery_core::engine::EngineFacts;

use crate::instructions;

/// How the user proves who they are to the engine's provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    /// The engine's own login, done once in its CLI or by Eavery spawning it
    /// (`codex login`). Eavery never sees the credential.
    OwnLogin,
    /// A key the user pastes, kept in the OS keychain and passed to the child
    /// as an environment variable (M7-T03).
    ApiKey,
    /// Nothing leaves the machine, so there is nothing to sign in to.
    Local,
    None,
}

/// Where the executable comes from (`docs/plan/08-onboarding-packaging.md` §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineSource {
    /// Shipped inside the installer as a Tauri sidecar.
    Bundled,
    /// Downloaded by Eavery, checksummed, and run from the data directory.
    /// `url_pattern` takes `{version}` and `{target}`.
    Download {
        url_pattern: &'static str,
        sha256: &'static [(&'static str, &'static str)],
    },
    /// The user installs it themselves; Eavery only finds it.
    UserInstalled,
}

/// How to find and start an engine. The concrete
/// [`eavery_acp::LaunchSpec`](eavery_acp::LaunchSpec) is what
/// [`crate::discovery`] produces from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launch {
    /// Executable names to look for, in order of preference. On Windows each
    /// is tried with the usual extensions (`.exe`, `.cmd`, `.bat`).
    pub binaries: &'static [&'static str],
    /// Arguments that always follow, whichever way the engine was found.
    pub args: &'static [&'static str],
    /// The npm package providing the adapter, used only when none of
    /// `binaries` is installed: `npx -y <package>`. Implies Node.
    pub npm_package: Option<&'static str>,
    /// The CLI the adapter drives (`claude`, `codex`, `gemini`). The adapter
    /// is useless without it, because the login lives there
    /// (`08-onboarding-packaging.md` §2).
    pub companion_cli: Option<&'static str>,
    /// Environment for the child only, never written into the engine's own
    /// config (working rule 8). Runtime values (model, keys) are added on top.
    pub env: &'static [(&'static str, &'static str)],
}

impl Launch {
    const fn new(binaries: &'static [&'static str]) -> Self {
        Self {
            binaries,
            args: &[],
            npm_package: None,
            companion_cli: None,
            env: &[],
        }
    }
}

/// One engine Eavery can drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub auth_kind: AuthKind,
    pub launch: Launch,
    /// Mode id substring for the execute phase: the mode that asks before it
    /// acts. Matched by [`pick_mode`].
    pub asking_mode_hint: Option<&'static str>,
    /// Mode id substring for the plan phase: the most restrictive mode the
    /// engine offers.
    pub plan_mode_hint: Option<&'static str>,
    /// Tool titles and `rawInput` markers that mean "leave plan mode". The
    /// plan gate refuses them (`06-plan-gate-permissions.md` §2.2).
    pub plan_exit_signatures: &'static [&'static str],
    pub source: EngineSource,
    pub needs_node: bool,
    /// Shown on the plan card: "Your documents are sent to {vendor}".
    pub vendor: &'static str,
    pub sign_in_instructions: &'static str,
    /// The one command the `NeedsSignIn` screen shows verbatim
    /// (`07-ui-vocabulary.md` §5). `None` for engines that are not signed in
    /// at all: a pasted key, or a model on this computer.
    pub sign_in_command: Option<&'static str>,
    /// Hidden behind Developer mode: known to be unreliable on the versions
    /// tested (M1-T07).
    pub experimental: bool,
    /// Never offered outside a development build (the fake engine).
    pub developer_only: bool,
}

impl EngineSpec {
    /// Whether this engine may be offered to a user at all. The fake engine is
    /// for tests, and a release build hides it unless `EAVERY_DEV=1`
    /// (`04-acp-engines.md` §3).
    pub fn visible(&self) -> bool {
        if !self.developer_only {
            return true;
        }
        cfg!(debug_assertions) || std::env::var_os("EAVERY_DEV").is_some_and(|value| value == "1")
    }

    /// Whether Eavery can install this engine itself, or has to send the user
    /// to the instructions.
    pub fn installable(&self) -> bool {
        matches!(
            self.source,
            EngineSource::Bundled | EngineSource::Download { .. }
        )
    }
}

/// Every engine, in the order they are offered.
pub const ENGINES: &[EngineSpec] = &[
    EngineSpec {
        id: "codex",
        display_name: "ChatGPT (Codex)",
        auth_kind: AuthKind::OwnLogin,
        launch: Launch {
            companion_cli: Some("codex"),
            ..Launch::new(&["codex-acp"])
        },
        // Codex has no "plan" mode; its modes are sandbox levels. Read-only is
        // the strongest plan-phase guarantee it offers, and workspace-write
        // with approval-on-request is the executing one. Exact ids are
        // recorded in M1-T06.
        asking_mode_hint: Some("workspace-write"),
        plan_mode_hint: Some("read-only"),
        plan_exit_signatures: &[],
        // Flipped to `Download` by M7-T02b, once a version and its checksums
        // are pinned in `releases.rs`.
        source: EngineSource::UserInstalled,
        needs_node: false,
        vendor: "OpenAI",
        sign_in_instructions: instructions::CODEX,
        sign_in_command: Some("codex login"),
        experimental: false,
        developer_only: false,
    },
    EngineSpec {
        id: "claude",
        display_name: "Claude Code",
        auth_kind: AuthKind::OwnLogin,
        launch: Launch {
            npm_package: Some("@agentclientprotocol/claude-agent-acp"),
            companion_cli: Some("claude"),
            ..Launch::new(&["claude-agent-acp"])
        },
        asking_mode_hint: Some("default"),
        plan_mode_hint: Some("plan"),
        // Leaving plan mode arrives as a tool call, probably with kind
        // `other`. M1-T05 records what it actually looks like.
        plan_exit_signatures: &["ExitPlanMode", "exit_plan_mode"],
        source: EngineSource::UserInstalled,
        needs_node: true,
        vendor: "Anthropic",
        sign_in_instructions: instructions::CLAUDE,
        sign_in_command: Some("claude"),
        experimental: false,
        developer_only: false,
    },
    EngineSpec {
        id: "goose",
        display_name: "goose",
        auth_kind: AuthKind::ApiKey,
        launch: Launch {
            args: &["acp"],
            ..Launch::new(&["goose"])
        },
        asking_mode_hint: None,
        plan_mode_hint: None,
        plan_exit_signatures: &[],
        source: EngineSource::UserInstalled,
        needs_node: false,
        vendor: "your provider",
        sign_in_instructions: instructions::GOOSE,
        sign_in_command: None,
        experimental: false,
        developer_only: false,
    },
    EngineSpec {
        id: "goose-local",
        display_name: "On this computer (Ollama)",
        auth_kind: AuthKind::Local,
        launch: Launch {
            args: &["acp"],
            // The model is chosen from what Ollama reports (M7-T04) and added
            // on top of these at launch.
            env: &[
                ("GOOSE_PROVIDER", "ollama"),
                ("OLLAMA_HOST", "http://localhost:11434"),
            ],
            ..Launch::new(&["goose"])
        },
        asking_mode_hint: None,
        plan_mode_hint: None,
        plan_exit_signatures: &[],
        source: EngineSource::UserInstalled,
        needs_node: false,
        vendor: "local",
        sign_in_instructions: instructions::GOOSE_LOCAL,
        sign_in_command: None,
        experimental: false,
        developer_only: false,
    },
    EngineSpec {
        id: "gemini",
        display_name: "Gemini",
        auth_kind: AuthKind::OwnLogin,
        launch: Launch {
            // Without the flag it starts its interactive UI and hangs.
            args: &["--experimental-acp"],
            npm_package: Some("@google/gemini-cli"),
            companion_cli: Some("gemini"),
            ..Launch::new(&["gemini"])
        },
        asking_mode_hint: None,
        plan_mode_hint: None,
        plan_exit_signatures: &[],
        source: EngineSource::UserInstalled,
        needs_node: true,
        vendor: "Google",
        sign_in_instructions: instructions::GEMINI,
        sign_in_command: Some("gemini"),
        // Flaky across Gemini CLI versions; M1-T07 either clears this or
        // records why it stays.
        experimental: true,
        developer_only: false,
    },
    EngineSpec {
        id: "fake",
        display_name: "Fake engine (tests)",
        auth_kind: AuthKind::None,
        launch: Launch::new(&["eavery-fake-agent"]),
        asking_mode_hint: Some("work"),
        plan_mode_hint: Some("plan"),
        plan_exit_signatures: &["ExitPlanMode"],
        source: EngineSource::Bundled,
        needs_node: false,
        vendor: "local",
        sign_in_instructions: instructions::FAKE,
        sign_in_command: None,
        experimental: false,
        developer_only: true,
    },
];

/// The engine with this id, whether or not it is visible.
pub fn find(id: &str) -> Option<&'static EngineSpec> {
    ENGINES.iter().find(|spec| spec.id == id)
}

/// The engines that may be offered in this build.
pub fn visible() -> impl Iterator<Item = &'static EngineSpec> {
    ENGINES.iter().filter(|spec| spec.visible())
}

pub use eavery_core::engine::pick_mode;

impl EngineSpec {
    /// What the turn engine needs to know about this engine beyond the
    /// `Engine` trait: the vendor for the plan card, and the mode hints and
    /// exit signatures the plan gate works from.
    pub fn facts(&self) -> EngineFacts {
        EngineFacts {
            vendor: self.vendor.to_owned(),
            plan_mode_hint: self.plan_mode_hint.map(str::to_owned),
            asking_mode_hint: self.asking_mode_hint.map(str::to_owned),
            plan_exit_signatures: self
                .plan_exit_signatures
                .iter()
                .map(|signature| (*signature).to_owned())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eavery_core::model::SessionMode;

    fn mode(id: &str) -> SessionMode {
        SessionMode {
            id: id.to_owned(),
            name: id.to_owned(),
            description: None,
        }
    }

    #[test]
    fn every_engine_id_is_unique() {
        let mut ids: Vec<&str> = ENGINES.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "two engines share an id");
    }

    /// An engine with no way to be found is an engine that can never start.
    #[test]
    fn every_engine_has_something_to_launch() {
        for spec in ENGINES {
            assert!(
                !spec.launch.binaries.is_empty() || spec.launch.npm_package.is_some(),
                "{} has no binary and no npm package",
                spec.id
            );
            assert!(
                !spec.sign_in_instructions.is_empty(),
                "{} has no sign-in instructions",
                spec.id
            );
        }
    }

    /// `needs_node` is what the onboarding flow branches on, so it has to agree
    /// with the launch row rather than be maintained beside it.
    #[test]
    fn engines_launched_through_npm_need_node() {
        for spec in ENGINES {
            if spec.launch.npm_package.is_some() {
                assert!(
                    spec.needs_node,
                    "{} runs through npx but is not marked as needing Node",
                    spec.id
                );
            }
        }
    }

    /// `NeedsSignIn` shows one command verbatim (`07-ui-vocabulary.md` §5). An
    /// engine that signs in through its own CLI and has no command to show
    /// leaves that screen with nothing on it.
    #[test]
    fn every_engine_with_its_own_login_has_a_command_to_show() {
        for spec in ENGINES {
            if spec.auth_kind == AuthKind::OwnLogin {
                assert!(
                    spec.sign_in_command.is_some(),
                    "{} signs in through its own CLI but names no command",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn the_fake_engine_is_the_only_developer_only_one() {
        let developer_only: Vec<&str> = ENGINES
            .iter()
            .filter(|spec| spec.developer_only)
            .map(|spec| spec.id)
            .collect();
        assert_eq!(developer_only, vec!["fake"]);
    }

    #[test]
    fn a_mode_hint_matches_across_separator_spelling() {
        let modes = vec![mode("read_only"), mode("workspace-write")];
        assert_eq!(
            pick_mode(&modes, Some("read-only")).map(|mode| mode.id.as_str()),
            Some("read_only")
        );
        assert_eq!(
            pick_mode(&modes, Some("workspace write")).map(|mode| mode.id.as_str()),
            Some("workspace-write")
        );
    }

    #[test]
    fn an_exact_mode_id_wins_over_a_longer_one() {
        let modes = vec![mode("planning"), mode("plan")];
        assert_eq!(
            pick_mode(&modes, Some("plan")).map(|mode| mode.id.as_str()),
            Some("plan")
        );
    }

    /// A hint that matches nothing leaves the engine in whichever mode it
    /// started in. The client-side plan gate holds regardless (`06` §2.1), so
    /// this must be a `None`, not a panic and not a wrong guess.
    #[test]
    fn a_hint_that_matches_nothing_picks_nothing() {
        let modes = vec![mode("auto")];
        assert!(pick_mode(&modes, Some("plan")).is_none());
        assert!(pick_mode(&modes, None).is_none());
        assert!(pick_mode(&[], Some("plan")).is_none());
    }

    #[test]
    fn engines_are_found_by_id() {
        assert_eq!(find("codex").map(|spec| spec.vendor), Some("OpenAI"));
        assert!(find("no-such-engine").is_none());
    }
}
