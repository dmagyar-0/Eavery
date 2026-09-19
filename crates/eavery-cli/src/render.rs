//! Printing the event stream.
//!
//! This is Developer-mode rendering: raw kinds, raw stop reasons, raw paths.
//! The vocabulary layer arrives with M5 and belongs to the GUI; a terminal
//! reader wants the protocol, not a translation of it.

use std::path::PathBuf;

use eavery_core::engine::{EngineError, OpenedSession, RawAgentEvent, StopReason};
use eavery_core::event::{CoreEvent, Decision, Digest, PermissionView};
use eavery_core::journal::{ChangeSet, Unprotected, UnprotectedReason};
use eavery_core::model::{Checkpoint, CheckpointKind, EngineInfo, EngineStatus, Plan, Project};
use eavery_core::turn::Approval;
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

// ---- the Project commands (M2-T08) -----------------------------------------

/// One line per event, or nothing for the ones a terminal reader does not
/// need: a checkpoint is shown by the commands that are about checkpoints.
pub fn core_event(event: &CoreEvent) -> Option<String> {
    Some(match event {
        CoreEvent::TurnStarted { phase, .. } => format!("turn     started ({phase:?})"),
        CoreEvent::PhaseChanged { phase, .. } => format!("phase    {phase:?}"),
        CoreEvent::AgentText { text, .. } => format!("text     {}", indent(text)),
        CoreEvent::AgentThought { text, .. } => format!("thought  {}", indent(text)),
        CoreEvent::ToolCallStarted { call, .. } | CoreEvent::ToolCallUpdated { call, .. } => {
            let where_ = if call.locations.is_empty() {
                String::new()
            } else {
                format!("  {}", call.locations.join(", "))
            };
            format!(
                "tool     [{}] {} ({}, {:?}){where_}",
                call.status, call.title, call.kind, call.risk
            )
        }
        CoreEvent::PlanUpdated { entries, .. } => {
            let mut lines = vec![format!("plan     {} step(s)", entries.len())];
            for entry in entries {
                lines.push(format!(
                    "           - [{}] {}",
                    entry.status.as_deref().unwrap_or("pending"),
                    entry.content
                ));
            }
            lines.join("\n")
        }
        CoreEvent::PermissionRequested { request, .. } => format!(
            "ask      {} ({:?})\n           {}",
            request.title, request.risk, request.explanation
        ),
        CoreEvent::PermissionResolved { decision, by, .. } => {
            format!("answer   {decision:?} (by {by:?})")
        }
        CoreEvent::PlanReady { plan, vendor, .. } => plan_lines(plan, vendor).join("\n"),
        CoreEvent::CheckpointCreated { checkpoint } => {
            format!("protect  {} {}", short(&checkpoint.id), checkpoint.label)
        }
        CoreEvent::Restored {
            to, new_checkpoint, ..
        } => format!("back to  {} (now at {})", short(to), short(new_checkpoint)),
        CoreEvent::TurnFinished { stop_reason, .. } => format!("done     {stop_reason}"),
        CoreEvent::EngineStatus { engine_id, status } => {
            format!("engine   {engine_id} {}", state_word(status))
        }
        CoreEvent::EngineCrashed { stderr_tail, .. } => {
            let mut lines = vec!["error    the engine stopped".to_owned()];
            for line in stderr_tail {
                lines.push(format!("stderr   {line}"));
            }
            lines.join("\n")
        }
        CoreEvent::Error {
            message,
            next_action,
            ..
        } => match next_action {
            Some(next) => format!("error    {message}\nnext     {next}"),
            None => format!("error    {message}"),
        },
    })
}

/// The plan, as the person is asked to approve it: the summary, the steps,
/// and — always — what would leave the computer and what could not be
/// undone, "nothing" when nothing.
fn plan_lines(plan: &Plan, vendor: &str) -> Vec<String> {
    let mut lines = vec![format!("plan     {}", plan.summary)];
    for (index, step) in plan.steps.iter().enumerate() {
        lines.push(format!("           {}. {}", index + 1, step.text));
    }
    let list = |lines: &mut Vec<String>, label: &str, items: &[String], none: &str| {
        lines.push(match items {
            [] => format!("{label:8} {none}"),
            items => format!("{label:8} {}", items.join("; ")),
        });
    };
    list(&mut lines, "files", &plan.files_touched, "none named");
    list(
        &mut lines,
        "sends",
        &plan.outbound,
        "nothing leaves this computer",
    );
    list(
        &mut lines,
        "forever",
        &plan.irreversible,
        "nothing that cannot be undone",
    );
    if !plan.will_not_do.is_empty() {
        list(&mut lines, "not", &plan.will_not_do, "");
    }
    if !vendor.is_empty() {
        lines.push(format!("vendor   your documents are sent to {vendor}"));
    }
    if plan.steps.is_empty() && !plan.raw_markdown.trim().is_empty() {
        // No structured block came back; the engine's own words are the plan.
        lines.push(format!("raw      {}", indent(plan.raw_markdown.trim())));
    }
    lines
}

/// Nobody at the terminal to read the plan, so nothing runs.
pub fn plan_unattended() -> String {
    "approve  no terminal to ask on; the plan is not carried out. \
     Use --approve yes to approve up front."
        .to_owned()
}

pub fn plan_answered(answer: &Approval) -> String {
    match answer {
        Approval::Approved { edits: None } => "approve  yes".to_owned(),
        Approval::Approved { edits: Some(edits) } => format!("approve  yes, with changes: {edits}"),
        Approval::Rejected => "approve  no".to_owned(),
    }
}

/// What the turn did. Always printed, including the empty lists: "nothing left
/// this computer" is the line worth reading.
pub fn digest(digest: &Digest) -> Vec<String> {
    let mut lines = Vec::new();
    for (label, files) in [
        ("added", &digest.files_added),
        ("changed", &digest.files_changed),
        ("removed", &digest.files_removed),
    ] {
        for file in files {
            lines.push(format!("{label:8} {file}"));
        }
    }
    if digest.files_added.is_empty()
        && digest.files_changed.is_empty()
        && digest.files_removed.is_empty()
    {
        lines.push("files    nothing changed".to_owned());
    }
    lines.push(match digest.outbound_actions.as_slice() {
        [] => "sent     nothing left this computer".to_owned(),
        actions => format!("sent     {}", actions.join("; ")),
    });
    if !digest.refused_actions.is_empty() {
        lines.push(format!("refused  {}", digest.refused_actions.join("; ")));
    }
    if let Some(undo_to) = &digest.undo_to {
        lines.push(format!("undo     eavery-cli undo --to {}", short(undo_to)));
    }
    lines
}

pub fn projects_table(projects: &[Project]) -> Vec<String> {
    let mut lines = Vec::new();
    for project in projects {
        lines.push(format!(
            "{}  {}  {}",
            project.id,
            project.name,
            project.root.display()
        ));
        if let Some(engine) = &project.engine_id {
            lines.push(format!("{:38}engine: {engine}", ""));
        }
    }
    lines
}

pub fn checkpoints_table(checkpoints: &[Checkpoint]) -> Vec<String> {
    checkpoints
        .iter()
        .map(|checkpoint| {
            format!(
                "{}  {}  {:<9}  {} file(s)  {}",
                short(&checkpoint.id),
                checkpoint.created_at.format("%Y-%m-%d %H:%M"),
                kind_word(checkpoint.kind),
                checkpoint.files_changed,
                checkpoint.label
            )
        })
        .collect()
}

pub fn change_set(changes: &ChangeSet) -> Vec<String> {
    let mut lines = Vec::new();
    for (label, files) in [
        ("added", &changes.added),
        ("changed", &changes.changed),
        ("removed", &changes.removed),
    ] {
        for file in files {
            lines.push(format!("{label:8} {}", file.display()));
        }
    }
    if lines.is_empty() {
        lines.push("files    nothing changed".to_owned());
    }
    for (path, patch) in &changes.text_diffs {
        lines.push(format!("--- {}", path.display()));
        lines.push(patch.trim_end().to_owned());
    }
    lines
}

/// Everything Undo does not cover, and why. Printed on every `project open`,
/// because "your files are protected" is a claim that has to be qualified the
/// moment it is not completely true.
pub fn unprotected(files: &[Unprotected]) -> Vec<String> {
    if files.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "note     {} file(s) are not protected by Undo:",
        files.len()
    )];
    for file in files {
        let why = match file.reason {
            UnprotectedReason::TooLarge { bytes } => format!("over 50 MB ({})", self::bytes(bytes)),
            UnprotectedReason::NotDownloaded => "not downloaded from the cloud yet".to_owned(),
        };
        lines.push(format!("           {}  — {why}", file.path.display()));
    }
    lines
}

/// The first eight characters of a commit, which is what a person types.
pub fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

pub fn bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn kind_word(kind: CheckpointKind) -> &'static str {
    match kind {
        CheckpointKind::PreTurn => "before",
        CheckpointKind::PostTurn => "after",
        CheckpointKind::Manual => "manual",
        CheckpointKind::Restore => "restore",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_line_text_stays_in_its_column() {
        assert_eq!(indent("one\ntwo"), "one\n         two");
        assert_eq!(indent("one"), "one");
    }

    /// The short form is what a person types back at `undo --to`, so it has
    /// to work on a real commit id and not panic on anything shorter.
    #[test]
    fn a_checkpoint_is_shortened_to_something_typeable() {
        assert_eq!(short("0123456789abcdef"), "01234567");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    /// "Nothing left this computer" is the line the digest exists for, so it
    /// is printed even when — especially when — there is nothing to say.
    #[test]
    fn a_digest_that_did_nothing_still_says_what_did_not_happen() {
        let lines = digest(&Digest::default());
        assert!(lines.iter().any(|line| line.contains("nothing changed")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("nothing left this computer"))
        );
    }

    #[test]
    fn a_digest_names_the_files_and_the_way_back() {
        let lines = digest(&Digest {
            files_changed: vec!["report.docx".into()],
            outbound_actions: vec!["Email the report to finance".into()],
            refused_actions: vec!["Run the backup script".into()],
            undo_to: Some("0123456789abcdef".into()),
            ..Digest::default()
        });
        assert!(lines.iter().any(|line| line == "changed  report.docx"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Email the report to finance"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("refused") && line.contains("backup"))
        );
        assert!(lines.iter().any(|line| line.contains("undo --to 01234567")));
    }
}
