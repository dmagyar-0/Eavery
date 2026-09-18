//! The two prompts a turn sends, and the renderer that fills them in
//! (`docs/plan/06-plan-gate-permissions.md` §4).
//!
//! The templates live beside this file as Markdown so they can be read and
//! edited as prose. The renderer knows two forms and no more: `{{key}}`, and
//! `{{#if key}}…{{/if}}`, which keeps its body only when `key` is set to
//! something non-empty. No template engine: the two prompts are the whole
//! demand, and a dependency that can do more is a dependency that will be
//! asked to.

use std::collections::HashMap;

use crate::model::Plan;

/// The plan-phase prompt. Variables: `project_root`, `request`, `playbooks`.
pub const PLAN: &str = include_str!("prompts/plan.md");

/// The execute-phase prompt. Variables: `plan`, `project_root`, and the
/// optional `user_edits`.
pub const EXECUTE: &str = include_str!("prompts/execute.md");

/// What the plan prompt says when no Playbook applies. A `{{playbooks}}` left
/// blank would read as a list that was cut off.
pub const NO_PLAYBOOKS: &str = "(none)";

/// Fills `template` from `vars`.
///
/// `{{#if key}}body{{/if}}` keeps `body` when `key` maps to a non-empty value
/// and drops it, with its tags, otherwise. `{{key}}` becomes the value, or
/// nothing when there is none: a prompt is read by a model, and a stray
/// `{{playbooks}}` left in it is an instruction of unknown meaning. Neither
/// form nests, and a `{{#if}}` without its `{{/if}}` runs to the end.
pub fn render(template: &str, vars: &HashMap<&str, String>) -> String {
    let conditionals = strip_conditionals(template, vars);
    substitute(&conditionals, vars)
}

fn strip_conditionals(template: &str, vars: &HashMap<&str, String>) -> String {
    const OPEN: &str = "{{#if ";
    const CLOSE: &str = "{{/if}}";

    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + OPEN.len()..];
        let Some(tag_end) = after_open.find("}}") else {
            // An opening tag that never closes is text, not a tag.
            out.push_str(&rest[start..]);
            return out;
        };
        let key = after_open[..tag_end].trim();
        let body_and_rest = &after_open[tag_end + 2..];
        let (body, remainder) = match body_and_rest.find(CLOSE) {
            Some(end) => (&body_and_rest[..end], &body_and_rest[end + CLOSE.len()..]),
            None => (body_and_rest, ""),
        };
        if vars.get(key).is_some_and(|value| !value.is_empty()) {
            out.push_str(body);
        }
        rest = remainder;
    }
    out.push_str(rest);
    out
}

fn substitute(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                if let Some(value) = vars.get(key) {
                    out.push_str(value);
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The plan prompt for one request.
pub fn plan_prompt(project_root: &str, request: &str, playbooks: &[String]) -> String {
    let playbooks = if playbooks.is_empty() {
        NO_PLAYBOOKS.to_owned()
    } else {
        playbooks
            .iter()
            .map(|playbook| format!("- {playbook}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    render(
        PLAN,
        &HashMap::from([
            ("project_root", project_root.to_owned()),
            ("request", request.to_owned()),
            ("playbooks", playbooks),
        ]),
    )
}

/// The execute prompt for an approved plan. `user_edits` is what the person
/// added when they approved with changes (§2.4).
pub fn execute_prompt(project_root: &str, plan: &Plan) -> String {
    execute_prompt_for(
        project_root,
        &plan_as_text(plan),
        plan.user_edits.as_deref(),
    )
}

/// The execute prompt with `{{plan}}` already in words: the rendered plan, or,
/// in direct mode, the request itself (§5).
pub fn execute_prompt_for(project_root: &str, plan: &str, user_edits: Option<&str>) -> String {
    render(
        EXECUTE,
        &HashMap::from([
            ("project_root", project_root.to_owned()),
            ("plan", plan.to_owned()),
            (
                "user_edits",
                user_edits.unwrap_or_default().trim().to_owned(),
            ),
        ]),
    )
}

/// A plan as the engine will be told it back. The structured fields when
/// there are any; the engine's own words when the block was missing and the
/// summary is all that was recovered.
pub fn plan_as_text(plan: &Plan) -> String {
    let mut lines = Vec::new();
    if !plan.summary.is_empty() {
        lines.push(plan.summary.clone());
    }
    if !plan.steps.is_empty() {
        lines.push(String::new());
        lines.push("Steps:".to_owned());
        for (index, step) in plan.steps.iter().enumerate() {
            lines.push(format!("{}. {}", index + 1, step.text));
        }
    }
    let mut list = |heading: &str, items: &[String]| {
        if !items.is_empty() {
            lines.push(String::new());
            lines.push(format!("{heading}:"));
            for item in items {
                lines.push(format!("- {item}"));
            }
        }
    };
    list("Files that will be created or changed", &plan.files_touched);
    list("Leaves this computer", &plan.outbound);
    list("Cannot be undone", &plan.irreversible);
    list("Will not do", &plan.will_not_do);

    if lines.is_empty() {
        return plan.raw_markdown.trim().to_owned();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PlanStep;

    fn vars(pairs: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        pairs
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned()))
            .collect()
    }

    #[test]
    fn keys_are_replaced_and_missing_ones_vanish() {
        assert_eq!(
            render(
                "in {{root}}: {{request}}{{nothing}}",
                &vars(&[("root", "/p"), ("request", "go")])
            ),
            "in /p: go"
        );
    }

    #[test]
    fn an_if_keeps_its_body_only_when_the_key_is_set_and_non_empty() {
        let template = "A{{#if x}} and {{x}}{{/if}}.";
        assert_eq!(render(template, &vars(&[("x", "B")])), "A and B.");
        assert_eq!(render(template, &vars(&[("x", "")])), "A.");
        assert_eq!(render(template, &vars(&[])), "A.");
    }

    #[test]
    fn two_ifs_in_one_template_are_independent() {
        let template = "{{#if a}}a{{/if}}-{{#if b}}b{{/if}}";
        assert_eq!(render(template, &vars(&[("b", "1")])), "-b");
        assert_eq!(render(template, &vars(&[("a", "1"), ("b", "1")])), "a-b");
    }

    /// Prompts are read by a model. A broken tag is better shown than
    /// silently swallowed, and never a panic.
    #[test]
    fn broken_tags_are_left_as_text() {
        assert_eq!(render("{{#if x}}open", &vars(&[("x", "1")])), "open");
        assert_eq!(render("{{#if x}}open", &vars(&[])), "");
        assert_eq!(render("a {{b", &vars(&[("b", "1")])), "a {{b");
        assert_eq!(render("{{#if x", &vars(&[])), "{{#if x");
    }

    #[test]
    fn a_value_containing_braces_is_not_rendered_again() {
        assert_eq!(
            render("{{r}}", &vars(&[("r", "{{root}}"), ("root", "no")])),
            "{{root}}"
        );
    }

    #[test]
    fn the_plan_prompt_carries_the_request_and_names_no_playbooks() {
        let prompt = plan_prompt("/home/me/Month end", "rename FY25 to FY26", &[]);
        assert!(prompt.contains("in the folder /home/me/Month end."));
        assert!(prompt.contains("They asked: \"rename FY25 to FY26\""));
        assert!(prompt.contains(&format!(
            "(follow the matching one if any):\n{NO_PLAYBOOKS}\n"
        )));
        assert!(prompt.contains("info string is eavery-plan"));
        assert!(!prompt.contains("{{"), "an unrendered tag: {prompt}");
    }

    #[test]
    fn the_plan_prompt_lists_playbooks_one_per_line() {
        let prompt = plan_prompt(
            "/p",
            "r",
            &["Month-end close".to_owned(), "Invoice run".to_owned()],
        );
        assert!(prompt.contains("- Month-end close\n- Invoice run\n"));
    }

    #[test]
    fn the_execute_prompt_mentions_the_edits_only_when_there_are_any() {
        let mut plan = Plan {
            summary: "Update the report".into(),
            steps: vec![PlanStep::new("Open it"), PlanStep::new("Change the year")],
            ..Plan::default()
        };
        let prompt = execute_prompt("/p", &plan);
        assert!(prompt.starts_with("The person approved this plan:\nUpdate the report\n\nSteps:\n1. Open it\n2. Change the year\n\n\nCarry out the plan now in the folder /p."));
        assert!(!prompt.contains("They added these changes"));

        plan.user_edits = Some("  skip the cover page  ".into());
        let prompt = execute_prompt("/p", &plan);
        assert!(prompt.contains("They added these changes to the plan: skip the cover page\n"));
        assert!(!prompt.contains("{{"), "an unrendered tag: {prompt}");
    }

    /// Direct mode sends the request where the plan would go (§5).
    #[test]
    fn direct_mode_puts_the_request_where_the_plan_goes() {
        let prompt = execute_prompt_for("/p", "tidy the folder", None);
        assert!(prompt.contains("approved this plan:\ntidy the folder\n"));
    }

    #[test]
    fn a_plan_with_nothing_structured_is_told_back_in_its_own_words() {
        let plan = Plan {
            raw_markdown: "  I would look around first.\n".into(),
            ..Plan::default()
        };
        assert_eq!(plan_as_text(&plan), "I would look around first.");

        let plan = Plan {
            summary: "S".into(),
            outbound: vec!["Email Sam".into()],
            will_not_do: vec!["Delete anything".into()],
            ..Plan::default()
        };
        assert_eq!(
            plan_as_text(&plan),
            "S\n\nLeaves this computer:\n- Email Sam\n\nWill not do:\n- Delete anything"
        );
    }
}
