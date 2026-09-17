//! Printing the event stream.
//!
//! This is Developer-mode rendering: raw kinds, raw stop reasons, raw paths.
//! The vocabulary layer arrives with M5 and belongs to the GUI; a terminal
//! reader wants the protocol, not a translation of it.

use std::path::PathBuf;

use eavery_core::engine::{EngineError, OpenedSession, RawAgentEvent, StopReason};
use eavery_core::event::{Decision, PermissionView};
use eavery_core::model::{EngineInfo, EngineStatus};
use eavery_engines::discovery::LaunchVia;
use eavery_engines::spec::EngineSpec;

use crate::println_flush;

pub fn engine_started(info: &EngineInfo) {
    let name = match (&info.name, &info.version) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name.clone(),
        _ => info.engine_id.clone(),
    };
    println_flush(format!(
        "engine   {name} (protocol v{}, loadSession={})",
        info.protocol_version, info.load_session
    ));
    if !info.auth_methods.is_empty() {
        println_flush(format!("auth     {}", info.auth_methods.join(", ")));
    }
}

pub fn session_opened(session: &OpenedSession) {
    println_flush(format!("session  {}", session.session_id));
    if !session.modes.is_empty() {
        let modes: Vec<String> = session
            .modes
            .iter()
            .map(|mode| {
                if Some(&mode.id) == session.current_mode.as_ref() {
                    format!("[{}]", mode.id)
                } else {
                    mode.id.clone()
                }
            })
            .collect();
        println_flush(format!("modes    {}", modes.join(" ")));
    }
}

pub fn event(event: &RawAgentEvent) {
    match event {
        RawAgentEvent::Text(text) => println_flush(format!("text     {}", indent(text))),
        RawAgentEvent::Thought(text) => println_flush(format!("thought  {}", indent(text))),
        RawAgentEvent::ToolCall(call) => {
            let where_ = if call.locations.is_empty() {
                String::new()
            } else {
                format!("  {}", call.locations.join(", "))
            };
            println_flush(format!(
                "tool     [{}] {} ({}){where_}",
                call.status, call.title, call.kind
            ));
        }
        RawAgentEvent::ToolCallUpdate(update) => {
            let status = update.status.as_deref().unwrap_or("updated");
            let title = update.title.as_deref().unwrap_or(update.id.as_str());
            println_flush(format!("tool     [{status}] {title}"));
        }
        RawAgentEvent::PlanEntries(entries) => {
            println_flush(format!("plan     {} step(s)", entries.len()));
            for entry in entries {
                let status = entry.status.as_deref().unwrap_or("pending");
                println_flush(format!("           - [{status}] {}", entry.content));
            }
        }
        RawAgentEvent::ModeChanged(mode) => println_flush(format!("mode     {mode}")),
        // Logged rather than printed: `Other` is what ACP grew since this
        // version was written, and it is noise in a transcript.
        RawAgentEvent::Other(value) => tracing::debug!(?value, "unmodelled session update"),
    }
}

// The permission lines are returned rather than printed: they go through the
// same queue as the events, so the transcript stays in the engine's order.

pub fn permission_prompt(view: &PermissionView) -> String {
    format!(
        "ask      {} ({:?})\n           {}\n           [a]llow / [r]eject:",
        view.title, view.risk, view.explanation
    )
}

pub fn permission_unattended(view: &PermissionView) -> String {
    format!(
        "ask      {} ({:?}) — rejected: no terminal to ask on. \
         Use --answer to decide up front.",
        view.title, view.risk
    )
}

pub fn permission_answered(view: &PermissionView, decision: Decision) -> String {
    format!("answer   {:?} for {}", decision, view.title)
}

pub fn finished(stop: StopReason) {
    println_flush(format!("done     {}", stop.as_str()));
}

pub fn engine_error(error: &EngineError) {
    println_flush(format!("error    {error}"));
    if let EngineError::Crashed { stderr_tail, .. } = error {
        for line in stderr_tail.iter().rev().take(50).rev() {
            println_flush(format!("stderr   {line}"));
        }
    }
}

/// One row per engine, plus an indented line saying where it was found and
/// what it said about itself. The health check reports states, not errors, so
/// a table is the right shape: nothing here is a failure of the command.
pub type EngineRow = (
    &'static EngineSpec,
    Option<(PathBuf, LaunchVia)>,
    EngineStatus,
);

pub fn engines_table(rows: &[EngineRow]) -> Vec<String> {
    let width = rows
        .iter()
        .map(|(spec, _, _)| spec.id.len())
        .max()
        .unwrap_or(0)
        .max(6);

    let mut lines = Vec::new();
    for (spec, found, status) in rows {
        lines.push(format!(
            "{:width$}  {:<13}  {}",
            spec.id,
            state_word(status),
            eavery_engines::health::describe(spec, status),
            width = width
        ));
        if let Some((program, via)) = found {
            lines.push(format!(
                "{:width$}  {}  ({})",
                "",
                program.display(),
                via_word(*via),
                width = width
            ));
        }
        for detail in details(status) {
            lines.push(format!("{:width$}  {detail}", "", width = width));
        }
    }
    lines
}

pub fn engines_json(rows: &[EngineRow]) -> anyhow::Result<String> {
    let values: Vec<serde_json::Value> = rows
        .iter()
        .map(|(spec, found, status)| {
            serde_json::json!({
                "id": spec.id,
                "display_name": spec.display_name,
                "vendor": spec.vendor,
                "program": found.as_ref().map(|(program, _)| program.display().to_string()),
                "via": found.as_ref().map(|(_, via)| via_word(*via)),
                "status": status,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&values)?)
}

/// The one-word state, for scanning a column.
fn state_word(status: &EngineStatus) -> &'static str {
    match status {
        EngineStatus::NotInstalled { .. } => "not installed",
        EngineStatus::NeedsNode => "needs node",
        EngineStatus::NeedsSignIn { .. } => "needs sign-in",
        EngineStatus::Installing { .. } => "installing",
        EngineStatus::SigningIn => "signing in",
        EngineStatus::Ready { .. } => "ready",
        EngineStatus::Unavailable { .. } => "unavailable",
    }
}

fn via_word(via: LaunchVia) -> &'static str {
    match via {
        LaunchVia::ExplicitPath => "from settings",
        LaunchVia::Path => "on PATH",
        LaunchVia::WellKnown => "well-known location",
        LaunchVia::Npx => "through npx",
    }
}

/// The developer-facing half: what the engine answered, or why it did not.
/// This is exactly what M1-T04 to M1-T07 ask to be recorded, so the command
/// that runs those verifications prints it.
fn details(status: &EngineStatus) -> Vec<String> {
    match status {
        EngineStatus::Ready {
            info,
            modes,
            current_mode,
        } => {
            let mut lines = vec![format!(
                "{} {}, protocol v{}, loadSession={}",
                info.name.as_deref().unwrap_or("(no agent name)"),
                info.version.as_deref().unwrap_or(""),
                info.protocol_version,
                info.load_session
            )];
            if !modes.is_empty() {
                let modes: Vec<String> = modes
                    .iter()
                    .map(|mode| {
                        if Some(&mode.id) == current_mode.as_ref() {
                            format!("[{}]", mode.id)
                        } else {
                            mode.id.clone()
                        }
                    })
                    .collect();
                lines.push(format!("modes: {}", modes.join(" ")));
            } else {
                lines.push("modes: none advertised".to_owned());
            }
            if !info.auth_methods.is_empty() {
                lines.push(format!("auth: {}", info.auth_methods.join(", ")));
            }
            lines
        }
        EngineStatus::Unavailable { reason } => vec![reason.clone()],
        EngineStatus::NotInstalled { searched, .. } if !searched.is_empty() => {
            vec![format!("looked in: {}", searched.join(", "))]
        }
        _ => Vec::new(),
    }
}

/// Keeps a multi-line chunk under the same left margin as everything else.
fn indent(text: &str) -> String {
    text.replace('\n', "\n         ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_line_text_stays_in_its_column() {
        assert_eq!(indent("one\ntwo"), "one\n         two");
        assert_eq!(indent("one"), "one");
    }
}
