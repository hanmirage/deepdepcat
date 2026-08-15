//! Gate helpers — the loop's text-signal detectors (narration, explicit
//! completion) and their tuning constants. Extracted from `run.rs` so the
//! main loop file stays navigable and the detectors stay unit-testable
//! without touching the loop.

/// Exploration budget (#84): consecutive all-read-only tool turns before
/// a convergence nudge fires. The diagnosed failure session ran 10+
/// list_dir rounds without converging — 6 is the point where patience
/// turns into enabling.
pub(super) const EXPLORATION_ROUND_LIMIT: u32 = 6;
/// Maximum exploration nudges per run — after two, the loop must break
/// the cycle by budget enforcement instead of more reminders.
pub(super) const MAX_EXPLORATION_NUDGES: u32 = 2;
/// Per-tool-name consecutive failure threshold (#84) — beyond this the
/// approach itself is doomed, not the arguments (mvn → javac → java all
/// missing from PATH is one failed approach).
pub(super) const TOOL_NAME_FAILURE_LIMIT: u32 = 3;
/// Minimum similarity (0..1) between two narration turns before they count
/// as repeated. Char-bigram Jaccard — generous enough to catch
/// paraphrased restatement ("改为自己直接审查" vs "改为直接逐对审查").
pub(super) const REPETITION_SIMILARITY: f64 = 0.65;
/// Consecutive near-identical narration turns before the repetition guard
/// fires. Two = one nudge, never a knee-jerk on a single restatement.
pub(super) const REPETITION_TURNS: u32 = 2;

/// Small-change scope guard — injected (once per turn) when the intent is a
/// light single-purpose edit. Counteracts overreach: a one-file restyle
/// must not escalate into web fetches, downloads into the user's workspace,
/// or a multi-round extraction pipeline. The escape hatch keeps it safe if
/// the task is misjudged as small.
pub(super) const LIGHT_TASK_GUIDANCE: &str =
    "Small-change scope guard: this request is a SINGLE small \
    edit. Change exactly what was asked and stop — do NOT download reference material, do NOT \
    fetch the web, and do NOT start a research pipeline. Read the target file(s), make the \
    minimal edit (verify with tests/lint if it involves code), then summarize. If the task \
    turns out bigger than it looks, work through it step by step and track progress with \
    `todo_write` instead.";

/// Whether an edited file is a NON-CODE document — plain text / markup /
/// data files that have no meaningful syntax-check or test command.
///
/// The verification gate and the Tier-3 independent evaluator only make
/// sense for code: demanding `tsc --noEmit` after writing a `.txt` report
/// forces the model into an endless "verify → re-summarize → verify" loop
/// (the 2026-08-07 "a txt file never ends" session: 5+ repeated completion
/// summaries). Document edits are verified by existence + content checks,
/// which the model does with read tools — no command ceremony required.
pub(super) fn is_non_code_document(path: &std::path::Path) -> bool {
    const DOC_EXTENSIONS: &[&str] = &[
        "txt", "md", "markdown", "rst", "csv", "tsv", "json", "jsonl", "toml", "yaml", "yml",
        "ini", "cfg", "conf", "env", "log", "html", "htm", "css", "scss", "less", "xml", "svg",
        "png", "jpg", "jpeg", "gif", "webp", "ico", "pdf", "docx", "xlsx", "pptx",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| DOC_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Whether EVERY edited file of this run is a non-code document.
/// Empty edit sets return `false` (the gate keeps its normal behavior).
pub(super) fn edited_only_documents(files: &[std::path::PathBuf]) -> bool {
    !files.is_empty() && files.iter().all(|p| is_non_code_document(p))
}

/// Distinct lowercase extensions of the edited CODE files (non-documents,
/// dotfiles excluded — `.env` etc. carry no extension in Rust's view and
/// would otherwise be misread as code). Used to infer the project's
/// typecheck command for the code-verify nudge.
pub(super) fn code_file_extensions(files: &[std::path::PathBuf]) -> Vec<String> {
    let mut exts = Vec::new();
    for p in files {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with('.') || is_non_code_document(p) {
            continue;
        }
        if let Some(e) = p.extension().and_then(|e| e.to_str()) {
            let e = e.to_ascii_lowercase();
            if !exts.contains(&e) {
                exts.push(e);
            }
        }
    }
    exts
}

/// A concrete typecheck/build command to suggest for the given code-file
/// extensions, or `None` when the project's command is not confidently known
/// (the nudge then names candidates generically instead of risking a wrong
/// command that wastes a round).
pub(super) fn suggest_check_command(exts: &[String]) -> Option<&'static str> {
    if exts.iter().any(|e| e == "rs") {
        Some("cargo check")
    } else if exts.iter().any(|e| e == "go") {
        Some("go build ./...")
    } else if exts.iter().any(|e| e == "py") {
        Some("python -m py_compile")
    } else if exts.iter().any(|e| e == "ts") {
        Some("tsc --noEmit")
    } else {
        None
    }
}

/// Whether `path` is inside `workspace` — or `true` when there is no workspace
/// (then every path counts). Keeps scratch files the agent writes OUTSIDE the
/// workspace (a temp `apply_opt.ps1` helper, `frag_*.html` in %TEMP%) from
/// re-arming the code-verify gate for an otherwise document-only task (an
/// HTML site whose only "code" was a temp helper script, which forced a
/// pointless lsp call).
pub(super) fn is_in_workspace(workspace: Option<&std::path::Path>, path: &std::path::Path) -> bool {
    match workspace {
        Some(ws) => path.starts_with(ws),
        None => true,
    }
}

/// Char-bigram Jaccard similarity between two texts (0..1). Empty texts
/// score 0 (never "similar" to anything).
pub(super) fn narration_similarity(a: &str, b: &str) -> f64 {
    fn bigrams(s: &str) -> std::collections::HashSet<(char, char)> {
        let chars: Vec<char> = s.chars().collect();
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let sa = bigrams(a);
    let sb = bigrams(b);
    let union: std::collections::HashSet<(char, char)> = sa.union(&sb).copied().collect();
    if union.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count();
    inter as f64 / union.len() as f64
}

/// Heuristic narration detector — does this assistant text read like a
/// progress report about an action that was NOT actually taken ("I'm going
/// to fix...", "让我先检查...") rather than a direct answer?
///
/// This must only match IN-PROGRESS / INTENT phrasing. Completed-work
/// summaries ("I've fixed...", "已完成...") are legitimate final answers
/// after real tool activity — flagging them as narration forces the model
/// to re-generate its summary every turn (the "summarizes twice/thrice"
/// bug). Paired with "recent tool activity in the transcript" before it can
/// fire, so conversational replies are never mistaken for stalled narration.
pub(super) fn is_narration_without_action(text: &str) -> bool {
    const EN: &[&str] = &[
        "i'm going to ",
        "i am going to ",
        "about to ",
        "let me check",
        "let me look",
        "let me run",
        "i'll start",
        "i'm starting",
        "i will start",
        "starting to ",
        "attempting to ",
        "trying to ",
    ];
    const ZH: &[&str] = &[
        "让我先",
        "让我看看",
        "让我检查",
        "正在执行",
        "开始处理",
        "进行中",
        "准备开始",
        "我来检查",
    ];
    let lower = text.to_lowercase();
    EN.iter().any(|m| lower.contains(m)) || ZH.iter().any(|m| text.contains(m))
}

/// Whether the model's text is an explicit completion statement — a final
/// summary declaring the work done with nothing left ("测试任务已完成，无剩余
/// 步骤"). Stop-time gates (TodoGate) must respect it: the model already
/// answered the completion question in the affirmative, and nudging it again
/// only forces duplicate completion summaries (the "summarizes thrice" bug).
pub(super) fn is_explicit_completion(text: &str) -> bool {
    const DONE_MARKERS: &[&str] = &[
        "已完成",
        "任务完成",
        "全部完成",
        "已全部清理",
        "全部清理",
        "已清理完毕",
        "清理完毕",
        "无剩余",
        "没有剩余",
        "没有遗留",
        "无遗留",
        "无需新建",
        "无需创建",
        "不需要新建",
        "已结束",
        "全部搞定",
        "搞定了",
        "做完了",
        "弄完了",
        "大功告成",
        "收工",
        "完工",
        "done",
        "finished",
        "complete",
        "all cleaned",
        "all done",
    ];
    // Continuation signals veto the completion reading: "已完成第一步，接下来
    // 继续处理第二步" is progress, not a final summary. ("然后"/"接着" are
    // deliberately absent — they are neutral sequence connectors that
    // over-vetoed legitimate completions like "全部完成，然后提交了代码".)
    const CONTINUE_MARKERS: &[&str] = &[
        "继续",
        "接下来",
        "下一步",
        "还需要",
        "还有",
        "尚未",
        "还没",
        "未完成",
        "后续",
        "next",
        "continue",
        "remaining",
        "still",
    ];
    // The bare English markers "done"/"finished"/"complete" substring-match
    // their own negations ("undone", "incomplete", "not done", …). Those must
    // veto a completion reading — the model is stating the work is INCOMPLETE,
    // and treating it as done would hard-stop the turn over unfinished work.
    const NEGATION_MARKERS: &[&str] = &[
        "undone",
        "unfinished",
        "incomplete",
        "not done",
        "not finished",
        "not complete",
        "not yet",
    ];
    let lower = text.to_lowercase();
    DONE_MARKERS.iter().any(|m| lower.contains(m))
        && !CONTINUE_MARKERS.iter().any(|m| lower.contains(m))
        && !NEGATION_MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_completion_recognizes_final_summaries() {
        assert!(is_explicit_completion(
            "测试用的临时文件已全部清理，你的工作区保持为空。"
        ));
        assert!(is_explicit_completion(
            "工具测试任务已完成，无需新建待办清单。确认无剩余步骤，工作区保持干净。"
        ));
        assert!(is_explicit_completion(
            "The test files are all cleaned up and the workspace is empty."
        ));
    }

    #[test]
    fn explicit_completion_rejects_progress_statements() {
        assert!(!is_explicit_completion(
            "已完成第一步，接下来继续处理第二步。"
        ));
        assert!(!is_explicit_completion(
            "任务尚未完成，还有几个文件需要检查。"
        ));
        assert!(!is_explicit_completion(
            "让我先看看代码结构，然后开始实现。"
        ));
    }

    #[test]
    fn negated_english_markers_are_not_completion() {
        // Bare "done"/"finished"/"complete" substring-match their own
        // negations — a model stating work is INCOMPLETE must never trip the
        // hard-stop brake (audit: gates-done-negation-false-positive).
        assert!(!is_explicit_completion("The migration is not done yet."));
        assert!(!is_explicit_completion("This is incomplete."));
        assert!(!is_explicit_completion("The refactor is unfinished."));
        assert!(!is_explicit_completion("Two tasks remain undone."));
        assert!(!is_explicit_completion("Not finished — still one test failing."));
        assert!(!is_explicit_completion("The work is not complete."));
    }

    #[test]
    fn neutral_connector_does_not_veto_completion() {
        // "然后"/"接着" are neutral sequence connectors, not progress
        // signals — a past-tense completion followed by one must still
        // read as done (audit: gates-done-connector-overveto).
        assert!(is_explicit_completion("全部完成，然后提交了代码。"));
        assert!(is_explicit_completion("做完了，接着清理了临时文件。"));
        // A genuine progress statement is still vetoed by the real signal.
        assert!(!is_explicit_completion("已完成第一步，然后继续处理第二步。"));
    }

    #[test]
    fn explicit_completion_covers_casual_zh_and_en_closers() {
        // The gate's whole job is to NOT drag a finished turn back for a
        // duplicate summary — casual closers must be recognized too.
        assert!(is_explicit_completion("全部搞定，没有剩余步骤。"));
        assert!(is_explicit_completion("做完了，工作区保持干净。"));
        assert!(is_explicit_completion("弄完了，测试全部通过。"));
        assert!(is_explicit_completion("大功告成。"));
        assert!(is_explicit_completion("收工。"));
        assert!(is_explicit_completion("All done."));
    }

    #[test]
    fn narration_detector_ignores_completed_work_summaries() {
        // Completed-work summaries are legitimate final answers — they must
        // never be flagged as narration (the "summarizes twice/thrice" bug:
        // flagging them forced a re-generation every turn).
        assert!(!is_narration_without_action("已完成，修复了崩溃问题。"));
        assert!(!is_narration_without_action(
            "已经修复了三个 bug，全部测试通过。"
        ));
        assert!(!is_narration_without_action(
            "I've fixed the crash and all tests pass."
        ));
        assert!(!is_narration_without_action(
            "I have implemented the feature."
        ));
        assert!(!is_narration_without_action("刚刚执行了测试，全部通过。"));
    }

    #[test]
    fn narration_detector_still_catches_in_progress_intent() {
        // In-progress / intent phrasing is the actual narration signal — a
        // claimed action with no tool call behind it.
        assert!(is_narration_without_action("让我先检查一下目录结构"));
        assert!(is_narration_without_action("我正在执行测试，稍等"));
        assert!(is_narration_without_action(
            "Let me check the current directory."
        ));
        assert!(is_narration_without_action("I'm going to fix the bug now."));
    }

    #[test]
    fn narration_similarity_scores_identical_texts_high() {
        assert_eq!(
            narration_similarity("同一个句子", "同一个句子"),
            1.0,
            "identical texts score 1.0"
        );
    }

    #[test]
    fn narration_similarity_paraphrase_reaches_threshold() {
        // The diagnosed session restated "环境无 Java/Maven、子代理失控,
        // 我改为自己直接审查" 5 times with light variations. Paraphrase
        // sharing most bigrams must clear the 0.65 bar.
        let a = "环境无 Java/Maven、子代理失控,我改为自己直接审查双实现文件";
        let b = "环境无 Java/Maven 且子代理失控,我改为自己直接审查逐对双实现";
        assert!(
            narration_similarity(a, b) >= REPETITION_SIMILARITY,
            "paraphrase similarity {} must clear the repetition bar",
            narration_similarity(a, b)
        );
    }

    #[test]
    fn narration_similarity_distinct_texts_score_low() {
        assert!(
            narration_similarity("修复目录下的 bug 并验证测试", "今天天气很好适合出去散步") < 0.3,
            "unrelated texts must score low"
        );
        assert_eq!(narration_similarity("", "任何文本"), 0.0);
        assert_eq!(narration_similarity("", ""), 0.0);
    }

    #[test]
    fn document_classification_separates_code_from_docs() {
        use std::path::Path;
        assert!(is_non_code_document(Path::new("report.txt")));
        assert!(is_non_code_document(Path::new("README.md")));
        assert!(is_non_code_document(Path::new("data/content.json")));
        assert!(is_non_code_document(Path::new("site/index.html")));
        assert!(!is_non_code_document(Path::new("src/main.rs")));
        assert!(!is_non_code_document(Path::new("src/app.ts")));
        assert!(!is_non_code_document(Path::new("script.py")));
        assert!(!is_non_code_document(Path::new("Makefile")));
    }

    #[test]
    fn document_only_edit_sets_skip_code_gates() {
        use std::path::PathBuf;
        assert!(edited_only_documents(&[
            PathBuf::from("a.txt"),
            PathBuf::from("b.md")
        ]));
        assert!(!edited_only_documents(&[
            PathBuf::from("a.txt"),
            PathBuf::from("b.rs")
        ]));
        assert!(!edited_only_documents(&[PathBuf::from("b.rs")]));
        // Empty edit sets keep the gates' normal behavior.
        assert!(!edited_only_documents(&[]));
    }

    #[test]
    fn code_file_extensions_picks_code_and_skips_documents_dotfiles() {
        use std::path::PathBuf;
        let files = vec![
            PathBuf::from("src/main.rs"),
            PathBuf::from("lib.ts"),
            PathBuf::from("README.md"),
            PathBuf::from(".env"),
            PathBuf::from("config.json"),
        ];
        let exts = code_file_extensions(&files);
        assert!(exts.contains(&"rs".to_string()));
        assert!(exts.contains(&"ts".to_string()));
        assert!(!exts.contains(&"md".to_string()), "documents excluded");
        assert!(!exts.contains(&"json".to_string()), "documents excluded");
        assert!(!exts.is_empty());
        // Dotfiles carry no extension in Rust's view — excluded as config.
        assert!(!exts.contains(&"env".to_string()));
        assert_eq!(exts.len(), 2);
    }

    #[test]
    fn suggest_check_command_maps_confident_extensions() {
        assert_eq!(suggest_check_command(&["rs".to_string()]), Some("cargo check"));
        assert_eq!(suggest_check_command(&["go".to_string()]), Some("go build ./..."));
        assert_eq!(
            suggest_check_command(&["py".to_string()]),
            Some("python -m py_compile")
        );
        assert_eq!(
            suggest_check_command(&["ts".to_string()]),
            Some("tsc --noEmit")
        );
        // Unknown / mixed-with-unknown → generic (None).
        assert_eq!(suggest_check_command(&["js".to_string()]), None);
        assert_eq!(suggest_check_command(&["ps1".to_string()]), None);
        assert_eq!(suggest_check_command(&[]), None);
    }

    #[test]
    fn workspace_membership_filters_scratch_files() {
        use std::path::Path;
        let ws = Path::new(r"D:\测试");
        assert!(is_in_workspace(Some(ws), Path::new(r"D:\测试\index.html")));
        assert!(!is_in_workspace(
            Some(ws),
            Path::new(r"C:\Users\hanzi\AppData\Local\Temp\apply_opt.ps1")
        ));
        // Component boundary: a sibling dir sharing a prefix is NOT inside.
        assert!(!is_in_workspace(Some(ws), Path::new(r"D:\测试2\index.html")));
        // No workspace → everything counts (no filtering).
        assert!(is_in_workspace(None, Path::new(r"C:\temp\apply_opt.ps1")));
    }
}
