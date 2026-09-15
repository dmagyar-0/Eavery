//! The script the fake agent replays, per `docs/plan/11-testing-ci.md` §2.
//!
//! Script keys follow the wire they describe: ACP payload fields are
//! `camelCase` (`toolCallId`, `agentInfo`), action names are `snake_case`
//! (`tool_call`, `request_permission`), because that is how they read in the
//! plan document and how a test author writes them.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Script {
    pub initialize: InitializeScript,
    pub session: SessionScript,
    pub turns: Vec<TurnScript>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeScript {
    /// `agentInfo` in the initialize response. `None` omits it.
    pub agent_info: Option<Value>,
    pub load_session: bool,
    pub auth_methods: Vec<Value>,
    /// Override the protocol version, so a client's version check can be tested.
    pub protocol_version: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionScript {
    /// Passed through verbatim as `modes` in the `session/new` response.
    /// Omitted when absent, which is the common case: `modes` is optional.
    pub modes: Option<Value>,
    /// Fixes the session id, for tests that assert on it.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TurnScript {
    /// A case-insensitive substring of the prompt. Absent matches anything.
    #[serde(rename = "match")]
    pub matches: Option<String>,
    /// A turn is consumed when used, unless it repeats.
    pub repeat: bool,
    pub actions: Vec<Action>,
}

/// One scripted step. Externally tagged, so each action is an object with
/// exactly one key: `{"text": "hello"}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// An `agent_message_chunk`.
    Text(String),
    /// An `agent_thought_chunk`.
    Thought(String),
    ToolCall(ToolCallAction),
    ToolCallUpdate(ToolCallAction),
    /// A `plan` update: either a bare array of entries or `{"entries": [...]}`.
    Plan(Value),
    /// A `current_mode_update`.
    Mode(String),
    RequestPermission(PermissionAction),
    /// Write through the client's `fs/write_text_file`.
    FsWrite(FileAction),
    /// Read through the client's `fs/read_text_file`, to exercise the client's
    /// read handler and the D15 rule about reads outside the Project.
    FsRead(ReadAction),
    /// Write the file directly, simulating an engine that bypasses the client.
    WriteDirect(FileAction),
    SleepMs(u64),
    /// Exit the process, simulating a crash mid-turn.
    Exit(i32),
    /// End the turn with this stop reason.
    Stop(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallAction {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    /// Plain paths in the script; the agent wraps them as ACP
    /// `{"path": ...}` locations on the way out.
    pub locations: Option<Vec<String>>,
    pub content: Option<Vec<Value>>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionAction {
    pub tool_call_id: String,
    pub title: String,
    pub kind: Option<String>,
    pub locations: Option<Vec<String>>,
    /// Override the offered options. Absent means the three standard ones.
    pub options: Option<Vec<Value>>,
    /// The option `kind` the client is expected to pick. A different answer
    /// exits the process with code 3, so a policy regression fails loudly
    /// instead of passing quietly.
    pub expect: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileAction {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReadAction {
    pub path: String,
    /// If set, the read is expected to fail; a success exits with code 3.
    pub expect_refused: bool,
}

/// Walks the script, remembering which turns have been spent.
#[derive(Debug)]
pub struct ScriptRunner {
    script: Script,
    used: Vec<bool>,
}

impl ScriptRunner {
    pub fn new(script: Script) -> Self {
        let used = vec![false; script.turns.len()];
        Self { script, used }
    }

    pub fn script(&self) -> &Script {
        &self.script
    }

    /// The first unspent turn whose `match` is a case-insensitive substring of
    /// `prompt`. Marks it spent unless it repeats.
    pub fn take_turn(&mut self, prompt: &str) -> Option<TurnScript> {
        let needle = prompt.to_lowercase();
        let index = self.script.turns.iter().enumerate().position(|(i, turn)| {
            if self.used[i] && !turn.repeat {
                return false;
            }
            match &turn.matches {
                Some(m) => needle.contains(&m.to_lowercase()),
                None => true,
            }
        })?;
        self.used[index] = true;
        Some(self.script.turns[index].clone())
    }
}

/// Replaces `{{cwd}}` with the session's working directory.
pub fn substitute(text: &str, cwd: &str) -> String {
    text.replace("{{cwd}}", cwd)
}

/// The same, applied to every string anywhere inside a JSON value.
pub fn substitute_value(value: &Value, cwd: &str) -> Value {
    match value {
        Value::String(s) => Value::String(substitute(s, cwd)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| substitute_value(v, cwd)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute_value(v, cwd)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The script from `docs/plan/11-testing-ci.md` §2, verbatim. If the plan's
    /// own example stops parsing, either the plan or this parser is wrong.
    const PLAN_EXAMPLE: &str = r#"
{
  "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"}, "loadSession": false },
  "session": { "modes": { "currentModeId": "work", "availableModes": [
      {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"} ] } },
  "turns": [
    { "match": "plan", "actions": [
        {"thought": "Looking around"},
        {"tool_call": {"id": "t1", "title": "Read report.docx", "kind": "read", "status": "completed", "locations": ["{{cwd}}/report.docx"]}},
        {"request_permission": {"toolCallId": "t2", "title": "Edit report.docx", "kind": "edit", "locations": ["{{cwd}}/report.docx"], "expect": "reject_once"}},
        {"text": "I will update the report.\n\n```eavery-plan\n{\"summary\":\"Update report\",\"steps\":[\"Open report\",\"Change FY25 to FY26\"],\"files_touched\":[\"report.docx\"],\"outbound\":[],\"irreversible\":[],\"will_not_do\":[\"send email\"]}\n```"},
        {"stop": "end_turn"} ] },
    { "match": "approved this plan", "actions": [
        {"request_permission": {"toolCallId": "t3", "title": "Edit report.docx", "kind": "edit", "locations": ["{{cwd}}/report.docx"], "expect": "allow_once"}},
        {"fs_write": {"path": "{{cwd}}/report.docx", "text": "FY26"}},
        {"tool_call": {"id": "t3", "title": "Edit report.docx", "kind": "edit", "status": "completed"}},
        {"text": "Done. Changed one number."},
        {"stop": "end_turn"} ] }
  ]
}
"#;

    fn plan_example() -> Script {
        serde_json::from_str(PLAN_EXAMPLE).expect("the plan's own example must parse")
    }

    #[test]
    fn the_plan_example_script_parses() {
        let script = plan_example();
        assert!(!script.initialize.load_session);
        assert_eq!(script.initialize.agent_info.unwrap()["name"], "fake");
        assert_eq!(script.session.modes.unwrap()["currentModeId"], "work");
        assert_eq!(script.turns.len(), 2);
        assert_eq!(script.turns[0].actions.len(), 5);
        assert!(matches!(script.turns[0].actions[0], Action::Thought(_)));
        assert!(matches!(script.turns[0].actions[4], Action::Stop(ref r) if r == "end_turn"));
    }

    #[test]
    fn matching_is_case_insensitive_and_by_substring() {
        let mut runner = ScriptRunner::new(plan_example());
        let turn = runner
            .take_turn("Please PLAN the work")
            .expect("first turn matches");
        assert_eq!(turn.matches.as_deref(), Some("plan"));
    }

    #[test]
    fn turns_are_consumed_in_order() {
        let mut runner = ScriptRunner::new(plan_example());
        assert!(runner.take_turn("plan it").is_some());
        // The first turn is spent, and the second does not match this prompt.
        assert!(runner.take_turn("plan it").is_none());
        assert!(runner.take_turn("The user approved this plan").is_some());
        assert!(runner.take_turn("The user approved this plan").is_none());
    }

    #[test]
    fn a_repeating_turn_is_never_spent() {
        let script: Script = serde_json::from_str(
            r#"{"turns":[{"match":"hi","repeat":true,"actions":[{"stop":"end_turn"}]}]}"#,
        )
        .unwrap();
        let mut runner = ScriptRunner::new(script);
        for _ in 0..3 {
            assert!(runner.take_turn("hi there").is_some());
        }
    }

    #[test]
    fn a_turn_without_a_match_matches_anything() {
        let script: Script =
            serde_json::from_str(r#"{"turns":[{"actions":[{"stop":"end_turn"}]}]}"#).unwrap();
        let mut runner = ScriptRunner::new(script);
        assert!(runner.take_turn("literally anything").is_some());
    }

    #[test]
    fn cwd_is_substituted_everywhere() {
        assert_eq!(substitute("{{cwd}}/a.txt", "/tmp/p"), "/tmp/p/a.txt");
        let value = json!({"locations": ["{{cwd}}/a"], "n": 1, "deep": {"p": "{{cwd}}"}});
        let out = substitute_value(&value, "/tmp/p");
        assert_eq!(out["locations"][0], "/tmp/p/a");
        assert_eq!(out["deep"]["p"], "/tmp/p");
        assert_eq!(out["n"], 1);
    }

    /// A typo in an action name must fail at load time, not silently replay a
    /// shorter script than the test author wrote.
    #[test]
    fn unknown_keys_are_rejected() {
        assert!(
            serde_json::from_str::<Script>(r#"{"turns":[{"actions":[{"txet":"oops"}]}]}"#).is_err()
        );
        assert!(serde_json::from_str::<Script>(r#"{"turnz":[]}"#).is_err());
    }
}
