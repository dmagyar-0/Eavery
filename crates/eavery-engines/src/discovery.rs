//! Finding an engine's executable (`docs/plan/08-onboarding-packaging.md` §2).
//!
//! The search order is fixed: an explicit path from Settings, then the
//! effective PATH (`docs/plan/02-challenges.md` C5), then the well-known
//! locations for the platform, then — for the adapters that ship on npm —
//! `npx`. Whatever happens, the list of places looked in comes back with the
//! answer: a user is never told "not installed" without being told where
//! Eavery looked.
//!
//! [`Platform`] is a parameter rather than a `cfg!`. Path handling bugs are
//! the ones that only show up on the OS nobody develops on, and a Windows rule
//! that can only be tested on Windows is a rule that gets tested once a
//! release.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eavery_acp::LaunchSpec;

use crate::path_env;
use crate::spec::EngineSpec;

/// Which set of rules to apply. `Platform::current()` is the real one; tests
/// name the other two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// Extensions to try, in order. `.exe` before `.cmd` so a real binary wins
    /// over a shim that would need `cmd` to run it.
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Platform::Windows => &[".exe", ".cmd", ".bat", ""],
            _ => &[""],
        }
    }

    /// Whether this program has to be handed to `cmd /C` rather than spawned
    /// (`04-acp-engines.md` §3): a `.cmd` shim is a batch file, not an image
    /// the loader can start.
    fn needs_cmd_shell(self, program: &Path) -> bool {
        self == Platform::Windows
            && program
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
                })
    }
}

/// Everything about the machine that discovery depends on. Built from the
/// process in [`Environment::current`], or by hand in tests.
#[derive(Clone, Debug)]
pub struct Environment {
    pub platform: Platform,
    /// The effective PATH, already fixed up by [`crate::path_env`].
    pub path: Vec<PathBuf>,
    pub home: Option<PathBuf>,
    /// `LOCALAPPDATA`, `APPDATA`, `USERPROFILE`, `ProgramFiles`.
    pub vars: BTreeMap<String, PathBuf>,
    /// Where the running Eavery binary lives. Bundled engines — and the fake
    /// agent, in a development build — sit beside it.
    pub beside_exe: Option<PathBuf>,
    /// Whether the PATH has to be passed to children explicitly, because it is
    /// not the one this process inherited.
    pub path_is_fixed: bool,
    /// Whether to look in the platform's fixed system directories
    /// (`/opt/homebrew/bin`, `/snap/bin`, …). A test that asserts an engine is
    /// *absent* has to be able to turn these off, or it passes or fails
    /// depending on what the machine running it happens to have installed.
    pub system_locations: bool,
}

impl Environment {
    pub fn current() -> Self {
        let effective = path_env::effective_path();
        let mut vars = BTreeMap::new();
        for name in ["LOCALAPPDATA", "APPDATA", "USERPROFILE", "ProgramFiles"] {
            if let Some(value) = std::env::var_os(name) {
                vars.insert(name.to_owned(), PathBuf::from(value));
            }
        }
        Self {
            platform: Platform::current(),
            path: effective.entries().to_vec(),
            home: home_dir(),
            vars,
            beside_exe: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(PathBuf::from)),
            path_is_fixed: effective.differs_from_process(),
            system_locations: true,
        }
    }

    /// A bare environment with nothing in it, for tests to fill in.
    pub fn empty(platform: Platform) -> Self {
        Self {
            platform,
            path: Vec::new(),
            home: None,
            vars: BTreeMap::new(),
            beside_exe: None,
            path_is_fixed: false,
            system_locations: false,
        }
    }

    /// The well-known locations for this platform, in the order of the table in
    /// `08-onboarding-packaging.md` §2, with `*` expanded against the disk.
    pub fn well_known(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        // Anything Eavery ships or downloads itself is found first: it is the
        // version Eavery knows the shape of.
        if let Some(beside) = &self.beside_exe {
            dirs.push(beside.clone());
        }

        match self.platform {
            Platform::MacOs => {
                self.push_system(&mut dirs, &["/opt/homebrew/bin", "/usr/local/bin"]);
                self.push_home(&mut dirs, &[".local/bin", ".claude/local", ".volta/bin"]);
                self.push_node_versions(&mut dirs);
                self.push_home(&mut dirs, &[".cargo/bin", ".bun/bin"]);
            }
            Platform::Linux => {
                self.push_system(&mut dirs, &["/usr/local/bin"]);
                self.push_home(&mut dirs, &[".local/bin", ".claude/local"]);
                self.push_node_versions(&mut dirs);
                self.push_home(&mut dirs, &[".volta/bin", ".cargo/bin"]);
                self.push_system(&mut dirs, &["/snap/bin"]);
            }
            Platform::Windows => {
                if let Some(local) = self.vars.get("LOCALAPPDATA") {
                    // `%LOCALAPPDATA%\Programs\*`: one directory per program.
                    dirs.extend(children_of(&local.join("Programs")));
                    dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
                }
                if let Some(appdata) = self.vars.get("APPDATA") {
                    dirs.push(appdata.join("npm"));
                }
                if let Some(profile) = self.vars.get("USERPROFILE") {
                    dirs.push(profile.join(".claude").join("local"));
                    dirs.push(profile.join(".cargo").join("bin"));
                }
                if let Some(program_files) = self.vars.get("ProgramFiles") {
                    dirs.push(program_files.join("nodejs"));
                }
            }
        }
        dirs
    }

    fn push_system(&self, dirs: &mut Vec<PathBuf>, absolute: &[&str]) {
        if !self.system_locations {
            return;
        }
        dirs.extend(absolute.iter().map(PathBuf::from));
    }

    fn push_home(&self, dirs: &mut Vec<PathBuf>, relative: &[&str]) {
        let Some(home) = &self.home else { return };
        for entry in relative {
            dirs.push(home.join(entry));
        }
    }

    /// `~/.nvm/versions/node/*/bin`: one directory per installed Node version.
    fn push_node_versions(&self, dirs: &mut Vec<PathBuf>) {
        let Some(home) = &self.home else { return };
        for version in children_of(&home.join(".nvm").join("versions").join("node")) {
            dirs.push(version.join("bin"));
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// The subdirectories of `dir`, sorted, or nothing if it cannot be read.
fn children_of(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    children.sort();
    children
}

/// How an engine was found. Shown in Diagnostics, because "it started the wrong
/// one" is otherwise impossible to see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchVia {
    /// A path the user set in Settings.
    ExplicitPath,
    Path,
    WellKnown,
    /// Not installed, but Node is: `npx -y <package>`. The first start
    /// downloads the package, which takes a minute.
    Npx,
}

/// A found engine, ready to be turned into a [`LaunchSpec`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub engine_id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub via: LaunchVia,
    /// Everywhere that was looked, whether or not anything was found there.
    pub searched: Vec<String>,
    /// The CLI the adapter drives (`claude`, `codex`, `gemini`), when it is
    /// installed. Without it the adapter starts and then has no login.
    pub companion: Option<PathBuf>,
    /// The adapter needs a companion CLI and it is not installed.
    pub companion_missing: bool,
}

impl Resolved {
    pub fn launch_spec(&self) -> LaunchSpec {
        let mut spec = LaunchSpec::new(self.engine_id.clone(), self.program.clone())
            .args(self.args.iter().cloned());
        spec.env = self.env.clone();
        spec
    }
}

/// Why an engine cannot be started, with everywhere that was looked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotInstalled {
    pub engine_id: String,
    pub searched: Vec<String>,
    /// The engine only ships as an npm package and there is no `npx` to run it
    /// with. This is a different screen from "not installed"
    /// (`07-ui-vocabulary.md` §5).
    pub needs_node: bool,
}

/// Resolves engine executables against one [`Environment`].
#[derive(Clone, Debug)]
pub struct Resolver {
    env: Environment,
    explicit: BTreeMap<String, PathBuf>,
}

impl Resolver {
    pub fn new(env: Environment) -> Self {
        Self {
            env,
            explicit: BTreeMap::new(),
        }
    }

    pub fn current() -> Self {
        Self::new(Environment::current())
    }

    /// `settings.engine_paths[id]`, which beats everything else.
    #[must_use]
    pub fn with_explicit_path(
        mut self,
        engine_id: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        self.explicit.insert(engine_id.into(), path.into());
        self
    }

    pub fn environment(&self) -> &Environment {
        &self.env
    }

    /// The directories searched, in order: the PATH first, then the well-known
    /// locations.
    fn search_dirs(&self) -> Vec<(PathBuf, LaunchVia)> {
        let mut dirs: Vec<(PathBuf, LaunchVia)> = self
            .env
            .path
            .iter()
            .cloned()
            .map(|dir| (dir, LaunchVia::Path))
            .collect();
        dirs.extend(
            self.env
                .well_known()
                .into_iter()
                .map(|dir| (dir, LaunchVia::WellKnown)),
        );
        dirs.retain(|(dir, _)| !dir.as_os_str().is_empty());
        dirs
    }

    /// Finds `name` anywhere on the search path.
    pub fn find_program(&self, name: &str) -> Option<PathBuf> {
        self.search_dirs()
            .iter()
            .find_map(|(dir, _)| executable_in(dir, name, self.env.platform))
    }

    pub fn resolve(&self, spec: &EngineSpec) -> Result<Resolved, NotInstalled> {
        let dirs = self.search_dirs();
        let mut searched: Vec<String> = Vec::new();

        if let Some(explicit) = self.explicit.get(spec.id) {
            searched.push(explicit.display().to_string());
            if is_executable(explicit, self.env.platform) {
                return Ok(self.build(
                    spec,
                    explicit.clone(),
                    Vec::new(),
                    LaunchVia::ExplicitPath,
                    searched,
                ));
            }
            tracing::warn!(
                engine = spec.id,
                path = %explicit.display(),
                "the path set in Settings is not an executable; falling back to the usual search"
            );
        }

        searched.extend(dirs.iter().map(|(dir, _)| dir.display().to_string()));

        for name in spec.launch.binaries {
            for (dir, via) in &dirs {
                if let Some(program) = executable_in(dir, name, self.env.platform) {
                    return Ok(self.build(spec, program, Vec::new(), *via, searched));
                }
            }
        }

        // Not installed as a binary. The npm adapters can still be run through
        // `npx`, at the cost of a download on first start.
        if let Some(package) = spec.launch.npm_package {
            if let Some(npx) = self.find_program("npx") {
                tracing::info!(
                    engine = spec.id,
                    package,
                    "running the adapter through npx; the first start downloads it"
                );
                let args = vec!["-y".to_owned(), package.to_owned()];
                return Ok(self.build(spec, npx, args, LaunchVia::Npx, searched));
            }
            return Err(NotInstalled {
                engine_id: spec.id.to_owned(),
                searched,
                needs_node: true,
            });
        }

        Err(NotInstalled {
            engine_id: spec.id.to_owned(),
            searched,
            needs_node: false,
        })
    }

    /// Assembles the command line, applying the two platform rules: a `.cmd`
    /// shim goes through `cmd /C`, and a child gets the fixed PATH when the
    /// process's own is not the one Eavery searched.
    fn build(
        &self,
        spec: &EngineSpec,
        program: PathBuf,
        mut leading_args: Vec<String>,
        via: LaunchVia,
        searched: Vec<String>,
    ) -> Resolved {
        leading_args.extend(spec.launch.args.iter().map(|arg| (*arg).to_owned()));

        let (program, args) = if self.env.platform.needs_cmd_shell(&program) {
            // `cmd /C <shim> <args>`. Spawning a batch file directly is not
            // something the loader can do.
            let mut args = vec!["/C".to_owned(), program.to_string_lossy().into_owned()];
            args.extend(leading_args);
            (PathBuf::from("cmd"), args)
        } else {
            (program, leading_args)
        };

        let mut env: Vec<(String, String)> = spec
            .launch
            .env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        if self.env.path_is_fixed {
            env.push(("PATH".to_owned(), path_env::effective_path().as_env_value()));
        }

        let companion = spec
            .launch
            .companion_cli
            .and_then(|name| self.find_program(name));
        Resolved {
            engine_id: spec.id.to_owned(),
            program,
            args,
            env,
            via,
            searched,
            companion_missing: spec.launch.companion_cli.is_some() && companion.is_none(),
            companion,
        }
    }
}

/// `name` in `dir`, with the platform's extensions tried in order.
fn executable_in(dir: &Path, name: &str, platform: Platform) -> Option<PathBuf> {
    for extension in platform.extensions() {
        let candidate = dir.join(format!("{name}{extension}"));
        if is_executable(&candidate, platform) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path, platform: Platform) -> bool {
    if !path.is_file() {
        return false;
    }
    if platform == Platform::Windows {
        // Windows decides by extension, which `executable_in` has already
        // applied.
        return true;
    }
    executable_bit(path)
}

#[cfg(unix)]
fn executable_bit(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// A Unix-rules search running on Windows — which is what a `Platform::Linux`
/// test does in CI — has no bit to look at. Being a file is as far as it goes.
#[cfg(not(unix))]
fn executable_bit(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec;

    /// Writes a file that the platform under test will accept as an
    /// executable: on Unix that means the bit is set, on Windows the name is
    /// what matters.
    fn put_executable(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("make the directory");
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").expect("write the executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("set the executable bit");
        }
        path
    }

    fn env_with_path(platform: Platform, dirs: &[&Path]) -> Environment {
        let mut env = Environment::empty(platform);
        env.path = dirs.iter().map(|dir| dir.to_path_buf()).collect();
        env
    }

    fn engine(id: &str) -> &'static EngineSpec {
        spec::find(id).expect("the engine table has this engine")
    }

    #[test]
    fn a_binary_on_the_path_is_found_with_its_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let goose = put_executable(dir.path(), "goose");

        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]));
        let resolved = resolver.resolve(engine("goose")).expect("goose is on PATH");

        assert_eq!(resolved.program, goose);
        assert_eq!(resolved.args, vec!["acp".to_owned()]);
        assert_eq!(resolved.via, LaunchVia::Path);
    }

    #[test]
    fn the_path_set_in_settings_beats_everything_else() {
        let on_path = tempfile::tempdir().unwrap();
        put_executable(on_path.path(), "goose");
        let elsewhere = tempfile::tempdir().unwrap();
        let chosen = put_executable(elsewhere.path(), "goose");

        let resolver = Resolver::new(env_with_path(Platform::Linux, &[on_path.path()]))
            .with_explicit_path("goose", &chosen);
        let resolved = resolver.resolve(engine("goose")).unwrap();

        assert_eq!(resolved.program, chosen);
        assert_eq!(resolved.via, LaunchVia::ExplicitPath);
    }

    /// A path in Settings that has gone stale — the user moved the binary —
    /// must not turn a working engine into a broken one.
    #[test]
    fn a_stale_path_in_settings_falls_back_to_the_search() {
        let dir = tempfile::tempdir().unwrap();
        let goose = put_executable(dir.path(), "goose");

        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]))
            .with_explicit_path("goose", dir.path().join("moved-away"));
        let resolved = resolver.resolve(engine("goose")).unwrap();

        assert_eq!(resolved.program, goose);
        assert_eq!(resolved.via, LaunchVia::Path);
    }

    #[test]
    fn a_well_known_location_is_searched_when_the_path_is_the_finder_one() {
        let home = tempfile::tempdir().unwrap();
        let goose = put_executable(&home.path().join(".local/bin"), "goose");

        let mut env = Environment::empty(Platform::MacOs);
        env.path = vec![PathBuf::from("/usr/bin")];
        env.home = Some(home.path().to_path_buf());

        let resolved = Resolver::new(env).resolve(engine("goose")).unwrap();
        assert_eq!(resolved.program, goose);
        assert_eq!(resolved.via, LaunchVia::WellKnown);
    }

    #[test]
    fn node_versions_under_nvm_are_expanded() {
        let home = tempfile::tempdir().unwrap();
        let adapter = put_executable(
            &home.path().join(".nvm/versions/node/v20.11.0/bin"),
            "claude-agent-acp",
        );
        std::fs::create_dir_all(home.path().join(".nvm/versions/node/v18.0.0/bin")).unwrap();

        let mut env = Environment::empty(Platform::Linux);
        env.home = Some(home.path().to_path_buf());

        let resolved = Resolver::new(env).resolve(engine("claude")).unwrap();
        assert_eq!(resolved.program, adapter);
        assert_eq!(resolved.via, LaunchVia::WellKnown);
    }

    /// The Windows rule from `04-acp-engines.md` §3: `npx` is `npx.cmd`, and a
    /// batch file cannot be spawned — it has to go through `cmd /C`.
    #[test]
    fn on_windows_an_npm_adapter_runs_through_cmd_and_npx_cmd() {
        let dir = tempfile::tempdir().unwrap();
        let npx = put_executable(dir.path(), "npx.cmd");

        let resolver = Resolver::new(env_with_path(Platform::Windows, &[dir.path()]));
        let resolved = resolver.resolve(engine("claude")).unwrap();

        assert_eq!(resolved.program, PathBuf::from("cmd"));
        assert_eq!(
            resolved.args,
            vec![
                "/C".to_owned(),
                npx.to_string_lossy().into_owned(),
                "-y".to_owned(),
                "@agentclientprotocol/claude-agent-acp".to_owned(),
            ]
        );
        assert_eq!(resolved.via, LaunchVia::Npx);
    }

    /// The same machine with the adapter actually installed: an `.exe` is
    /// spawned directly, and `cmd` stays out of it.
    #[test]
    fn on_windows_an_installed_exe_is_spawned_directly() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = put_executable(dir.path(), "claude-agent-acp.exe");
        put_executable(dir.path(), "claude-agent-acp.cmd");

        let resolver = Resolver::new(env_with_path(Platform::Windows, &[dir.path()]));
        let resolved = resolver.resolve(engine("claude")).unwrap();

        assert_eq!(
            resolved.program, adapter,
            "the .exe should win over the .cmd"
        );
        assert!(resolved.args.is_empty());
        assert_eq!(resolved.via, LaunchVia::Path);
    }

    #[test]
    fn windows_well_known_locations_come_from_the_environment() {
        let root = tempfile::tempdir().unwrap();
        let appdata = root.path().join("AppData/Roaming");
        let npx = put_executable(&appdata.join("npm"), "npx.cmd");

        let mut env = Environment::empty(Platform::Windows);
        env.vars.insert("APPDATA".to_owned(), appdata.clone());
        let resolver = Resolver::new(env);

        assert_eq!(resolver.find_program("npx"), Some(npx));
    }

    #[test]
    fn an_npm_adapter_with_no_node_anywhere_asks_for_node() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]));

        let error = resolver.resolve(engine("claude")).unwrap_err();
        assert_eq!(error.engine_id, "claude");
        assert!(error.needs_node, "the claude adapter is an npm package");
    }

    /// An engine that is simply absent is not a Node problem, and must not be
    /// reported as one.
    #[test]
    fn an_absent_binary_engine_does_not_ask_for_node() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]));

        let error = resolver.resolve(engine("goose")).unwrap_err();
        assert!(!error.needs_node);
        assert!(
            error.searched.contains(&dir.path().display().to_string()),
            "the places looked in have to come back with the answer: {:?}",
            error.searched
        );
    }

    /// The adapter without its CLI starts and then has no login
    /// (`08-onboarding-packaging.md` §2), so the gap is reported rather than
    /// discovered at the first prompt.
    #[test]
    fn a_missing_companion_cli_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        put_executable(dir.path(), "codex-acp");

        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]));
        let resolved = resolver.resolve(engine("codex")).unwrap();
        assert!(resolved.companion_missing);
        assert!(resolved.companion.is_none());

        let codex = put_executable(dir.path(), "codex");
        let resolved = resolver.resolve(engine("codex")).unwrap();
        assert!(!resolved.companion_missing);
        assert_eq!(resolved.companion, Some(codex));
    }

    #[test]
    fn a_bundled_engine_is_found_beside_eavery() {
        let dir = tempfile::tempdir().unwrap();
        let fake = put_executable(dir.path(), "eavery-fake-agent");

        let mut env = Environment::empty(Platform::Linux);
        env.beside_exe = Some(dir.path().to_path_buf());

        let resolved = Resolver::new(env).resolve(engine("fake")).unwrap();
        assert_eq!(resolved.program, fake);
    }

    #[test]
    fn a_resolved_engine_carries_its_environment_into_the_launch_spec() {
        let dir = tempfile::tempdir().unwrap();
        put_executable(dir.path(), "goose");

        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]));
        let resolved = resolver.resolve(engine("goose-local")).unwrap();
        let launch = resolved.launch_spec();

        assert_eq!(launch.engine_id, "goose-local");
        assert_eq!(launch.args, vec!["acp".to_owned()]);
        assert!(
            launch
                .env
                .contains(&("GOOSE_PROVIDER".to_owned(), "ollama".to_owned())),
            "the local engine has to tell goose which provider to use: {:?}",
            launch.env
        );
    }

    #[test]
    fn the_platform_tables_keep_their_documented_directories_in_order() {
        let mut env = Environment::empty(Platform::MacOs);
        env.system_locations = true;
        let macos = env.well_known();
        assert_eq!(
            macos,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin")
            ],
            "08 §2 looks in Homebrew's directory before /usr/local"
        );

        let mut env = Environment::empty(Platform::Linux);
        env.system_locations = true;
        assert_eq!(
            env.well_known(),
            vec![PathBuf::from("/usr/local/bin"), PathBuf::from("/snap/bin")],
            "a sandboxed snap is the last resort, not the first"
        );
    }

    /// A file without the executable bit is a file, not a program. Picking it
    /// up produces a spawn failure that reads like a missing engine.
    #[cfg(unix)]
    #[test]
    fn a_file_without_the_executable_bit_is_not_an_engine() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("goose"), "not executable").unwrap();

        let resolver = Resolver::new(env_with_path(Platform::Linux, &[dir.path()]));
        assert!(resolver.resolve(engine("goose")).is_err());
    }
}
