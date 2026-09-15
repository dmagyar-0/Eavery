//! ACP client: spawns an engine as a child process and speaks newline-delimited
//! JSON-RPC 2.0 to it over stdio (`docs/plan/04-acp-engines.md`).
//!
//! This is the §7 hand-rolled client rather than the 2.x SDK. The reasons are
//! recorded in `docs/plan/CHANGELOG-plan.md` under M0-T06; the short version is
//! that the SDK pins a schema version the workspace cannot also use, runs on a
//! different async runtime, and gives no way to set a child's working directory
//! or to read its stderr while it is alive.
//!
//! The layering the rest of the plan depends on holds either way: this crate is
//! dumb. It maps ACP onto [`eavery_core::engine::RawAgentEvent`] and answers the
//! agent's requests by asking the layer above. It holds no policy and no
//! vocabulary.
#![deny(unsafe_code)]

mod conn;
mod engine;
mod wire;

pub use conn::{ClientHandler, Connection, HandlerError, LaunchSpec, NotificationSink};
pub use engine::{AcpEngine, FsGuard};
pub use wire::{PROTOCOL_VERSION, map_session_update};
