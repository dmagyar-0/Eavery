//! The engine table, executable discovery, and health checks
//! (`docs/plan/04-acp-engines.md` §2, §3, §9).
//!
//! Everything here is about *which* engine and *where*. Talking to one is
//! `eavery-acp`'s job; deciding what it may do is `eavery-core`'s.
#![deny(unsafe_code)]

pub mod discovery;
pub mod instructions;
pub mod path_env;
pub mod spec;

pub use discovery::{Environment, LaunchVia, NotInstalled, Platform, Resolved, Resolver};
pub use spec::{AuthKind, ENGINES, EngineSource, EngineSpec, Launch, find, pick_mode, visible};
