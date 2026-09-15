//! Printing the event stream.
//!
//! This is Developer-mode rendering: raw kinds, raw stop reasons, raw paths.
//! The vocabulary layer arrives with M5 and belongs to the GUI; a terminal
//! reader wants the protocol, not a translation of it.

use eavery_core::engine::{EngineError, OpenedSession, RawAgentEvent, StopReason};
use eavery_core::event::{Decision, PermissionView};
use eavery_core::model::EngineInfo;

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
