//! Skill inventory formatting — renders the available-skill list for the
//! agent's dynamic context.
//!
//! The agent knows skills are activated by path globs but has no way to see
//! *which* skills exist. This module produces a compact name+description
//! inventory (never the skill content) so the model can decide to invoke a
//! skill deliberately. It is mode-filtered, sorted, and capped.

use crate::skills::types::Skill;
use crate::toolkit::WorkMode;

/// Maximum skills listed in the inventory (capped to bound token cost).
pub const MAX_SKILL_INVENTORY: usize = 20;

/// Maximum description length per skill.
const MAX_DESC_CHARS: usize = 200;
/// Total inventory budget (chars). Codex's skill-list rule: the initial
/// skill list uses at most 2% of the context window, or 8000 chars when the
/// window is unknown — DeepDepCat's inventory is built before the model
/// window is resolved, so the 8000-char fallback is the binding budget.
pub const INVENTORY_BUDGET_CHARS: usize = 8_000;
/// Minimum description budget per skill — below this the row is dropped
/// rather than rendered as a useless stub.
const MIN_DESC_CHARS: usize = 40;

/// Render a mode-filtered inventory of available skills.
///
/// Returns `None` when there is nothing worth listing (no enabled skills for
/// this mode, or every description is empty).
pub fn format_skill_inventory(skills: &[Skill], mode: WorkMode) -> Option<String> {
    format_skill_inventory_budgeted(skills, mode, INVENTORY_BUDGET_CHARS)
}

/// Budgeted variant: shortens descriptions first, then drops overflow rows,
/// so the whole rendered inventory stays within `budget_chars`.
pub fn format_skill_inventory_budgeted(
    skills: &[Skill],
    mode: WorkMode,
    budget_chars: usize,
) -> Option<String> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for skill in skills {
        if !skill.enabled {
            continue;
        }
        if !skill.work_modes.is_empty()
            && !skill
                .work_modes
                .iter()
                .any(|m| m.eq_ignore_ascii_case(mode.as_str()))
        {
            continue;
        }
        let desc = skill.description.trim();
        if desc.is_empty() {
            continue;
        }
        rows.push((skill.name.clone(), desc.to_string()));
    }

    if rows.is_empty() {
        return None;
    }

    rows.sort_by_key(|a| a.0.to_ascii_lowercase());
    rows.truncate(MAX_SKILL_INVENTORY);

    // ── Budget allocation: shorten first, drop last ──────────────
    let header = "## Available Skills\n\n";
    let budget = budget_chars.max(header.len() + 80);
    let body_budget = budget - header.len();

    // Per-skill description cap: share the body budget evenly, bounded by
    // the hard per-entry cap and the minimum useful length.
    let mut per_desc = (body_budget / rows.len().max(1)).min(MAX_DESC_CHARS);
    per_desc = per_desc.max(MIN_DESC_CHARS);

    let mut rendered: Vec<String> = Vec::new();
    for (name, desc) in &rows {
        let truncated: String = desc.chars().take(per_desc).collect();
        rendered.push(format!("- **{name}** — {truncated}\n"));
    }

    // Drop overflow rows from the end until the whole block fits (a
    // hard guarantee the "shorten first" pass cannot give).
    while rendered.len() > 1 && rendered.iter().map(String::len).sum::<usize>() > body_budget {
        rendered.pop();
    }
    if rendered.is_empty() {
        return None;
    }

    let mut out = String::from(header);
    out.extend(rendered);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, desc: &str, modes: &[&str]) -> Skill {
        Skill {
            id: name.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
            content: "".to_string(),
            model: None,
            allowed_tools: vec![],
            permission_mode: None,
            paths: vec![],
            work_modes: modes.iter().map(|m| m.to_string()).collect(),
            when_to_use: vec![],
            source: crate::skills::types::SkillSource::Bundled,
            file_path: None,
            enabled: true,
        }
    }

    #[test]
    fn renders_inventory() {
        let skills = vec![
            skill("Review", "Review code for bugs", &[]),
            skill("Plan", "Plan a task", &[]),
        ];
        let out = format_skill_inventory(&skills, WorkMode::Code).unwrap();
        assert!(out.contains("## Available Skills"));
        assert!(out.contains("**Review**"));
        assert!(out.contains("**Plan**"));
    }

    #[test]
    fn filters_by_work_mode() {
        let skills = vec![skill("DepSkill", "Only depwork", &["depwork"])];
        assert!(format_skill_inventory(&skills, WorkMode::Code).is_none());
        assert!(format_skill_inventory(&skills, WorkMode::Depwork).is_some());
    }

    #[test]
    fn skips_disabled_and_empty_desc() {
        let mut disabled = skill("Off", "desc", &[]);
        disabled.enabled = false;
        let no_desc = skill("NoDesc", "", &[]);
        let skills = vec![disabled, no_desc];
        assert!(format_skill_inventory(&skills, WorkMode::Code).is_none());
    }

    #[test]
    fn caps_description_length() {
        let long = "x".repeat(500);
        let skills = vec![skill("Long", &long, &[])];
        let out = format_skill_inventory(&skills, WorkMode::Code).unwrap();
        assert!(!out.contains(&"x".repeat(201)));
    }

    #[test]
    fn caps_inventory_size() {
        let mut skills = Vec::new();
        for i in 0..50 {
            skills.push(skill(&format!("S{i:02}"), "desc", &[]));
        }
        let out = format_skill_inventory(&skills, WorkMode::Code).unwrap();
        let count = out.matches("**S").count();
        assert!(count <= MAX_SKILL_INVENTORY);
    }

    #[test]
    fn budget_shortens_descriptions_before_dropping() {
        let long_desc = "x".repeat(600);
        let skills = vec![skill("A", &long_desc, &[]), skill("B", &long_desc, &[])];
        let tight = format_skill_inventory_budgeted(&skills, WorkMode::Code, 500).unwrap();
        assert!(
            tight.len() <= 500,
            "inventory must fit the budget, got {}",
            tight.len()
        );
        assert!(
            tight.contains("**A**") && tight.contains("**B**"),
            "both rows survive via shortened descriptions"
        );
    }

    #[test]
    fn budget_drops_overflow_rows_as_last_resort() {
        let skills: Vec<Skill> = (0..50)
            .map(|i| skill(&format!("S{i:02}"), "a fairly long description here", &[]))
            .collect();
        let tiny = format_skill_inventory_budgeted(&skills, WorkMode::Code, 300).unwrap();
        assert!(
            tiny.len() <= 300,
            "hard budget must hold even when descriptions are minimal"
        );
        assert!(
            tiny.contains("**S00**"),
            "the first (sorted) skill must survive"
        );
    }
}
