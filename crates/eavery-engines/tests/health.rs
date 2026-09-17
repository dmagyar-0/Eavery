//! The health check state machine (`docs/plan/04-acp-engines.md` §9), driven
//! against the fake agent.
//!
//! The behaviour worth protecting is the shallow/deep split: the default check
//! must establish that the engine is installed, responding and signed in
//! *without* spending the user's subscription. Every test below turns on
//! whether a prompt was sent, which the script makes visible by writing a file
//! when its turn runs.

mod common;

use std::path::{Path, PathBuf};

use eavery_core::model::EngineStatus;
use eavery_engines::discovery::{Environment, Platform, Resolver};
use eavery_engines::health::{HealthCache, HealthOptions, run_health_check};
use eavery_engines::spec::{self, EngineSpec};
use serde_json::json;

fn fake() -> &'static EngineSpec {
    spec::find("fake").expect("the fake engine is in the table")
}

/// A resolver that finds only the fake agent, and nothing the machine running
/// the test happens to have installed.
fn resolver_for_fake_agent() -> Resolver {
    let mut env = Environment::empty(Platform::current());
    env.beside_exe = common::fake_agent().parent().map(PathBuf::from);
    Resolver::new(env)
}

fn write_script(dir: &Path, script: serde_json::Value) -> PathBuf {
    let path = dir.join("script.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&script).unwrap()).unwrap();
    path
}

/// A script whose only turn writes `marker`, so a test can see whether the
/// deep check's prompt reached the agent.
fn script_that_marks(marker: &Path) -> serde_json::Value {
    json!({
        "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"} },
        "session": { "modes": { "currentModeId": "work", "availableModes": [
            {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"}
        ]}},
        "turns": [{
            "match": "OK",
            "actions": [
                {"write_direct": {"path": marker.to_string_lossy(), "text": "OK"}},
                {"text": "OK"},
                {"stop": "end_turn"}
            ]
        }]
    })
}

fn options(script: &Path) -> HealthOptions {
    HealthOptions {
        extra_args: vec!["--script".to_owned(), script.to_string_lossy().into_owned()],
        ..HealthOptions::default()
    }
}

#[tokio::test]
async fn a_shallow_check_proves_the_engine_works_without_prompting_it() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("prompted");
    let script = write_script(dir.path(), script_that_marks(&marker));

    let status = run_health_check(fake(), &resolver_for_fake_agent(), &options(&script)).await;

    match status {
        EngineStatus::Ready {
            info,
            modes,
            current_mode,
        } => {
            assert_eq!(info.engine_id, "fake");
            assert_eq!(info.name.as_deref(), Some("fake"));
            assert_eq!(
                modes
                    .iter()
                    .map(|mode| mode.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["work", "plan"]
            );
            assert_eq!(current_mode.as_deref(), Some("work"));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
    assert!(
        !marker.exists(),
        "the default check sent a prompt; §9 says it stops at session/new"
    );
}

#[tokio::test]
async fn a_deep_check_sends_one_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("prompted");
    let script = write_script(dir.path(), script_that_marks(&marker));

    let status = run_health_check(
        fake(),
        &resolver_for_fake_agent(),
        &HealthOptions {
            deep: true,
            ..options(&script)
        },
    )
    .await;

    assert!(matches!(status, EngineStatus::Ready { .. }), "{status:?}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "OK",
        "the deep check did not reach the agent"
    );
}

#[tokio::test]
async fn an_engine_that_is_not_installed_says_where_it_looked() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = Environment::empty(Platform::current());
    env.path = vec![dir.path().to_path_buf()];
    let resolver = Resolver::new(env);

    let status = run_health_check(fake(), &resolver, &HealthOptions::default()).await;

    match status {
        EngineStatus::NotInstalled {
            instructions,
            searched,
        } => {
            assert!(!instructions.is_empty());
            assert!(
                searched.contains(&dir.path().display().to_string()),
                "a user told 'not installed' has to be told where Eavery looked: {searched:?}"
            );
        }
        other => panic!("expected NotInstalled, got {other:?}"),
    }
}

#[tokio::test]
async fn an_adapter_that_needs_node_is_its_own_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = Environment::empty(Platform::current());
    env.path = vec![dir.path().to_path_buf()];

    let claude = spec::find("claude").unwrap();
    let status = run_health_check(claude, &Resolver::new(env), &HealthOptions::default()).await;

    assert!(
        matches!(status, EngineStatus::NeedsNode),
        "the claude adapter without Node is a Node problem, not a missing engine: {status:?}"
    );
}

/// An engine that starts and then dies is not "not installed": the difference
/// is what the user is told to do next.
#[tokio::test]
async fn an_engine_that_will_not_start_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-script.json");

    let status = run_health_check(fake(), &resolver_for_fake_agent(), &options(&missing)).await;

    assert!(
        matches!(status, EngineStatus::Unavailable { .. }),
        "{status:?}"
    );
}

#[tokio::test]
async fn an_engine_speaking_another_protocol_version_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({ "initialize": { "protocolVersion": 99 }, "turns": [] }),
    );

    let status = run_health_check(fake(), &resolver_for_fake_agent(), &options(&script)).await;

    match status {
        EngineStatus::Unavailable { reason } => {
            assert!(reason.contains("99"), "{reason}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// §9: cache for ten minutes. Settings opens with several engines listed, and
/// re-spawning all of them on every render is how a settings screen takes five
/// seconds to appear.
#[tokio::test]
async fn a_cached_answer_is_reused_instead_of_respawning_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("prompted");
    let script = write_script(dir.path(), script_that_marks(&marker));

    let cache = HealthCache::new();
    let resolver = resolver_for_fake_agent();
    let first = cache.get_or_run(fake(), &resolver, &options(&script)).await;
    assert!(matches!(first, EngineStatus::Ready { .. }), "{first:?}");

    // Anything that re-runs the check now would fail: the script is gone.
    std::fs::remove_file(&script).unwrap();
    let second = cache.get_or_run(fake(), &resolver, &options(&script)).await;
    assert!(matches!(second, EngineStatus::Ready { .. }), "{second:?}");

    cache.invalidate("fake").await;
    let third = cache.get_or_run(fake(), &resolver, &options(&script)).await;
    assert!(
        matches!(third, EngineStatus::Unavailable { .. }),
        "'Check again' has to actually check again: {third:?}"
    );
}

/// A shallow answer has not proved what a deep one proves, so it must not be
/// handed back to a deep request.
#[tokio::test]
async fn a_cached_shallow_answer_does_not_satisfy_a_deep_check() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("prompted");
    let script = write_script(dir.path(), script_that_marks(&marker));

    let cache = HealthCache::new();
    let resolver = resolver_for_fake_agent();
    cache.get_or_run(fake(), &resolver, &options(&script)).await;
    assert!(!marker.exists());

    cache
        .get_or_run(
            fake(),
            &resolver,
            &HealthOptions {
                deep: true,
                ..options(&script)
            },
        )
        .await;
    assert!(marker.exists(), "the deep check was skipped");
}
