//! Getting a [`Plan`] out of what the engine said
//! (`docs/plan/06-plan-gate-permissions.md` §2.3).
//!
//! The plan prompt asks for a fenced `eavery-plan` block of JSON at the end
//! of the reply. Engines mostly comply; the rule here is that when one does
//! not, the person still gets something to approve. Parsing never fails the
//! turn: the worst case is the engine's own words as the plan, with its
//! bullet points as the steps.

use crate::model::{Plan, PlanJson, PlanStep};

/// The info string the plan prompt asks for.
pub const FENCE_INFO: &str = "eavery-plan";

/// The plan in `text`, the engine's whole reply for the plan phase.
///
/// 1. The last fenced block whose info string is `eavery-plan`, parsed as
///    JSON with every field optional.
/// 2. Otherwise the reply itself: `raw_markdown` is all of it, `summary` its
///    first non-empty line, and `steps` its list items, if it has any.
///
/// `raw_markdown` is kept in both cases, so the UI can always show what the
/// engine actually said.
pub fn extract(text: &str) -> Plan {
    let mut plan = match last_plan_block(text).and_then(parse_block) {
        Some(plan) => plan,
        None => fallback(text),
    };
    if plan.summary.is_empty() {
        plan.summary = first_line(text).unwrap_or_default();
    }
    plan.raw_markdown = text.to_owned();
    plan
}

/// The body of the last complete ```` ```eavery-plan ```` block.
fn last_plan_block(text: &str) -> Option<String> {
    let mut last = None;
    let mut open: Option<Vec<&str>> = None;
    let mut wanted = false;

    for line in text.lines() {
        match &mut open {
            None => {
                if let Some(info) = fence_info(line) {
                    wanted = info == FENCE_INFO;
                    open = Some(Vec::new());
                }
            }
            Some(body) => {
                if fence_info(line) == Some("") {
                    if wanted {
                        last = Some(body.join("\n"));
                    }
                    open = None;
                } else {
                    body.push(line);
                }
            }
        }
    }
    last
}

/// The info string of a fence line, `Some("")` for a bare fence, `None` for
/// any other line. Up to three spaces of indentation are allowed, as in
/// CommonMark; a longer fence (four or more backticks) is accepted too.
fn fence_info(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix("```")?;
    Some(rest.trim_start_matches('`').trim())
}

fn parse_block(body: String) -> Option<Plan> {
    match serde_json::from_str::<PlanJson>(&body) {
        Ok(json) => Some(Plan::from(json)),
        Err(error) => {
            tracing::warn!(%error, "the eavery-plan block is not valid JSON; using the reply as the plan");
            None
        }
    }
}

/// The reply as a plan: first line as summary, list items as steps.
fn fallback(text: &str) -> Plan {
    Plan {
        summary: first_line(text).unwrap_or_default(),
        steps: list_items(text).into_iter().map(PlanStep::new).collect(),
        ..Plan::default()
    }
}

/// The first non-empty line outside any fenced block, without a Markdown
/// heading marker.
fn first_line(text: &str) -> Option<String> {
    prose_lines(text)
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Every `- `, `* ` or `1. ` item outside a fenced block, with a task-list
/// checkbox stripped.
fn list_items(text: &str) -> Vec<String> {
    prose_lines(text)
        .filter_map(list_item)
        .filter(|item| !item.is_empty())
        .collect()
}

fn list_item(line: &str) -> Option<String> {
    let line = line.trim();
    let item = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| numbered_item(line))?;
    let item = item
        .trim_start()
        .strip_prefix("[ ] ")
        .or_else(|| item.trim_start().strip_prefix("[x] "))
        .or_else(|| item.trim_start().strip_prefix("[X] "))
        .unwrap_or(item);
    Some(item.trim().to_owned())
}

fn numbered_item(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// The lines that are not inside a fenced block. Fences themselves are
/// skipped too.
fn prose_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut inside = false;
    text.lines().filter(move |line| {
        if fence_info(line).is_some() {
            inside = !inside;
            return false;
        }
        !inside
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "I looked at the folder.\n\nI will update the report.\n\n```eavery-plan\n{\"summary\":\"Update report\",\"steps\":[\"Open report\",\"Change FY25 to FY26\"],\"files_touched\":[\"report.docx\"],\"outbound\":[],\"irreversible\":[],\"will_not_do\":[\"send email\"]}\n```";

    #[test]
    fn a_valid_block_becomes_the_plan_and_the_reply_is_kept() {
        let plan = extract(REPLY);
        assert_eq!(plan.summary, "Update report");
        assert_eq!(
            plan.steps,
            vec![
                PlanStep::new("Open report"),
                PlanStep::new("Change FY25 to FY26")
            ]
        );
        assert_eq!(plan.files_touched, vec!["report.docx"]);
        assert_eq!(plan.will_not_do, vec!["send email"]);
        assert_eq!(plan.raw_markdown, REPLY);
        assert_eq!(plan.user_edits, None);
    }

    /// The prompt says "exactly one", and engines that think aloud sometimes
    /// draft one, change their mind, and write another. The last one is the
    /// plan.
    #[test]
    fn the_last_block_wins() {
        let text = "```eavery-plan\n{\"summary\":\"first draft\"}\n```\nOn reflection:\n```eavery-plan\n{\"summary\":\"final\"}\n```\n";
        assert_eq!(extract(text).summary, "final");
    }

    #[test]
    fn other_fenced_blocks_are_not_mistaken_for_the_plan() {
        let text = "Here is some JSON:\n```json\n{\"summary\":\"not it\"}\n```\n- Do the thing\n";
        let plan = extract(text);
        assert_eq!(plan.summary, "Here is some JSON:");
        assert_eq!(plan.steps, vec![PlanStep::new("Do the thing")]);
    }

    #[test]
    fn an_indented_or_longer_fence_still_counts() {
        let text = "   ````eavery-plan\n   {\"summary\":\"indented\"}\n   ````";
        assert_eq!(extract(text).summary, "indented");
    }

    /// Test 4 from `06` §7: malformed JSON falls back to the reply, with the
    /// steps taken from its list.
    #[test]
    fn malformed_json_falls_back_to_the_markdown_list() {
        let text = "# Plan\n\nI would do three things:\n\n1. Open the report\n2) Change the year\n- [ ] Save it\n* Tell you\n\n```eavery-plan\n{\"summary\": \"broken\", \"steps\": [\n```\n";
        let plan = extract(text);
        assert_eq!(plan.summary, "Plan");
        assert_eq!(
            plan.steps,
            vec![
                PlanStep::new("Open the report"),
                PlanStep::new("Change the year"),
                PlanStep::new("Save it"),
                PlanStep::new("Tell you"),
            ]
        );
        assert!(plan.files_touched.is_empty());
        assert_eq!(plan.raw_markdown, text);
    }

    #[test]
    fn an_unclosed_block_is_not_a_block() {
        let text = "Working on it\n```eavery-plan\n{\"summary\":\"cut off\"}";
        let plan = extract(text);
        assert_eq!(plan.summary, "Working on it");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn a_block_with_no_summary_takes_the_first_line_of_the_reply() {
        let text = "I'll rename the files.\n```eavery-plan\n{\"steps\":[\"Rename\"]}\n```";
        let plan = extract(text);
        assert_eq!(plan.summary, "I'll rename the files.");
        assert_eq!(plan.steps, vec![PlanStep::new("Rename")]);
    }

    /// Never fail the turn on plan parsing (§2.3 rule 3): even nothing at all
    /// is a plan, just an empty one.
    #[test]
    fn an_empty_reply_is_an_empty_plan() {
        let plan = extract("");
        assert_eq!(plan, Plan::default());
        let plan = extract("   \n\n");
        assert_eq!(plan.summary, "");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn list_items_inside_a_code_block_are_not_steps() {
        let text = "Notes\n```\n- not a step\n```\n- a step\n";
        assert_eq!(extract(text).steps, vec![PlanStep::new("a step")]);
    }
}
