//! Eavery's domain core: the model, the event stream, and the engine contract.
//!
//! This crate never talks to an engine process. It defines the [`engine::Engine`]
//! trait; `eavery-acp` implements it. That separation is what makes the fake
//! agent and the CLI cheap (see `docs/plan/03-architecture.md` §1).
#![deny(unsafe_code)]

pub mod event;
pub mod model;
