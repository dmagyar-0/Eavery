//! ACP client: spawns an engine as a child process and speaks newline-delimited
//! JSON-RPC 2.0 to it over stdio (`docs/plan/04-acp-engines.md`).
#![deny(unsafe_code)]
