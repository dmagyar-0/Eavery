//! The permission policy: what a tool call is allowed to do without asking,
//! and what the plan gate refuses while the engine is only meant to be
//! looking (`docs/plan/06-plan-gate-permissions.md` §2.2 and §3).
//!
//! Everything here is a pure function of the call, the Project root and the
//! Connector registry. The turn engine (`crate::turn`) is the only caller
//! that acts on the answers; the ACP layer uses [`prompt_for`] to give the UI
//! a provisional answer before the turn engine has reclassified.
//!
//! The rows of §3.2 are also the prompt-injection defence
//! (`docs/plan/02-challenges.md` C11). There is no Everyday-mode "always" for
//! anything that runs a command or leaves the machine, an Outbound call is
//! never auto-allowed because the plan listed it, and a Connector's own idea
//! of how trusted it is never overrides its `outbound` flag. Those three are
//! tested here, and they should stay tested.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::McpServerSpec;
use crate::event::{Decision, PermissionView, ToolCallView};
use crate::model::{ProjectId, RiskClass, UiMode};

pub use crate::paths::is_inside;

/// One registered MCP server and what the policy knows about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connector {
    pub spec: McpServerSpec,
    /// Whether its tools can send anything off this computer. Set from
    /// `connectors.json` (M6-T08); the bundled document Connector is not.
    pub outbound: bool,
    /// The tool names it exposes, when known, so a call can be matched to it
    /// by tool as well as by the `mcp__<server>__` prefix engines put in
    /// titles and `rawInput`.
    #[serde(default)]
    pub tools: Vec<String>,
}

impl Connector {
    pub fn local(spec: McpServerSpec) -> Self {
        Self {
            spec,
            outbound: false,
            tools: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.spec.name()
    }
}

/// The Connectors an engine is handed in `session/new`, and the one fact the
/// policy needs about each (§3.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectorRegistry {
    connectors: Vec<Connector>,
}

impl ConnectorRegistry {
    pub fn new(connectors: Vec<Connector>) -> Self {
        Self { connectors }
    }

    /// Every server, as `session/new` wants them.
    pub fn specs(&self) -> Vec<McpServerSpec> {
        self.connectors
            .iter()
            .map(|connector| connector.spec.clone())
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Connector> {
        self.connectors.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }

    /// The Connector this call belongs to, if any.
    ///
    /// Engines name the server in the tool call one of two ways: an
    /// `mcp__<server>__<tool>` name in the title or `rawInput` (the Claude
    /// adapter, goose), or the bare tool name. Both are looked for, the
    /// second only as a whole word, so a Connector with a tool called `send`
    /// does not claim every call whose title contains "sender". Which form
    /// each engine uses is what M1-T04 to M1-T07 record.
    pub fn owning(&self, call: &CallFacts<'_>) -> Option<&Connector> {
        let haystack = call.haystack();
        self.connectors.iter().find(|connector| {
            let prefix = format!("mcp__{}__", connector.name().to_lowercase());
            haystack.contains(&prefix)
                || connector
                    .tools
                    .iter()
                    .any(|tool| contains_word(&haystack, &tool.to_lowercase()))
        })
    }
}

/// True when `word` appears in `text` bounded by something that is not part
/// of an identifier.
fn contains_word(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let is_boundary = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    text.match_indices(word).any(|(start, _)| {
        is_boundary(text[..start].chars().next_back())
            && is_boundary(text[start + word.len()..].chars().next())
    })
}

/// What the policy looks at in a tool call. Borrowed from whichever view
/// the caller has: a permission request or a tool call update.
#[derive(Clone, Copy, Debug)]
pub struct CallFacts<'a> {
    /// The ACP `kind`; `other` when the engine omitted it.
    pub kind: &'a str,
    pub title: &'a str,
    /// Absolute paths as the engine gave them. Empty means the engine did not
    /// say.
    pub locations: &'a [String],
    pub raw_input: Option<&'a serde_json::Value>,
}

impl<'a> CallFacts<'a> {
    /// The title and the raw input as one lowercase string, for matching
    /// Connector names and plan-exit signatures against.
    fn haystack(&self) -> String {
        let mut text = self.title.to_lowercase();
        if let Some(raw) = self.raw_input {
            text.push(' ');
            text.push_str(&raw.to_string().to_lowercase());
        }
        text
    }
}

impl<'a> From<&'a PermissionView> for CallFacts<'a> {
    fn from(view: &'a PermissionView) -> Self {
        Self {
            kind: &view.kind,
            title: &view.title,
            locations: &view.locations,
            raw_input: view.raw_input.as_ref(),
        }
    }
}

impl<'a> From<&'a ToolCallView> for CallFacts<'a> {
    fn from(view: &'a ToolCallView) -> Self {
        Self {
            kind: &view.kind,
            title: &view.title,
            locations: &view.locations,
            raw_input: None,
        }
    }
}

/// The risk table from §3.1.
///
/// Three rules are worth keeping in sight. A Connector marked outbound makes
/// every one of its calls `Outbound`, whatever the engine called the kind. A
/// mutation that names no file is `Destructive`, not `Reversible`: an engine
/// that will not say what it is about to touch is a reason to ask. And
/// anything unrecognised is `Execute`, never `Read`.
pub fn classify(
    call: &CallFacts<'_>,
    project_root: &Path,
    connectors: &ConnectorRegistry,
) -> RiskClass {
    if let Some(connector) = connectors.owning(call)
        && connector.outbound
    {
        return RiskClass::Outbound;
    }
    classify_kind(call.kind, call.locations, project_root)
}

/// [`classify`] without the Connector lookup, for a layer that has the
/// Project root and nothing else.
pub fn classify_kind(kind: &str, locations: &[String], project_root: &Path) -> RiskClass {
    match kind {
        "read" | "search" | "think" | "other" => RiskClass::Read,
        "edit" | "delete" | "move" => {
            if locations.is_empty() {
                return RiskClass::Destructive;
            }
            if locations.iter().all(|path| is_inside(path, project_root)) {
                RiskClass::Reversible
            } else {
                // Outside the Project is outside the Journal: nothing here
                // could take it back.
                RiskClass::Destructive
            }
        }
        "fetch" => RiskClass::Outbound,
        _ => RiskClass::Execute,
    }
}

/// Whether the permission dialog may offer "always" (the last column of
/// §3.2). Serialised for the UI, which knows the mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AlwaysOffer {
    Never,
    /// In either mode, per Project.
    Yes,
    DeveloperOnly,
}

impl AlwaysOffer {
    /// The safe default for a view built without the table.
    pub fn never() -> Self {
        AlwaysOffer::Never
    }

    pub fn allowed_in(self, mode: UiMode) -> bool {
        match self {
            AlwaysOffer::Never => false,
            AlwaysOffer::Yes => true,
            AlwaysOffer::DeveloperOnly => mode == UiMode::Developer,
        }
    }
}

/// How a question is put to the person, when one has to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub always: AlwaysOffer,
    /// The dialog's default button is Reject.
    pub default_reject: bool,
    /// For an Outbound call: whether the plan listed it. `None` when the
    /// question is not about something leaving the machine.
    pub in_plan: Option<bool>,
}

/// The middle column of §3.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Allowed by the policy without asking. Still written down.
    Allow,
    /// Only a person can answer this one.
    Ask(Prompt),
}

impl Verdict {
    pub fn always(self) -> AlwaysOffer {
        match self {
            Verdict::Allow => AlwaysOffer::Never,
            Verdict::Ask(prompt) => prompt.always,
        }
    }

    pub fn in_plan(self) -> Option<bool> {
        match self {
            Verdict::Allow => None,
            Verdict::Ask(prompt) => prompt.in_plan,
        }
    }
}

/// The decision table (§3.2). `in_plan` is [`listed_in_plan`] for an
/// Outbound call and ignored for every other class.
pub fn decide(risk: RiskClass, in_plan: bool) -> Verdict {
    match risk {
        // Undo covers it, so asking would be theatre.
        RiskClass::Read | RiskClass::Reversible => Verdict::Allow,
        RiskClass::Execute => Verdict::Ask(Prompt {
            always: AlwaysOffer::DeveloperOnly,
            default_reject: false,
            in_plan: None,
        }),
        RiskClass::Outbound => Verdict::Ask(Prompt {
            always: AlwaysOffer::Never,
            default_reject: false,
            in_plan: Some(in_plan),
        }),
        RiskClass::Destructive => Verdict::Ask(Prompt {
            always: AlwaysOffer::Never,
            default_reject: true,
            in_plan: None,
        }),
    }
}

/// The provisional prompt for a class the ACP layer guessed from the kind
/// alone, before the turn engine has reclassified.
pub fn prompt_for(risk: RiskClass) -> Verdict {
    decide(risk, false)
}

/// Whether "always" may be stored for this class in this mode. The core
/// enforces it as well as the UI: an `AllowAlways` the table does not permit
/// is narrowed to `AllowOnce` before it is remembered or sent.
pub fn always_allowed(risk: RiskClass, mode: UiMode) -> bool {
    decide(risk, false).always().allowed_in(mode)
}

/// Narrows a person's answer to what the table permits for this class.
pub fn narrow(decision: Decision, risk: RiskClass, mode: UiMode) -> Decision {
    match decision {
        Decision::AllowAlways if !always_allowed(risk, mode) => Decision::AllowOnce,
        other => other,
    }
}

/// Whether the plan's `outbound` list covers this call (§3.2, the two
/// Outbound rows). A plan lists outbound actions in sentences, and a tool
/// call names itself in a title, so the match is loose on purpose: either
/// contains the other, or the Connector the call belongs to is named in the
/// sentence. The answer only ever changes the wording of a question that is
/// asked either way.
pub fn listed_in_plan(
    plan_outbound: &[String],
    call: &CallFacts<'_>,
    connector: Option<&str>,
) -> bool {
    let title = normalise(call.title);
    plan_outbound.iter().any(|sentence| {
        let sentence = normalise(sentence);
        if sentence.is_empty() || title.is_empty() {
            return false;
        }
        sentence.contains(&title)
            || title.contains(&sentence)
            || connector.is_some_and(|name| sentence.contains(&normalise(name)))
    })
}

/// The key an "always" decision is stored under (§3.3): the kind, the
/// Connector, and the title with its case and spacing normalised, so the
/// same tool asked the same way is recognised next time.
pub fn signature(call: &CallFacts<'_>, connector: Option<&str>) -> String {
    format!(
        "{}|{}|{}",
        call.kind,
        connector.unwrap_or_default(),
        normalise(call.title)
    )
}

/// The settings key under which a Project's "always" signatures are kept, as
/// a `Vec<String>`.
pub fn always_key(project_id: ProjectId) -> String {
    format!("always_allow:{project_id}")
}

fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ---- the plan gate (§2.2) --------------------------------------------------

/// What `fs/write_text_file` is refused with during planning. The wording is
/// §2.2's.
pub const PLANNING_WRITE_REFUSAL: &str = "Eavery is in planning mode; no changes are allowed yet";

/// A call the plan gate refused, for the digest and the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PlanGateRefusal {
    pub tool_call_id: String,
    pub title: String,
}

/// The plan gate's answer to one permission request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanGateVerdict {
    Allow,
    Refuse(PlanGateRefusal),
}

/// The client-side gate for `session/request_permission` during the plan
/// phase (§2.2). Reads go through, from anywhere (D15); everything that
/// would change, run or send something is refused; and so is any attempt to
/// leave plan mode, whatever kind the engine gave it, because allowing that
/// lets the engine start executing inside the plan prompt.
///
/// `exit_signatures` is the engine's `plan_exit_signatures`
/// (`04-acp-engines.md` §2), matched case-insensitively against the title and
/// the raw input.
pub fn plan_gate(
    call: &CallFacts<'_>,
    tool_call_id: &str,
    exit_signatures: &[&str],
) -> PlanGateVerdict {
    let refusal = || {
        PlanGateVerdict::Refuse(PlanGateRefusal {
            tool_call_id: tool_call_id.to_owned(),
            title: call.title.to_owned(),
        })
    };

    if is_plan_exit(call, exit_signatures) {
        return refusal();
    }
    match call.kind {
        "read" | "search" | "think" | "other" => PlanGateVerdict::Allow,
        _ => refusal(),
    }
}

/// Whether this call is the engine trying to leave plan mode.
pub fn is_plan_exit(call: &CallFacts<'_>, exit_signatures: &[&str]) -> bool {
    if exit_signatures.is_empty() {
        return false;
    }
    let haystack = call.haystack();
    exit_signatures
        .iter()
        .filter(|signature| !signature.is_empty())
        .any(|signature| haystack.contains(&signature.to_lowercase()))
}

/// Whether a tool call update during planning means the engine changed
/// something without asking (§2.2, last paragraph): a mutation that reached
/// `completed` with no permission request on the way.
pub fn bypassed_plan_gate(kind: &str, status: &str) -> bool {
    matches!(kind, "edit" | "delete" | "move") && status == "completed"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        let dir = std::env::temp_dir().join("eavery-policy");
        std::fs::create_dir_all(&dir).unwrap();
        crate::paths::canonical_or_self(&dir)
    }

    fn call<'a>(kind: &'a str, title: &'a str, locations: &'a [String]) -> CallFacts<'a> {
        CallFacts {
            kind,
            title,
            locations,
            raw_input: None,
        }
    }

    fn connector(name: &str, outbound: bool, tools: &[&str]) -> Connector {
        Connector {
            spec: McpServerSpec::Stdio {
                name: name.to_owned(),
                command: PathBuf::from("/bin/true"),
                args: vec![],
                env: vec![],
            },
            outbound,
            tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        }
    }

    // ---- classify: every row of §3.1 ----

    #[test]
    fn reads_are_read_whatever_they_read() {
        let root = root();
        let none = ConnectorRegistry::default();
        for kind in ["read", "search", "think", "other"] {
            assert_eq!(
                classify(&call(kind, "", &[]), &root, &none),
                RiskClass::Read,
                "{kind}"
            );
        }
        let outside = vec!["/somewhere/else.txt".to_owned()];
        assert_eq!(
            classify(&call("read", "", &outside), &root, &none),
            RiskClass::Read
        );
    }

    #[test]
    fn an_edit_inside_the_project_is_reversible_and_outside_it_is_not() {
        let root = root();
        let none = ConnectorRegistry::default();
        let inside = vec![root.join("report.docx").display().to_string()];
        for kind in ["edit", "delete", "move"] {
            assert_eq!(
                classify(&call(kind, "", &inside), &root, &none),
                RiskClass::Reversible,
                "{kind}"
            );
        }
        let mixed = vec![inside[0].clone(), "/etc/hosts".to_owned()];
        assert_eq!(
            classify(&call("delete", "", &mixed), &root, &none),
            RiskClass::Destructive,
            "one file outside the Project is enough: Undo could not take it back"
        );
    }

    /// An engine that will not say what it is about to change is a reason to
    /// ask, not a reason to relax.
    #[test]
    fn an_edit_that_names_no_file_is_destructive() {
        assert_eq!(
            classify(
                &call("edit", "", &[]),
                &root(),
                &ConnectorRegistry::default()
            ),
            RiskClass::Destructive
        );
    }

    #[test]
    fn fetch_is_outbound_and_anything_unrecognised_is_execute_never_read() {
        let root = root();
        let none = ConnectorRegistry::default();
        assert_eq!(
            classify(&call("fetch", "", &[]), &root, &none),
            RiskClass::Outbound
        );
        assert_eq!(
            classify(&call("execute", "", &[]), &root, &none),
            RiskClass::Execute
        );
        assert_eq!(
            classify(&call("something_new", "", &[]), &root, &none),
            RiskClass::Execute
        );
        assert_eq!(
            classify(&call("", "", &[]), &root, &none),
            RiskClass::Execute
        );
    }

    /// The `\\?\C:\` case §3.1 asks for, run on every platform. Without the
    /// normalisation every edit on Windows would be Destructive.
    #[test]
    fn a_verbatim_windows_root_still_makes_edits_reversible() {
        let root = Path::new(r"\\?\C:\Users\me\Project");
        let none = ConnectorRegistry::default();
        let inside = vec![r"C:\Users\me\Project\report.docx".to_owned()];
        assert_eq!(
            classify(&call("edit", "", &inside), root, &none),
            RiskClass::Reversible
        );
        let verbatim = vec![r"\\?\C:\Users\me\Project\new\report.docx".to_owned()];
        assert_eq!(
            classify(&call("edit", "", &verbatim), root, &none),
            RiskClass::Reversible
        );
        let outside = vec![r"C:\Users\me\Elsewhere\report.docx".to_owned()];
        assert_eq!(
            classify(&call("edit", "", &outside), root, &none),
            RiskClass::Destructive
        );
    }

    // ---- connectors ----

    #[test]
    fn an_outbound_connector_makes_every_one_of_its_calls_outbound() {
        let root = root();
        let registry = ConnectorRegistry::new(vec![
            connector("eavery-docs", false, &["docx_read_text"]),
            connector("gmail", true, &["send_email"]),
        ]);
        let inside = vec![root.join("x").display().to_string()];

        // The prefix form, whatever the kind says.
        let mail = call("other", "mcp__gmail__send_email", &[]);
        assert_eq!(classify(&mail, &root, &registry), RiskClass::Outbound);
        assert_eq!(registry.owning(&mail).unwrap().name(), "gmail");

        // The bare tool name, as a whole word, in the title...
        let mail = call("edit", "Run send_email for Sam", &inside);
        assert_eq!(classify(&mail, &root, &registry), RiskClass::Outbound);

        // ...or in the raw input.
        let raw = serde_json::json!({"tool": "send_email", "to": "sam@example.com"});
        let mail = CallFacts {
            raw_input: Some(&raw),
            ..call("other", "Tool", &[])
        };
        assert_eq!(classify(&mail, &root, &registry), RiskClass::Outbound);

        // A local Connector's calls are classified on their kind as usual.
        let read = call("read", "mcp__eavery-docs__docx_read_text", &[]);
        assert_eq!(registry.owning(&read).unwrap().name(), "eavery-docs");
        assert_eq!(classify(&read, &root, &registry), RiskClass::Read);

        // Nothing matches: no Connector.
        assert!(
            registry
                .owning(&call("read", "Read report.docx", &[]))
                .is_none()
        );
    }

    #[test]
    fn a_tool_name_matches_only_as_a_whole_word() {
        let registry = ConnectorRegistry::new(vec![connector("mail", true, &["send"])]);
        assert!(
            registry
                .owning(&call("other", "sender list", &[]))
                .is_none()
        );
        assert!(registry.owning(&call("other", "resend", &[])).is_none());
        assert!(
            registry
                .owning(&call("other", "send: report", &[]))
                .is_some()
        );
        assert!(registry.owning(&call("other", "Send", &[])).is_some());
    }

    #[test]
    fn the_registry_hands_the_session_its_specs() {
        let registry =
            ConnectorRegistry::new(vec![connector("a", false, &[]), connector("b", true, &[])]);
        let names: Vec<String> = registry
            .specs()
            .iter()
            .map(|spec| spec.name().to_owned())
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(ConnectorRegistry::default().is_empty());
        assert_eq!(
            registry
                .iter()
                .filter(|connector| connector.outbound)
                .count(),
            1
        );
    }

    // ---- decide: every row of §3.2 ----

    #[test]
    fn the_policy_answers_for_what_undo_covers_and_nothing_else() {
        assert_eq!(decide(RiskClass::Read, false), Verdict::Allow);
        assert_eq!(decide(RiskClass::Reversible, false), Verdict::Allow);
        for risk in [
            RiskClass::Execute,
            RiskClass::Outbound,
            RiskClass::Destructive,
        ] {
            assert!(
                matches!(decide(risk, false), Verdict::Ask(_)),
                "{risk:?} is not ours to allow"
            );
            assert!(
                matches!(decide(risk, true), Verdict::Ask(_)),
                "{risk:?} is not ours to allow, in the plan or not"
            );
        }
    }

    #[test]
    fn always_is_offered_where_the_table_says_and_nowhere_else() {
        assert_eq!(
            decide(RiskClass::Execute, false).always(),
            AlwaysOffer::DeveloperOnly
        );
        assert_eq!(
            decide(RiskClass::Outbound, false).always(),
            AlwaysOffer::Never
        );
        assert_eq!(
            decide(RiskClass::Outbound, true).always(),
            AlwaysOffer::Never
        );
        assert_eq!(
            decide(RiskClass::Destructive, false).always(),
            AlwaysOffer::Never
        );

        // Reversible is "yes (per Project)" on the table, but it never
        // reaches a dialog, so there is nothing to offer.
        assert_eq!(
            decide(RiskClass::Reversible, false).always(),
            AlwaysOffer::Never
        );

        assert!(always_allowed(RiskClass::Execute, UiMode::Developer));
        assert!(!always_allowed(RiskClass::Execute, UiMode::Everyday));
        for mode in [UiMode::Everyday, UiMode::Developer] {
            assert!(!always_allowed(RiskClass::Outbound, mode));
            assert!(!always_allowed(RiskClass::Destructive, mode));
        }
    }

    #[test]
    fn an_outbound_question_says_whether_the_plan_listed_it() {
        assert_eq!(decide(RiskClass::Outbound, true).in_plan(), Some(true));
        assert_eq!(decide(RiskClass::Outbound, false).in_plan(), Some(false));
        assert_eq!(decide(RiskClass::Execute, true).in_plan(), None);
        assert_eq!(decide(RiskClass::Read, true).in_plan(), None);
    }

    #[test]
    fn destructive_defaults_to_reject() {
        let Verdict::Ask(prompt) = decide(RiskClass::Destructive, false) else {
            panic!("destructive is asked");
        };
        assert!(prompt.default_reject);
        let Verdict::Ask(prompt) = decide(RiskClass::Execute, false) else {
            panic!("execute is asked");
        };
        assert!(!prompt.default_reject);
    }

    /// C11 again: the core narrows an "always" the table forbids, whatever
    /// the UI sent.
    #[test]
    fn a_forbidden_always_is_narrowed_to_once() {
        assert_eq!(
            narrow(
                Decision::AllowAlways,
                RiskClass::Outbound,
                UiMode::Developer
            ),
            Decision::AllowOnce
        );
        assert_eq!(
            narrow(
                Decision::AllowAlways,
                RiskClass::Destructive,
                UiMode::Developer
            ),
            Decision::AllowOnce
        );
        assert_eq!(
            narrow(Decision::AllowAlways, RiskClass::Execute, UiMode::Everyday),
            Decision::AllowOnce
        );
        assert_eq!(
            narrow(Decision::AllowAlways, RiskClass::Execute, UiMode::Developer),
            Decision::AllowAlways
        );
        assert_eq!(
            narrow(
                Decision::RejectAlways,
                RiskClass::Outbound,
                UiMode::Everyday
            ),
            Decision::RejectAlways
        );
        assert_eq!(
            narrow(Decision::AllowOnce, RiskClass::Outbound, UiMode::Everyday),
            Decision::AllowOnce
        );
    }

    #[test]
    fn listed_in_plan_is_a_loose_match_in_either_direction() {
        let plan = vec!["Send the report to Sam by email".to_owned()];
        assert!(listed_in_plan(
            &plan,
            &call("fetch", "send the report to sam", &[]),
            None
        ));
        assert!(listed_in_plan(
            &plan,
            &call("fetch", "Send  the report to Sam by EMAIL, then wait", &[]),
            None
        ));
        assert!(listed_in_plan(
            &plan,
            &call("other", "mcp__gmail__send", &[]),
            Some("email")
        ));
        assert!(!listed_in_plan(
            &plan,
            &call("fetch", "Post to Slack", &[]),
            Some("slack")
        ));
        assert!(!listed_in_plan(&[], &call("fetch", "anything", &[]), None));
        assert!(!listed_in_plan(&plan, &call("fetch", "", &[]), None));
    }

    #[test]
    fn a_signature_is_the_kind_the_connector_and_the_normalised_title() {
        let inside: Vec<String> = vec![];
        assert_eq!(
            signature(&call("execute", "  Run   npm TEST ", &inside), None),
            "execute||run npm test"
        );
        assert_eq!(
            signature(
                &call("other", "docx_read_text", &inside),
                Some("eavery-docs")
            ),
            "other|eavery-docs|docx_read_text"
        );
        assert!(always_key(uuid::Uuid::nil()).starts_with("always_allow:"));
    }

    // ---- the plan gate: every row of §2.2 ----

    #[test]
    fn the_plan_gate_lets_looking_through_and_refuses_the_rest() {
        for kind in ["read", "search", "think", "other"] {
            assert_eq!(
                plan_gate(&call(kind, "Look", &[]), "t1", &[]),
                PlanGateVerdict::Allow,
                "{kind}"
            );
        }
        for kind in [
            "edit",
            "delete",
            "move",
            "execute",
            "fetch",
            "",
            "brand_new",
        ] {
            assert_eq!(
                plan_gate(&call(kind, "Change it", &[]), "t1", &[]),
                PlanGateVerdict::Refuse(PlanGateRefusal {
                    tool_call_id: "t1".into(),
                    title: "Change it".into(),
                }),
                "{kind}"
            );
        }
    }

    /// D15: reads outside the Project go through during planning too. The
    /// gate does not look at locations at all.
    #[test]
    fn the_plan_gate_does_not_care_where_a_read_reads() {
        let outside = vec!["/etc/hosts".to_owned()];
        assert_eq!(
            plan_gate(&call("read", "Read hosts", &outside), "t1", &[]),
            PlanGateVerdict::Allow
        );
    }

    /// Leaving plan mode is refused whatever kind it arrives with, matched on
    /// the title or the raw input, ignoring case.
    #[test]
    fn leaving_plan_mode_is_refused_whatever_its_kind() {
        let signatures = ["ExitPlanMode", "exit_plan_mode"];
        let exit = call("other", "ExitPlanMode", &[]);
        assert!(matches!(
            plan_gate(&exit, "t1", &signatures),
            PlanGateVerdict::Refuse(_)
        ));
        let exit = call("read", "exitplanmode: ready to go", &[]);
        assert!(matches!(
            plan_gate(&exit, "t1", &signatures),
            PlanGateVerdict::Refuse(_)
        ));

        let raw = serde_json::json!({"tool": "exit_plan_mode"});
        let exit = CallFacts {
            raw_input: Some(&raw),
            ..call("think", "Tool", &[])
        };
        assert!(is_plan_exit(&exit, &signatures));
        assert!(matches!(
            plan_gate(&exit, "t1", &signatures),
            PlanGateVerdict::Refuse(_)
        ));

        // No signatures for this engine: nothing is a plan exit, and an
        // empty signature must not match everything.
        assert!(!is_plan_exit(&call("other", "ExitPlanMode", &[]), &[]));
        assert!(!is_plan_exit(&call("other", "ExitPlanMode", &[]), &[""]));
        assert_eq!(
            plan_gate(&call("read", "Read it", &[]), "t1", &signatures),
            PlanGateVerdict::Allow
        );
    }

    #[test]
    fn a_completed_mutation_during_planning_is_a_bypass() {
        assert!(bypassed_plan_gate("edit", "completed"));
        assert!(bypassed_plan_gate("delete", "completed"));
        assert!(!bypassed_plan_gate("edit", "in_progress"));
        assert!(!bypassed_plan_gate("edit", "failed"));
        assert!(!bypassed_plan_gate("read", "completed"));
        assert!(
            !bypassed_plan_gate("execute", "completed"),
            "a command is the engine's asking mode's business, and the Journal's"
        );
    }
}
