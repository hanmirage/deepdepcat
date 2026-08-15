//! Automatic procedural-memory capture — the "learn workflows" half.
//!
//! After a turn that changed files and finished successfully, an LLM
//! extracts 0-1 reusable workflows from the conversation and we append it
//! (name-deduplicated, never overwrites) to the project procedures.md.
//! Explicit `procedure_save` stays the manual path; this is the background
//! self-evolution path wired into both the chat and ACP run entry points.

use crate::core::error::AppResult;
use crate::core::types::ConversationItem;
use crate::llm::client::LlmClient;
use crate::llm::provider::{LlmProvider, LlmRequest};
use crate::memory::procedure::{self, Procedure};
use std::path::Path;

/// Marker the LLM returns when nothing is worth persisting.
const NO_PROCEDURE_MARKER: &str = "NO_PROCEDURE";
/// How many recent conversation items feed one extraction pass.
const EXTRACTION_CONTEXT_ITEMS: usize = 30;

/// Parse an LLM draft into a normalized procedure. `mode` is the session's
/// work mode ("code" / "depwork") — the model never chooses it. Returns
/// None when nothing is worth persisting or the draft is malformed.
pub fn parse_procedure_draft(text: &str, mode: &str) -> Option<Procedure> {
    if text.contains(NO_PROCEDURE_MARKER) {
        return None;
    }
    let mut name = String::new();
    let mut trigger = String::new();
    let mut steps: Vec<String> = Vec::new();
    let mut verify: Vec<String> = Vec::new();
    let mut lessons: Vec<String> = Vec::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("PROCEDURE_NAME:") {
            name = v.trim().to_string();
            continue;
        }
        if let Some(v) = line.strip_prefix("TRIGGER:") {
            trigger = v.trim().to_string();
            continue;
        }
        if line.eq_ignore_ascii_case("STEPS:") {
            section = "steps".to_string();
            continue;
        }
        if line.eq_ignore_ascii_case("VERIFY:") {
            section = "verify".to_string();
            continue;
        }
        if line.eq_ignore_ascii_case("LESSONS:") {
            section = "lessons".to_string();
            continue;
        }
        let Some(item) = strip_list_marker(line) else {
            continue;
        };
        if item.is_empty() {
            continue;
        }
        match section.as_str() {
            "steps" => steps.push(item.to_string()),
            "verify" => verify.push(item.to_string()),
            "lessons" => lessons.push(item.to_string()),
            _ => {}
        }
    }
    let procedure = Procedure {
        name,
        mode: mode.to_string(),
        trigger,
        steps,
        verify,
        lessons,
    }
    .normalized();
    if procedure.name.is_empty() || procedure.steps.is_empty() {
        return None;
    }
    Some(procedure)
}

/// Strip `- ` / `* ` / `1. ` / `1) ` list markers (digits only).
fn strip_list_marker(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(rest);
    }
    let mut digits = 0;
    for c in line.chars() {
        if c.is_ascii_digit() {
            digits += 1;
        } else {
            break;
        }
    }
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// Extract 0-1 reusable workflows from a conversation tail. Returns None
/// when nothing qualifies or the extraction call fails.
pub async fn capture_procedure(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    conversation: &[ConversationItem],
    mode: &str,
) -> Option<Procedure> {
    let tail =
        crate::memory::learning::serialize_tail(conversation, EXTRACTION_CONTEXT_ITEMS);
    if tail.trim().is_empty() {
        return None;
    }
    let system_prompt = "You extract ONE reusable workflow (procedure) from an agent \
        session. Only extract when the task had a clear, repeatable, multi-step process \
        AND the agent verified its outcome (tests passed, artifacts checked, output \
        confirmed). Include: ordered concrete steps (3-10), what counts as verified, and \
        non-obvious pitfalls. NEVER include: single trivial edits, one-off session \
        details, vague advice, or processes already dictated by the task brief. If \
        nothing qualifies, reply exactly NO_PROCEDURE. Otherwise output EXACTLY this \
        format:\nPROCEDURE_NAME: short-name\nTRIGGER: comma separated keywords\nSTEPS:\n\
        - step 1\n- step 2\nVERIFY:\n- check\nLESSONS:\n- pitfall\nUse the same language \
        as the session.";
    let request = LlmRequest {
        model: model.to_string(),
        provider: provider.map(str::to_string),
        messages: vec![ConversationItem::user(format!("Session tail:\n\n{tail}"))],
        tools: vec![],
        system_prompt: system_prompt.to_string(),
        temperature: Some(0.2),
        top_p: None,
        // Chinese sessions burn tokens fast in the fixed format
        // (name/trigger/3-10 steps/verify/lessons) — 600 cut real drafts
        // mid-step (observed in the real-API smoke: the last step ended at
        // an unclosed parenthesis). 1200 leaves headroom for the format
        // overhead + a full procedure.
        max_tokens: Some(1200),
        stream: false,
        reasoning_effort: None,
        response_format: None,
        cache_control: None,
        user_id: None,
    };
    let response = llm.complete(&request).await.ok()?;
    parse_procedure_draft(&response.content, mode)
}

/// Steps normalized for near-duplicate comparison — whitespace, punctuation
/// and case are dropped so "运行 cargo test" and "Run `cargo test`" collide.
fn normalized_step(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether the new procedure's steps substantially overlap an existing one —
/// the same workflow captured under a slightly different name. Used to stop
/// automatic capture from accumulating near-duplicate rows.
fn steps_overlap(existing: &Procedure, new: &Procedure) -> bool {
    if existing.steps.is_empty() || new.steps.is_empty() {
        return false;
    }
    let existing_norm: Vec<String> = existing.steps.iter().map(|s| normalized_step(s)).collect();
    let hits = new
        .steps
        .iter()
        .map(|s| normalized_step(s))
        .filter(|ns| {
            !ns.is_empty()
                && existing_norm
                    .iter()
                    .any(|es| es.contains(ns) || ns.contains(es))
        })
        .count();
    hits * 10 >= new.steps.len() * 6
}

/// Append a procedure only when its name is new — automatic capture must
/// never overwrite a workflow the user or agent saved deliberately. A
/// near-duplicate (same steps under a slightly different name) is skipped
/// too, so the 50-entry file is not flooded with the same process captured
/// repeatedly by the LLM.
pub fn save_if_new(path: &Path, procedure: &Procedure) -> AppResult<bool> {
    let existing = procedure::read_procedures(path);
    if existing
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(&procedure.name))
    {
        return Ok(false);
    }
    if existing.iter().any(|p| steps_overlap(p, procedure)) {
        return Ok(false);
    }
    procedure::save_procedure(path, procedure)?;
    Ok(true)
}

/// Full background pass: capture → save into the project procedures.md.
/// Workspace-less sessions are skipped (a global auto-capture would pollute
/// the user layer with project-specific workflows).
pub async fn run_procedure_pass(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    conversation: &[ConversationItem],
    workspace: Option<&Path>,
    mode: &str,
) -> AppResult<bool> {
    let Some(ws) = workspace else {
        tracing::debug!("procedure capture skipped: no workspace");
        return Ok(false);
    };
    let Some(procedure) = capture_procedure(llm, model, provider, conversation, mode).await
    else {
        tracing::debug!("procedure capture skipped: nothing worth persisting");
        return Ok(false);
    };
    let path = procedure::project_procedures_path(ws);
    let saved = save_if_new(&path, &procedure)?;
    tracing::info!(
        procedure = %procedure.name,
        saved,
        path = %path.display(),
        "background procedure capture"
    );
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_draft_with_mode_override() {
        let text = "PROCEDURE_NAME: wechat-article\nTRIGGER: 公众号, 排版\nSTEPS:\n- 收集素材\n1. 写初稿\n* 校对\nVERIFY:\n- 文章可发布\nLESSONS:\n- 图片要压缩\n";
        let p = parse_procedure_draft(text, "depwork").expect("draft");
        assert_eq!(p.name, "wechat-article");
        assert_eq!(p.mode, "depwork");
        assert_eq!(p.steps, vec!["收集素材", "写初稿", "校对"]);
        assert_eq!(p.verify, vec!["文章可发布"]);
        assert_eq!(p.lessons, vec!["图片要压缩"]);
    }

    #[test]
    fn marker_and_malformed_return_none() {
        assert!(parse_procedure_draft("NO_PROCEDURE", "code").is_none());
        assert!(parse_procedure_draft("plain prose without markers", "code").is_none());
        assert!(parse_procedure_draft("PROCEDURE_NAME: x\nSTEPS:\n- a", "code").is_some());
        assert!(parse_procedure_draft("PROCEDURE_NAME: x\nSTEPS:\n- a", "code")
            .is_some());
        assert!(parse_procedure_draft("STEPS:\n- a", "code").is_none());
    }

    #[test]
    fn save_if_new_never_overwrites() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("procedures.md");
        let p = Procedure {
            name: "flow-a".to_string(),
            mode: "code".to_string(),
            trigger: "x".to_string(),
            steps: vec!["s1".to_string()],
            verify: vec![],
            lessons: vec![],
        };
        assert!(save_if_new(&path, &p).expect("first save"));
        assert!(!save_if_new(&path, &p).expect("dup skipped"));
        let mut other = p.clone();
        other.name = "FLOW-A".to_string();
        assert!(!save_if_new(&path, &other).expect("case-insensitive dup"));
        other.name = "flow-b".to_string();
        other.steps = vec!["a genuinely different step".to_string()];
        assert!(save_if_new(&path, &other).expect("new workflow saved"));
        assert_eq!(procedure::read_procedures(&path).len(), 2);
    }

    #[test]
    fn save_if_new_skips_near_duplicate_steps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("procedures.md");
        let original = Procedure {
            name: "deploy".to_string(),
            mode: "code".to_string(),
            trigger: "deploy".to_string(),
            steps: vec![
                "run cargo build".to_string(),
                "run tests".to_string(),
                "push to server".to_string(),
            ],
            verify: vec![],
            lessons: vec![],
        };
        assert!(save_if_new(&path, &original).expect("first save"));

        // Same workflow under a slightly different name, near-identical steps
        // → skipped (no near-duplicate accumulation in the 50-entry file).
        let renamed = Procedure {
            name: "部署流程".to_string(),
            mode: "code".to_string(),
            trigger: "deploy".to_string(),
            steps: vec![
                "run `cargo build`".to_string(),
                "run tests now".to_string(),
                "push to the server".to_string(),
            ],
            verify: vec![],
            lessons: vec![],
        };
        assert!(!save_if_new(&path, &renamed).expect("near-duplicate skipped"));

        // A genuinely different workflow is still saved.
        let different = Procedure {
            name: "release-tag".to_string(),
            mode: "code".to_string(),
            trigger: "release".to_string(),
            steps: vec!["tag the commit".to_string(), "changelog".to_string()],
            verify: vec![],
            lessons: vec![],
        };
        assert!(save_if_new(&path, &different).expect("different workflow saved"));
        assert_eq!(procedure::read_procedures(&path).len(), 2);
    }
}
