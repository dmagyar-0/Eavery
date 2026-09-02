//! Minimal serde types for the ACP messages Eavery sends and receives
//! (`docs/plan/04-acp-engines.md` §1), plus the mapping into
//! [`RawAgentEvent`] (§6).
//!
//! These are deliberately narrow and forgiving: every field defaults, no struct
//! denies unknown fields, and an unrecognised `sessionUpdate` becomes
//! [`RawAgentEvent::Other`] rather than an error. ACP grows; a client that
//! crashes on a field it has never seen is a client that breaks on the next
//! engine release.

use eavery_core::engine::{RawAgentEvent, RawPlanEntry, RawToolCall, RawToolCallUpdate};
use serde::Deserialize;
use serde_json::{Value, json};

/// The protocol version Eavery speaks.
pub const PROTOCOL_VERSION: u16 = 1;

/// `initialize` params. Eavery serves both filesystem methods and no terminal:
/// engines then use their own shell tooling, which the policy handler sees as
/// ordinary tool calls.
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": true, "writeTextFile": true },
            "terminal": false,
        },
        "clientInfo": { "name": "Eavery", "version": env!("CARGO_PKG_VERSION") },
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: u16,
    pub agent_capabilities: AgentCapabilities,
    pub auth_methods: Vec<AuthMethod>,
    pub agent_info: Option<Implementation>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub load_session: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NewSessionResponse {
    pub session_id: String,
    pub modes: Option<SessionModeState>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionModeState {
    pub current_mode_id: String,
    pub available_modes: Vec<SessionModeWire>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionModeWire {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptResponse {
    pub stop_reason: String,
}

/// A `session/request_permission` request from the agent.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestPermissionParams {
    pub session_id: String,
    pub tool_call: ToolCallWire,
    pub options: Vec<PermissionOptionWire>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PermissionOptionWire {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolCallWire {
    pub tool_call_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub locations: Option<Vec<LocationWire>>,
    pub content: Option<Vec<Value>>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LocationWire {
    pub path: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReadTextFileParams {
    pub session_id: String,
    pub path: String,
    pub line: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WriteTextFileParams {
    pub session_id: String,
    pub path: String,
    pub content: String,
}

/// The `params` of a `session/update` notification.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: Value,
}

/// Maps one `session/update` payload onto a [`RawAgentEvent`]
/// (`docs/plan/04-acp-engines.md` §6).
///
/// `user_message_chunk` is dropped: it echoes the prompt Eavery just sent.
/// Everything unmodelled — including `sessionUpdate` values this version has
/// never heard of — becomes `Other`, which is logged and never shown.
pub fn map_session_update(update: &Value) -> Option<RawAgentEvent> {
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or("");
    match kind {
        "agent_message_chunk" => Some(RawAgentEvent::Text(text_of(update)?)),
        "agent_thought_chunk" => Some(RawAgentEvent::Thought(text_of(update)?)),
        // Echoes our own prompt back at us. Only interesting when replaying a
        // loaded session, which is M7's problem.
        "user_message_chunk" => None,
        "tool_call" => Some(RawAgentEvent::ToolCall(tool_call(update))),
        "tool_call_update" => Some(RawAgentEvent::ToolCallUpdate(tool_call_update(update))),
        "plan" => Some(RawAgentEvent::PlanEntries(plan_entries(update))),
        "current_mode_update" => Some(RawAgentEvent::ModeChanged(
            update
                .get("currentModeId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )),
        _ => Some(RawAgentEvent::Other(update.clone())),
    }
}

/// The text of a content chunk. A chunk carrying an image or a resource has no
/// text, and is reported as `Other` rather than as an empty message.
fn text_of(update: &Value) -> Option<String> {
    let content = update.get("content")?;
    content
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn locations_of(value: &Value) -> Vec<String> {
    value
        .get("locations")
        .and_then(Value::as_array)
        .map(|locations| {
            locations
                .iter()
                .filter_map(|l| l.get("path").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Paths from `content[].type == "diff"` entries: the files this call proposes
/// to change, which the digest and the risk classifier both want.
fn diff_paths_of(value: &Value) -> Vec<String> {
    value
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("diff"))
                .filter_map(|c| c.get("path").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn tool_call(update: &Value) -> RawToolCall {
    RawToolCall {
        id: string_at(update, "toolCallId").unwrap_or_default(),
        title: string_at(update, "title").unwrap_or_default(),
        kind: string_at(update, "kind").unwrap_or_else(|| "other".to_owned()),
        status: string_at(update, "status").unwrap_or_else(|| "pending".to_owned()),
        locations: locations_of(update),
        diff_paths: diff_paths_of(update),
        raw_input: update.get("rawInput").cloned(),
    }
}

fn tool_call_update(update: &Value) -> RawToolCallUpdate {
    RawToolCallUpdate {
        id: string_at(update, "toolCallId").unwrap_or_default(),
        title: string_at(update, "title"),
        kind: string_at(update, "kind"),
        status: string_at(update, "status"),
        // Absent means unchanged, so these stay `None` rather than becoming
        // empty vectors that would wipe what the tool call already reported.
        locations: update.get("locations").map(|_| locations_of(update)),
        diff_paths: update.get("content").map(|_| diff_paths_of(update)),
        raw_output: update.get("rawOutput").cloned(),
    }
}

fn plan_entries(update: &Value) -> Vec<RawPlanEntry> {
    update
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| RawPlanEntry {
                    content: string_at(entry, "content").unwrap_or_default(),
                    priority: string_at(entry, "priority"),
                    status: string_at(entry, "status"),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_thought_chunks_become_text_and_thought() {
        let text = map_session_update(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hello"}
        }));
        assert_eq!(text, Some(RawAgentEvent::Text("hello".into())));

        let thought = map_session_update(&json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "thinking"}
        }));
        assert_eq!(thought, Some(RawAgentEvent::Thought("thinking".into())));
    }

    #[test]
    fn our_own_prompt_echo_is_dropped() {
        let echo = map_session_update(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "what we sent"}
        }));
        assert_eq!(echo, None);
    }

    #[test]
    fn a_tool_call_carries_locations_and_diff_paths() {
        let event = map_session_update(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "c1",
            "title": "Edit report.md",
            "kind": "edit",
            "status": "in_progress",
            "locations": [{"path": "/abs/report.md", "line": 12}],
            "content": [
                {"type": "content", "content": {"type": "text", "text": "..."}},
                {"type": "diff", "path": "/abs/report.md", "oldText": "a", "newText": "b"}
            ],
            "rawInput": {"file": "report.md"}
        }))
        .unwrap();

        let RawAgentEvent::ToolCall(call) = event else {
            panic!("expected a tool call")
        };
        assert_eq!(call.id, "c1");
        assert_eq!(call.kind, "edit");
        assert_eq!(call.status, "in_progress");
        assert_eq!(call.locations, ["/abs/report.md"]);
        assert_eq!(call.diff_paths, ["/abs/report.md"]);
        assert_eq!(call.raw_input.unwrap()["file"], "report.md");
    }

    /// An engine that omits `kind` and `status` must not be reported as having
    /// sent empty strings; ACP's own defaults apply.
    #[test]
    fn a_tool_call_without_kind_or_status_gets_acps_defaults() {
        let event = map_session_update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Something"
        }))
        .unwrap();
        let RawAgentEvent::ToolCall(call) = event else {
            panic!("expected a tool call")
        };
        assert_eq!(call.kind, "other");
        assert_eq!(call.status, "pending");
    }

    /// In ACP an absent field on an update means unchanged. Mapping it to an
    /// empty vector would erase the locations the original tool call reported.
    #[test]
    fn an_update_distinguishes_absent_from_empty() {
        let event = map_session_update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"
        }))
        .unwrap();
        let RawAgentEvent::ToolCallUpdate(update) = event else {
            panic!("expected an update")
        };
        assert_eq!(update.status.as_deref(), Some("completed"));
        assert_eq!(update.locations, None);
        assert_eq!(update.title, None);

        let cleared = map_session_update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c1", "locations": []
        }))
        .unwrap();
        let RawAgentEvent::ToolCallUpdate(update) = cleared else {
            panic!("expected an update")
        };
        assert_eq!(update.locations, Some(vec![]));
    }

    #[test]
    fn plan_updates_become_plan_entries() {
        let event = map_session_update(&json!({
            "sessionUpdate": "plan",
            "entries": [{"content": "Open the report", "priority": "high", "status": "pending"}]
        }))
        .unwrap();
        let RawAgentEvent::PlanEntries(entries) = event else {
            panic!("expected plan entries")
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Open the report");
        assert_eq!(entries[0].priority.as_deref(), Some("high"));
    }

    #[test]
    fn mode_changes_are_reported() {
        let event = map_session_update(&json!({
            "sessionUpdate": "current_mode_update", "currentModeId": "plan"
        }));
        assert_eq!(event, Some(RawAgentEvent::ModeChanged("plan".into())));
    }

    /// The rule from §6: an unknown `sessionUpdate` must not crash the client.
    #[test]
    fn unknown_updates_survive_as_other() {
        let event = map_session_update(&json!({
            "sessionUpdate": "some_update_from_2027", "payload": {"a": 1}
        }))
        .unwrap();
        assert!(matches!(event, RawAgentEvent::Other(_)));

        let usage =
            map_session_update(&json!({"sessionUpdate": "usage_update", "tokens": 12})).unwrap();
        assert!(matches!(usage, RawAgentEvent::Other(_)));

        // Not even a sessionUpdate key. Still not a crash.
        let nonsense = map_session_update(&json!({"hello": "world"})).unwrap();
        assert!(matches!(nonsense, RawAgentEvent::Other(_)));
    }

    /// A chunk carrying an image has no text; reporting it as an empty message
    /// would put a blank line in the transcript.
    #[test]
    fn a_non_text_content_chunk_is_not_an_empty_message() {
        let event = map_session_update(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "image", "data": "..."}
        }));
        assert_eq!(event, None);
    }

    #[test]
    fn responses_tolerate_missing_fields() {
        let init: InitializeResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(init.protocol_version, 0);
        assert!(!init.agent_capabilities.load_session);
        assert!(init.agent_info.is_none());

        let session: NewSessionResponse =
            serde_json::from_str(r#"{"sessionId":"s","unknownField":true}"#).unwrap();
        assert_eq!(session.session_id, "s");
        assert!(session.modes.is_none());
    }

    #[test]
    fn initialize_params_match_the_protocol_cheat_sheet() {
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], 1);
        assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], true);
        assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], true);
        assert_eq!(params["clientCapabilities"]["terminal"], false);
    }
}
