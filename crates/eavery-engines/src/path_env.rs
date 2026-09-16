//! The login-shell PATH probe (`docs/plan/02-challenges.md` C5).
//!
//! An app launched from Finder or the Dock gets
//! `PATH=/usr/bin:/bin:/usr/sbin:/sbin`. Every engine the user has installed
//! lives somewhere else — `/opt/homebrew/bin`, `~/.local/bin`, a Node version
//! manager's shims — so without this, Eavery reports "not installed" for
//! engines that work perfectly well in a terminal.
//!
//! The probe asks the user's login shell what their PATH is, under a three
//! second timeout, once per process. The result is returned as data rather
//! than written back to the process environment: `std::env::set_var` is
//! `unsafe` in edition 2024 (another thread reading the environment at the
//! same moment is undefined behaviour), and this crate denies unsafe code.
//! [`crate::discovery`] searches the effective PATH, and every engine child is
//! launched with it, which is what mutating the process environment was for.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// The plan's budget. A login shell that sources a slow profile must not hold
/// up the first screen.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The PATH Eavery searches and hands to engine children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectivePath {
    entries: Vec<PathBuf>,
    /// How this was arrived at, for the Diagnostics panel (M3-T08).
    pub origin: PathOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathOrigin {
    /// The process PATH was used as it stands: Windows, or the probe was
    /// disabled, or it added nothing.
    Inherited,
    /// The login shell knew about directories the process PATH did not.
    LoginShell { shell: String, added: Vec<PathBuf> },
    /// The probe was tried and did not work. The process PATH is used; the
    /// reason is developer-facing.
    ProbeFailed { reason: String },
}

impl EffectivePath {
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    /// The value to set as `PATH` on a child process.
    pub fn as_env_value(&self) -> String {
        std::env::join_paths(&self.entries)
            .map(|joined| joined.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether this differs from what the process itself inherited, and so has
    /// to be passed to children explicitly.
    pub fn differs_from_process(&self) -> bool {
        !matches!(self.origin, PathOrigin::Inherited)
    }
}

/// The effective PATH for this process, probed at most once.
///
/// Called at process start in the CLI and in the desktop app, but safe to call
/// anywhere: later calls return the same answer without spawning anything.
pub fn effective_path() -> &'static EffectivePath {
    static PATH: OnceLock<EffectivePath> = OnceLock::new();
    PATH.get_or_init(|| {
        let inherited = process_path();
        if cfg!(windows) {
            // Windows GUI processes inherit the user's PATH already, and there
            // is no login shell to ask.
            return EffectivePath {
                entries: inherited,
                origin: PathOrigin::Inherited,
            };
        }
        if std::env::var_os("EAVERY_NO_PATH_FIX").is_some() {
            return EffectivePath {
                entries: inherited,
                origin: PathOrigin::Inherited,
            };
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        match probe_login_shell(&shell) {
            Ok(probed) => {
                let (entries, added) = merge(&inherited, &split(&probed));
                if added.is_empty() {
                    EffectivePath {
                        entries,
                        origin: PathOrigin::Inherited,
                    }
                } else {
                    tracing::debug!(%shell, ?added, "login shell knew about more of the PATH");
                    EffectivePath {
                        entries,
                        origin: PathOrigin::LoginShell { shell, added },
                    }
                }
            }
            Err(reason) => {
                tracing::debug!(%shell, %reason, "could not read the login shell's PATH");
                EffectivePath {
                    entries: inherited,
                    origin: PathOrigin::ProbeFailed { reason },
                }
            }
        }
    })
}

fn process_path() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default()
}

fn split(value: &str) -> Vec<PathBuf> {
    std::env::split_paths(value).collect()
}

/// The login shell's entries first, then whatever the process had that the
/// shell did not mention. The shell's order is the user's own: a version
/// manager's shims come first there for a reason, and reversing that would
/// start the wrong `node`.
fn merge(inherited: &[PathBuf], probed: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = Vec::with_capacity(probed.len() + inherited.len());
    let mut added = Vec::new();
    for entry in probed {
        if entry.as_os_str().is_empty() || entries.contains(entry) {
            continue;
        }
        if !inherited.contains(entry) {
            added.push(entry.clone());
        }
        entries.push(entry.clone());
    }
    for entry in inherited {
        if entry.as_os_str().is_empty() || entries.contains(entry) {
            continue;
        }
        entries.push(entry.clone());
    }
    (entries, added)
}

/// Runs `$SHELL -ilc 'printf %s "$PATH"'` and returns what it printed.
///
/// The timeout is enforced by waiting on a channel rather than on the child:
/// reading a child's output to EOF is a blocking read with no deadline, so the
/// read happens on its own thread. A shell that never answers leaves that
/// thread parked on a process that will exit on its own — the whole command is
/// one `printf` — rather than holding up start-up.
fn probe_login_shell(shell: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let probed_shell = shell.to_owned();
    std::thread::Builder::new()
        .name("eavery-path-probe".to_owned())
        .spawn(move || {
            let output = std::process::Command::new(&probed_shell)
                .args(["-ilc", r#"printf %s "$PATH""#])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output();
            let _ = tx.send(output);
        })
        .map_err(|error| format!("could not start the probe: {error}"))?;

    let output = rx
        .recv_timeout(PROBE_TIMEOUT)
        .map_err(|_| format!("{shell} did not answer within {}s", PROBE_TIMEOUT.as_secs()))?
        .map_err(|error| format!("could not run {shell}: {error}"))?;

    if !output.status.success() {
        return Err(format!("{shell} exited with {}", output.status));
    }
    let probed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if probed.is_empty() {
        return Err(format!("{shell} printed an empty PATH"));
    }
    Ok(probed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(entries: &[&str]) -> Vec<PathBuf> {
        entries.iter().map(PathBuf::from).collect()
    }

    /// The Finder case: four system directories inherited, the real PATH from
    /// the shell. Homebrew must end up ahead of `/usr/bin`, or `node` resolves
    /// to the system one.
    #[test]
    fn the_login_shells_order_wins() {
        let inherited = paths(&["/usr/bin", "/bin"]);
        let probed = paths(&["/opt/homebrew/bin", "/usr/bin", "/bin"]);
        let (entries, added) = merge(&inherited, &probed);

        assert_eq!(entries, paths(&["/opt/homebrew/bin", "/usr/bin", "/bin"]));
        assert_eq!(added, paths(&["/opt/homebrew/bin"]));
    }

    /// Nothing the process already had may be lost: a directory the shell does
    /// not mention can still be the one holding the engine.
    #[test]
    fn entries_the_shell_did_not_mention_are_kept() {
        let inherited = paths(&["/usr/bin", "/opt/eavery/bin"]);
        let probed = paths(&["/home/me/.local/bin", "/usr/bin"]);
        let (entries, added) = merge(&inherited, &probed);

        assert_eq!(
            entries,
            paths(&["/home/me/.local/bin", "/usr/bin", "/opt/eavery/bin"])
        );
        assert_eq!(added, paths(&["/home/me/.local/bin"]));
    }

    #[test]
    fn duplicates_and_empty_entries_are_dropped() {
        let inherited = paths(&["/usr/bin", "", "/usr/bin"]);
        let probed = paths(&["/usr/bin", "/usr/bin", ""]);
        let (entries, added) = merge(&inherited, &probed);

        assert_eq!(entries, paths(&["/usr/bin"]));
        assert!(added.is_empty());
    }

    /// A shell that adds nothing is not worth reporting as a change: children
    /// then inherit the PATH without Eavery setting one.
    #[test]
    fn a_shell_that_adds_nothing_leaves_the_path_inherited() {
        let (_, added) = merge(&paths(&["/usr/bin"]), &paths(&["/usr/bin"]));
        assert!(added.is_empty());
    }

    #[test]
    fn the_effective_path_is_resolved_once_and_is_never_empty_when_path_is_set() {
        let effective = effective_path();
        assert_eq!(effective, effective_path(), "the probe ran twice");
        if std::env::var_os("PATH").is_some() {
            assert!(!effective.entries().is_empty());
        }
    }

    #[test]
    fn a_shell_that_is_not_there_fails_rather_than_hangs() {
        let error = probe_login_shell("/eavery/no/such/shell").unwrap_err();
        assert!(error.contains("/eavery/no/such/shell"), "{error}");
    }
}
