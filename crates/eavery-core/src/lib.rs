//! Eavery's domain core: the model, the event stream, and the engine contract.
//!
//! This crate never talks to an engine process. It defines the [`engine::Engine`]
//! trait; `eavery-acp` implements it. That separation is what makes the fake
//! agent and the CLI cheap (see `docs/plan/03-architecture.md` §1).
#![deny(unsafe_code)]

pub mod diagnostics;
pub mod engine;
pub mod error;
pub mod event;
pub mod journal;
pub mod model;
pub mod paths;
pub mod plan;
pub mod policy;
pub mod prompts;
pub mod store;
pub mod turn;
