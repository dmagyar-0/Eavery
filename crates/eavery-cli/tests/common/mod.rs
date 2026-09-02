//! Finding the fake agent binary from another crate's test.
//!
//! Lives under `tests/common/` rather than `tests/` so Cargo treats it as a
//! module rather than as a test target of its own.
//!
//! Lives under `tests/common/` rather than `tests/` so Cargo treats it as a
//! module rather than as a test target of its own.
//!
//! `CARGO_BIN_EXE_*` only exists inside the crate that declares the binary, and
//! artifact dependencies are still nightly-only. So: look next to the test
//! executable, which is where Cargo puts workspace binaries, and build it if it
//! is not there yet — which is what happens when someone runs
//! `cargo test -p eavery-acp` on its own rather than `--workspace`.

use std::path::PathBuf;
use std::sync::OnceLock;

pub fn fake_agent() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let name = format!("eavery-fake-agent{}", std::env::consts::EXE_SUFFIX);
        let profile_dir = std::env::current_exe()
            .expect("the test has a path")
            // .../target/<profile>/deps/<test binary>
            .parent()
            .and_then(|deps| deps.parent())
            .map(PathBuf::from)
            .expect("the test binary lives under target/<profile>/deps");

        let candidate = profile_dir.join(&name);
        if candidate.exists() {
            return candidate;
        }

        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "eavery-fake-agent"])
            .status()
            .expect("run cargo build for the fake agent");
        assert!(status.success(), "could not build eavery-fake-agent");
        assert!(
            candidate.exists(),
            "eavery-fake-agent is still missing at {}",
            candidate.display()
        );
        candidate
    })
    .clone()
}
