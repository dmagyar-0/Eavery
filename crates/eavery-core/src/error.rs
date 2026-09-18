//! The one error shape that crosses a boundary.
//!
//! Every command the desktop app exposes returns `Result<T, AppError>`, and
//! `AppError` is three fields: a code for the caller, a message, and the next
//! action for the person (`docs/plan/03-architecture.md` §7). The last of
//! those is the point. Everyday mode renders a failure as something to do
//! about it rather than as a failure (`docs/plan/07-ui-vocabulary.md` §4), and
//! the only layer that knows which action fits is the one the error came from.
//!
//! So every error type in this crate can say its own next action, and this
//! carries the answer outwards without inventing one.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::EngineError;
use crate::event::ErrorCode;
use crate::journal::JournalError;
use crate::store::StoreError;
use crate::turn::TurnError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    /// What the person can do about it, in their own words. `None` when there
    /// is genuinely nothing to suggest, which the UI renders as a plain
    /// report rather than as an instruction.
    pub next_action: Option<String>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            next_action: None,
        }
    }

    #[must_use]
    pub fn with_next_action(mut self, next_action: impl Into<String>) -> Self {
        self.next_action = Some(next_action.into());
        self
    }

    /// Something that is Eavery's own fault. The message is for Diagnostics;
    /// the next action is the only one that ever helps.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
            .with_next_action("Try again. If it keeps happening, check Diagnostics.")
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

impl From<TurnError> for AppError {
    fn from(error: TurnError) -> Self {
        AppError {
            code: error.code(),
            next_action: error.next_action(),
            message: error.to_string(),
        }
    }
}

impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        AppError {
            code: ErrorCode::Internal,
            next_action: error.next_action(),
            message: error.to_string(),
        }
    }
}

impl From<JournalError> for AppError {
    fn from(error: JournalError) -> Self {
        AppError {
            code: ErrorCode::CheckpointFailed,
            next_action: error.next_action(),
            message: error.to_string(),
        }
    }
}

impl From<EngineError> for AppError {
    fn from(error: EngineError) -> Self {
        AppError {
            code: error.code(),
            next_action: error.next_action(),
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three fields the frontend switches on, in the spelling §7 gives.
    #[test]
    fn an_app_error_serialises_as_the_three_fields() {
        let json = serde_json::to_value(AppError::internal("the database is locked")).unwrap();
        assert_eq!(json["code"], "internal");
        assert_eq!(json["message"], "the database is locked");
        assert!(json["next_action"].is_string());
    }

    /// An error that reaches the user without a next action has to be one that
    /// genuinely has none. The ones with an answer must carry it across.
    #[test]
    fn the_next_action_survives_the_conversion() {
        let locked = TurnError::Busy {
            doing: "a turn is already running",
            turn_id: None,
        };
        let error = AppError::from(locked);
        assert_eq!(error.code, ErrorCode::TurnAlreadyRunning);
        assert!(
            error.next_action.unwrap().contains("Stop"),
            "the user is told how to get out of it"
        );
    }
}
