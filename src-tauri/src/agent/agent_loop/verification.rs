//! Verification classification — which bash commands count as verification,
//! how executed tool results map to gate outcomes, and the failure
//! guidance fed back to the model. Extracted from `run.rs` so the main
//! loop file stays navigable and the gate semantics stay unit-testable.

use crate::agent::chat_state::ChatState;

/// Verification evidence strength — weakest to strongest:
/// `None < Syntax < Tests`.
///
/// - `Syntax` — static/syntax evidence (type-check, lint, diagnostics):
///   proves the edit is well-formed, not that it works.
/// - `Tests` — real execution evidence (tests, builds, verification
///   pipelines): proves the change actually runs and passes.
///
/// The acceptance gate treats `Tests` as the bar that skips the independent
/// evaluator; `Syntax` alone still gets the independent review (Tier 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum VerificationTier {
    None,
    Syntax,
    Tests,
}

impl VerificationTier {
    /// Whether this level already covers `tier` — `Tests` is at least
    /// `Syntax`, so a later static check cannot downgrade accepted evidence.
    pub(super) fn is_at_least(self, tier: VerificationTier) -> bool {
        self >= tier
    }
}

/// Classify a bash command's verification tier, or `None` when it is not a
/// verification command at all.
///
/// Token-based on purpose: the previous substring match misclassified
/// innocent commands (`cat test.txt`, `grep verify src/main.rs`) as
/// verification, letting the gate pass without real evidence. A command
/// counts only when its executable + action match a known verifier.
///
/// A `&&` chain only succeeds when EVERY segment succeeds, so the strongest
/// tier across segments is the evidence earned (`npm test && npm run lint`
/// proves Tests, not just the trailing lint). Each segment is reduced to its
/// last `;`-command, whose exit code is the one the shell reports.
pub(super) fn verification_command_tier(command: &str) -> Option<VerificationTier> {
    let mut best: Option<VerificationTier> = None;
    for segment in command.split("&&") {
        let last = segment.split(';').next_back().unwrap_or(segment);
        if let Some(tier) = verification_segment_tier(last) {
            best = Some(best.map_or(tier, |b| b.max(tier)));
        }
    }
    best
}

/// Classify a single `&&`-segment (already reduced to its last `;`-command).
fn verification_segment_tier(segment: &str) -> Option<VerificationTier> {
    let lower = segment.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let &first = tokens.first()?;
    let first = normalize_verification_executable(first);
    let action = tokens.get(1).copied().unwrap_or("");
    let script = if matches!(action, "run" | "exec") {
        tokens.get(2).copied().unwrap_or("")
    } else {
        action
    };

    let tier = match first.as_str() {
        // Test runners — the intent is unambiguous.
        "pytest" | "vitest" | "jest" | "phpunit" => VerificationTier::Tests,
        // Static checkers — syntax/type/lint only.
        "tsc" | "eslint" | "ruff" | "mypy" | "flake8" | "golangci-lint" => VerificationTier::Syntax,
        // Package runners whose TARGET decides (`npx tsc --noEmit`).
        "npx" | "bunx" | "yarn" | "pnpm" => {
            if matches!(script, "vitest" | "jest") {
                VerificationTier::Tests
            } else if matches!(script, "tsc" | "eslint" | "ruff" | "mypy" | "clippy") {
                VerificationTier::Syntax
            } else {
                verification_script_tier(script)?
            }
        }
        // Toolchains whose subcommand decides.
        "cargo" => match action {
            "test" | "build" => VerificationTier::Tests,
            "check" | "clippy" => VerificationTier::Syntax,
            _ => return None,
        },
        "go" => match action {
            "test" | "build" => VerificationTier::Tests,
            "vet" => VerificationTier::Syntax,
            _ => return None,
        },
        "deno" => match action {
            "test" => VerificationTier::Tests,
            "check" => VerificationTier::Syntax,
            _ => return None,
        },
        // `node --test` runs tests; `node --check <file>` is the syntax
        // check the agent routinely runs after JS edits.
        "node" => match action {
            "--test" => VerificationTier::Tests,
            "--check" => VerificationTier::Syntax,
            _ => return None,
        },
        "make" => match action {
            "test" | "build" | "verify" => VerificationTier::Tests,
            "check" | "lint" => VerificationTier::Syntax,
            _ => return None,
        },
        "gradle" | "gradlew" | "./gradlew" | "mvn" | "mvnw" | "./mvnw" => match action {
            "test" | "check" | "build" | "verify" => VerificationTier::Tests,
            // `lint` is static — it must not skip the independent review.
            "lint" => VerificationTier::Syntax,
            _ => return None,
        },
        "dotnet" => match action {
            "test" | "build" => VerificationTier::Tests,
            _ => return None,
        },
        "flutter" => match action {
            "test" | "build" => VerificationTier::Tests,
            "analyze" | "check" => VerificationTier::Syntax,
            _ => return None,
        },
        "dart" => match action {
            "test" => VerificationTier::Tests,
            "analyze" => VerificationTier::Syntax,
            _ => return None,
        },
        "swift" => match action {
            "test" | "build" => VerificationTier::Tests,
            _ => return None,
        },
        // Bun's own runner: `bun test`, `bun run test`, `bun run check`…
        "bun" => match action {
            "test" | "build" => VerificationTier::Tests,
            "lint" | "typecheck" | "check" => VerificationTier::Syntax,
            "run" => verification_script_tier(script)?,
            _ => return None,
        },
        "python" | "python3" | "py" => {
            if action == "-m" {
                match tokens.get(2).copied() {
                    Some("pytest" | "unittest") => VerificationTier::Tests,
                    Some("mypy" | "ruff" | "flake8" | "py_compile") => VerificationTier::Syntax,
                    _ => return None,
                }
            } else {
                match action {
                    "pytest" | "unittest" => VerificationTier::Tests,
                    "mypy" | "ruff" | "flake8" | "py_compile" => VerificationTier::Syntax,
                    _ => return None,
                }
            }
        }
        "npm" => verification_script_tier(script)?,
        _ => return None,
    };
    Some(tier)
}

/// Whether a bash command counts as a verification step at all (any tier).
#[cfg(test)]
pub(super) fn is_verification_command(command: &str) -> bool {
    verification_command_tier(command).is_some()
}

/// Normalize a command's executable token for verification classification:
/// trim quotes, take the basename (`./node_modules/.bin/tsc.cmd` →
/// `tsc.cmd`), lowercase, and strip Windows executable suffixes
/// (`.exe`/`.cmd`/`.bat`/`.ps1`). The model on Windows freely alternates
/// `cargo test`, `cargo.exe test`, `npx.cmd tsc --noEmit` — without this,
/// the verification gate misses real test runs and keeps nagging.
fn normalize_verification_executable(first: &str) -> String {
    let first = first.trim_matches('"').trim_matches('\'');
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let lower = base.to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stripped) = lower.strip_suffix(suffix) {
            if !stripped.is_empty() {
                return stripped.to_string();
            }
        }
    }
    lower
}

/// Tier for a package-manager script name (`npm test`, `npm run build`…).
/// The name sits in the explicit action position, so prefix matching here
/// cannot collide with file names.
fn verification_script_tier(script: &str) -> Option<VerificationTier> {
    match script {
        "test" | "build" | "verify" => Some(VerificationTier::Tests),
        "lint" | "typecheck" | "check" => Some(VerificationTier::Syntax),
        _ if script.starts_with("test:") => Some(VerificationTier::Tests),
        _ if script.starts_with("lint:") => Some(VerificationTier::Syntax),
        _ => None,
    }
}

/// Whether a `finish_reason=insufficient_system_resource` request should be
/// retried: only when the backend interrupted BEFORE any output (partial text
/// or tool calls are usable — a retry would duplicate the billed work), and
/// within the retry budget.
pub(super) fn should_retry_insufficient_resource(
    finish_reason: &str,
    has_output: bool,
    retries: u32,
) -> bool {
    finish_reason == "insufficient_system_resource" && !has_output && retries < 3
}

/// Classify a verification tool call against its executed result into the
/// verification state transition for the gate:
/// - `Pass(tier)` — a verification step that actually succeeded (is_error
///   false), carrying the evidence tier (Syntax/Tests) it earned
/// - `Fail` — a verification step that ran but returned an error
/// - `NoResult` — a verification step whose result is missing (blocked,
///   denied, or never executed) — not verified
/// - `NotVerification` — not a verification step (no state change)
///
/// `is_error` is the dispatcher's authoritative success/failure flag. For
/// `lsp` diagnostics the server reports errors as a SUCCESS result with the
/// error text in the content — so the content is inspected for error
/// diagnostics too: a diagnostics run that found errors must count as a
/// failed verification, not "verified".
///
/// `lsp_operation` is the lsp tool's `operation` argument. Only `diagnostics`
/// checks the edited code for errors — the other operations (hover, symbols,
/// definition, references, format, workspace_symbols) return lookup/format
/// content and must NOT count as verification (otherwise the model could
/// defeat the gate with a no-op LSP query).
pub(super) fn verification_outcome(
    tool_name: &str,
    command: Option<&str>,
    lsp_operation: Option<&str>,
    executed: Option<(bool, String)>,
) -> VerificationOutcome {
    match tool_name {
        "bash" => {
            if let Some(cmd) = command {
                if let Some(tier) = verification_command_tier(cmd) {
                    match executed {
                        Some((true, _)) => VerificationOutcome::Fail,
                        Some((false, _)) => VerificationOutcome::Pass(tier),
                        // The command never ran (permission denied / hook
                        // blocked) — not verified.
                        None => VerificationOutcome::NoResult,
                    }
                } else {
                    VerificationOutcome::NotVerification
                }
            } else {
                VerificationOutcome::NotVerification
            }
        }
        "lsp" => {
            if lsp_operation != Some("diagnostics") {
                return VerificationOutcome::NotVerification;
            }
            match executed {
                Some((true, _)) => VerificationOutcome::Fail,
                // Diagnostics can succeed while reporting errors — inspect the
                // content for error-severity diagnostics.
                Some((false, content)) => {
                    if content.trim().is_empty() {
                        // No result content (anomalous — the lsp tool reports
                        // "No diagnostics." when clean) — not verified.
                        VerificationOutcome::NoResult
                    } else if lsp_content_has_errors(&content) {
                        VerificationOutcome::Fail
                    } else {
                        // Diagnostics are static evidence — Tier 1 (Syntax).
                        VerificationOutcome::Pass(VerificationTier::Syntax)
                    }
                }
                // lsp ran but produced no result content — not verified.
                None => VerificationOutcome::NoResult,
            }
        }
        _ => VerificationOutcome::NotVerification,
    }
}

/// Whether an `lsp` diagnostics result contains error-severity diagnostics.
///
/// The lsp tool formats diagnostics as `[severity] file:line: message` per
/// line (`mod.rs`). Any line carrying `[error]` (case-insensitive) means the
/// file does not type-check — the verification must count as failed.
pub(super) fn lsp_content_has_errors(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.to_ascii_lowercase().contains("[error]"))
}

/// Build structured debugging guidance for a FAILED verification.
///
/// Pairs the edited-file list with the ACTUAL failing output (the last
/// failed tool result in the conversation — non-zero exit or error
/// diagnostics) so the model debugs with evidence instead of guessing, and
/// prescribes the systematic debug loop: reproduce → isolate → fix →
/// re-verify. This is the difference between "fix it" (guess) and
/// "here is what broke and how to pin it down" (systematic debugging).
pub(super) fn build_verification_failure_guidance(
    files: &[String],
    chat_state: &ChatState,
) -> String {
    let excerpt = chat_state
        .conversation
        .iter()
        .rev()
        .find_map(|item| match item {
            crate::core::types::ConversationItem::ToolResult(tr) => {
                let failed = tr.is_error
                    || tr
                        .content
                        .lines()
                        .any(|l| l.contains("[error]") || l.contains("[Error]"));
                failed.then(|| tr.content.chars().take(600).collect::<String>())
            }
            _ => None,
        });

    let mut out = format!(
        "A verification command you ran returned a non-zero exit code (or diagnostics \
         reported errors), so your changes to {} are NOT verified. Fix the failure, \
         re-run the verification, and confirm it passes before you conclude.\n\n\
         <debug_workflow>\nDebug systematically, not by guesswork:\n\
         1. REPRODUCE — re-run the failing command and read the full failure output.\n\
         2. ISOLATE — trace the failure to the exact file:line; check what your edit \
         actually produced (re-read the file if needed).\n\
         3. FIX — make the smallest change that addresses the root cause, not the \
         symptom.\n\
         4. RE-VERIFY — re-run the same verification until it passes.\n\
         </debug_workflow>",
        files.join(", ")
    );
    if let Some(excerpt) = excerpt {
        out.push_str(&format!("\n\n<failed_output>\n{excerpt}\n</failed_output>"));
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum VerificationOutcome {
    Pass(VerificationTier),
    Fail,
    NoResult,
    NotVerification,
}

/// Apply a verification outcome to the run's gate state. The evidence tier
/// only ever moves UP (None → Syntax → Tests): a successful test run
/// followed by a static check keeps `Tests`, never downgrades.
///
/// `NoResult` (blocked/denied/never ran) is NOT evidence either way — it
/// must not reset an already-passed verification: a successful `cargo test`
/// followed by a denied verification attempt must keep `verification_done`
/// true, or the gate re-fires on evidence it already accepted.
pub(super) fn apply_verification_outcome(
    outcome: VerificationOutcome,
    verification_tier: &mut VerificationTier,
    verification_failed: &mut bool,
) {
    match outcome {
        VerificationOutcome::Pass(tier) => {
            if tier > *verification_tier {
                *verification_tier = tier;
            }
            // A later Tests-tier pass resolves an earlier failure — the fix
            // round is now verified, so a stale "failed" flag must not keep
            // the completion brake from firing or force a redundant evaluator
            // review of already-passing work (audit:
            // sticky-failed-false-positive). Syntax alone does NOT clear it:
            // `tsc` passing after a failed `cargo test` leaves the test
            // failure unresolved.
            if tier >= VerificationTier::Tests {
                *verification_failed = false;
            }
        }
        VerificationOutcome::Fail => *verification_failed = true,
        VerificationOutcome::NoResult => {}
        VerificationOutcome::NotVerification => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_recognizes_build_test_lint() {
        assert!(is_verification_command("cargo build"));
        assert!(is_verification_command("npm test"));
        assert!(is_verification_command("cargo clippy -- -D warnings"));
        assert!(is_verification_command("tsc --noEmit"));
        // Toolchain subcommands decide, not substrings anywhere in the line.
        assert!(is_verification_command("cargo test"));
        assert!(is_verification_command("cargo check"));
        assert!(is_verification_command("npm run build"));
        assert!(is_verification_command("npm run typecheck"));
        assert!(is_verification_command("npm run test:unit"));
        assert!(is_verification_command("go test ./..."));
        assert!(is_verification_command("go build ./..."));
        assert!(is_verification_command("pytest tests/"));
        assert!(is_verification_command("npx tsc --noEmit"));
        assert!(is_verification_command("npx vitest run"));
        assert!(is_verification_command("python -m pytest"));
        assert!(is_verification_command("node --check main.js"));
        assert!(is_verification_command("python -m py_compile main.py"));
        assert!(is_verification_command("bun test"));
        assert!(is_verification_command("bun run test"));
        assert!(is_verification_command("bun run check"));
        assert!(is_verification_command("flutter test"));
        assert!(is_verification_command("flutter analyze"));
        assert!(is_verification_command("dart test"));
        assert!(is_verification_command("phpunit tests/"));
        assert!(is_verification_command("swift test"));
        // The LAST segment of a chain is what actually runs.
        assert!(is_verification_command("cd src && cargo test"));
        assert!(is_verification_command("cd src && npm run lint"));
    }

    #[test]
    fn verification_rejects_innocent_commands() {
        // Token-based matching: file names and innocent commands that merely
        // CONTAIN verification words must not count as verification.
        assert!(!is_verification_command("cat test.txt"));
        assert!(!is_verification_command("grep verify src/main.rs"));
        assert!(!is_verification_command("echo building things"));
        assert!(!is_verification_command("cat lint_results.txt"));
        assert!(!is_verification_command("rm -rf test/"));
        assert!(!is_verification_command("python main.py"));
        assert!(!is_verification_command("npm run dev"));
        assert!(!is_verification_command("cargo run"));
        assert!(!is_verification_command("node index.js"));
        assert!(!is_verification_command("cat verify.sh"));
        // Non-verifier subcommands of the newly-added toolchains stay out.
        assert!(!is_verification_command("bun run dev"));
        assert!(!is_verification_command("flutter run"));
        assert!(!is_verification_command("dart format ."));
    }

    #[test]
    fn verification_recognizes_windows_executable_variants() {
        // Windows models alternate executable spellings — the gate must
        // recognize real test/lint/build runs behind .exe/.cmd/.bat/.ps1
        // suffixes and relative bin paths.
        assert!(is_verification_command("npx.cmd tsc --noEmit"));
        assert!(is_verification_command("npm.cmd run build"));
        assert!(is_verification_command("cargo.exe test"));
        assert!(is_verification_command("tsc.exe --noEmit"));
        assert!(is_verification_command("python.exe -m pytest"));
        assert!(is_verification_command("node.exe --test"));
        assert!(is_verification_command(
            "./node_modules/.bin/tsc.cmd --noEmit"
        ));
        assert!(is_verification_command("pnpm.exe run lint"));
        // Innocent commands behind suffixes stay out.
        assert!(!is_verification_command("notepad.exe test.txt"));
        assert!(!is_verification_command("npm.cmd run dev"));
        assert!(!is_verification_command("cargo.exe run"));
    }

    #[test]
    fn insufficient_resource_retries_only_empty_output_within_budget() {
        // No output + within budget → retry.
        assert!(should_retry_insufficient_resource(
            "insufficient_system_resource",
            false,
            0
        ));
        assert!(should_retry_insufficient_resource(
            "insufficient_system_resource",
            false,
            2
        ));
        // Budget exhausted → no retry.
        assert!(!should_retry_insufficient_resource(
            "insufficient_system_resource",
            false,
            3
        ));
        // Partial output (text or tool calls) → usable, never retried.
        assert!(!should_retry_insufficient_resource(
            "insufficient_system_resource",
            true,
            0
        ));
        // Other finish reasons are not this handler's business.
        assert!(!should_retry_insufficient_resource("length", false, 0));
        assert!(!should_retry_insufficient_resource("stop", false, 0));
    }

    #[test]
    fn verification_is_token_based_not_substring_based() {
        // The failure-correctness comes from the outcome check, and the
        // gate must not fire on innocent commands: `cat test.txt` / `grep
        // verify` are reads, not verification steps.
        assert!(!is_verification_command("cat test.txt"));
        assert!(!is_verification_command("grep verify src/main.rs"));
        assert!(!is_verification_command("echo building things"));
    }

    #[test]
    fn failed_verification_command_is_fail_not_pass() {
        // A build/test that returned non-zero must NOT count as verified.
        assert_eq!(
            verification_outcome(
                "bash",
                Some("cargo build"),
                None,
                Some((true, String::new())),
            ),
            VerificationOutcome::Fail
        );
    }

    #[test]
    fn successful_verification_command_is_pass() {
        assert_eq!(
            verification_outcome(
                "bash",
                Some("cargo build"),
                None,
                Some((false, String::new())),
            ),
            VerificationOutcome::Pass(VerificationTier::Tests)
        );
    }

    #[test]
    fn verification_command_without_result_is_not_verified() {
        // A bash verification command that never ran (permission denied /
        // hook blocked) has no executed result — not verified.
        assert_eq!(
            verification_outcome("bash", Some("cargo build"), None, None),
            VerificationOutcome::NoResult
        );
    }

    #[test]
    fn no_result_does_not_reset_passed_verification() {
        // Pass then NoResult: the denied attempt is not evidence — the
        // passed state must survive, or the gate re-fires on evidence it
        // already accepted (successful test + denied retry → still verified).
        let mut tier = VerificationTier::None;
        let mut failed = false;
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Tests),
            &mut tier,
            &mut failed,
        );
        apply_verification_outcome(VerificationOutcome::NoResult, &mut tier, &mut failed);
        assert!(
            tier.is_at_least(VerificationTier::Tests),
            "already-passed verification must survive a NoResult"
        );
        assert!(!failed);
    }

    #[test]
    fn fail_flips_failed_but_noresult_keeps_prior_fail() {
        let mut tier = VerificationTier::Syntax;
        let mut failed = false;
        apply_verification_outcome(VerificationOutcome::Fail, &mut tier, &mut failed);
        apply_verification_outcome(VerificationOutcome::NoResult, &mut tier, &mut failed);
        assert!(failed, "a real failure stays failed");
        assert!(
            tier.is_at_least(VerificationTier::Syntax),
            "a later NoResult must not clear accepted evidence either"
        );
    }

    #[test]
    fn tests_pass_resolves_earlier_failure() {
        // A failed verification followed by a later Tests-tier pass is a
        // resolved fix — the stale "failed" flag must clear, or the brake
        // stays defeated and the evaluator reviews already-passing work.
        let mut tier = VerificationTier::None;
        let mut failed = false;
        apply_verification_outcome(VerificationOutcome::Fail, &mut tier, &mut failed);
        assert!(failed);
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Tests),
            &mut tier,
            &mut failed,
        );
        assert!(!failed, "a Tests pass resolves an earlier failure");
        assert_eq!(tier, VerificationTier::Tests);

        // Syntax alone does NOT resolve a failure (the test may still fail).
        let mut tier2 = VerificationTier::None;
        let mut failed2 = false;
        apply_verification_outcome(VerificationOutcome::Fail, &mut tier2, &mut failed2);
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Syntax),
            &mut tier2,
            &mut failed2,
        );
        assert!(failed2, "Syntax alone leaves a test failure unresolved");
    }

    #[test]
    fn lsp_failure_is_fail() {
        assert_eq!(
            verification_outcome(
                "lsp",
                None,
                Some("diagnostics"),
                Some((true, String::new())),
            ),
            VerificationOutcome::Fail
        );
    }

    #[test]
    fn lsp_success_without_content_is_no_result() {
        // A success result with no content isn't a usable verification.
        assert_eq!(
            verification_outcome(
                "lsp",
                None,
                Some("diagnostics"),
                Some((false, String::new())),
            ),
            VerificationOutcome::NoResult
        );
    }

    #[test]
    fn lsp_success_with_error_diagnostics_is_fail() {
        // The lsp tool reports diagnostics as SUCCESS with error text in
        // the content — a diagnostics run that found errors must count as
        // a failed verification.
        assert_eq!(
            verification_outcome(
                "lsp",
                None,
                Some("diagnostics"),
                Some((false, "[error] src/main.rs:12: expected `;`".to_string())),
            ),
            VerificationOutcome::Fail
        );
    }

    #[test]
    fn lsp_success_with_clean_diagnostics_is_pass() {
        assert_eq!(
            verification_outcome(
                "lsp",
                None,
                Some("diagnostics"),
                Some((false, "[info] src/main.rs:3: nothing here".to_string())),
            ),
            VerificationOutcome::Pass(VerificationTier::Syntax)
        );
    }

    #[test]
    fn lsp_non_diagnostics_operation_is_not_verification() {
        // hover/symbols/definition/references/format/workspace_symbols do not
        // check the edited code for errors — they must not count as evidence,
        // or the model defeats the verify gate with a no-op LSP lookup.
        let lookups = ["hover", "symbols", "definition", "references", "format", "workspace_symbols"];
        for op in lookups {
            assert_eq!(
                verification_outcome(
                    "lsp",
                    None,
                    Some(op),
                    Some((false, "fn main()".to_string())),
                ),
                VerificationOutcome::NotVerification,
                "lsp {op} must not count as verification"
            );
        }
        // A missing operation (anomalous) is likewise not verification.
        assert_eq!(
            verification_outcome(
                "lsp",
                None,
                None,
                Some((false, "fn main()".to_string())),
            ),
            VerificationOutcome::NotVerification
        );
    }

    #[test]
    fn verification_tiers_classify_syntax_vs_tests() {
        // Static checks are Syntax; real execution is Tests.
        assert_eq!(
            verification_command_tier("tsc --noEmit"),
            Some(VerificationTier::Syntax)
        );
        assert_eq!(
            verification_command_tier("cargo check"),
            Some(VerificationTier::Syntax)
        );
        assert_eq!(
            verification_command_tier("cargo clippy -- -D warnings"),
            Some(VerificationTier::Syntax)
        );
        assert_eq!(
            verification_command_tier("npm run lint"),
            Some(VerificationTier::Syntax)
        );
        assert_eq!(
            verification_command_tier("node --check main.js"),
            Some(VerificationTier::Syntax)
        );
        assert_eq!(
            verification_command_tier("python -m py_compile main.py"),
            Some(VerificationTier::Syntax)
        );

        assert_eq!(
            verification_command_tier("cargo test"),
            Some(VerificationTier::Tests)
        );
        assert_eq!(
            verification_command_tier("cargo build"),
            Some(VerificationTier::Tests)
        );
        assert_eq!(
            verification_command_tier("npx vitest run"),
            Some(VerificationTier::Tests)
        );
        assert_eq!(
            verification_command_tier("npm test"),
            Some(VerificationTier::Tests)
        );
        assert_eq!(
            verification_command_tier("npm run test:unit"),
            Some(VerificationTier::Tests)
        );
        assert_eq!(
            verification_command_tier("node --test"),
            Some(VerificationTier::Tests)
        );
    }

    #[test]
    fn verification_tier_only_moves_up() {
        // A successful test run then a static check keeps Tests — evidence
        // never downgrades (Syntax after Tests must not re-arm the review).
        let mut tier = VerificationTier::None;
        let mut failed = false;
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Tests),
            &mut tier,
            &mut failed,
        );
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Syntax),
            &mut tier,
            &mut failed,
        );
        assert_eq!(tier, VerificationTier::Tests);

        // And a syntax pass followed by tests upgrades to Tests.
        let mut tier2 = VerificationTier::None;
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Syntax),
            &mut tier2,
            &mut failed,
        );
        apply_verification_outcome(
            VerificationOutcome::Pass(VerificationTier::Tests),
            &mut tier2,
            &mut failed,
        );
        assert_eq!(tier2, VerificationTier::Tests);
    }

    #[test]
    fn non_verification_bash_is_not_verification() {
        // No verification marker in the command.
        assert_eq!(
            verification_outcome(
                "bash",
                Some("echo building things"),
                None,
                Some((false, String::new()))
            ),
            VerificationOutcome::NotVerification
        );
    }

    #[test]
    fn and_chain_takes_strongest_segment_tier() {
        // `npm test && npm run lint` proves Tests (the first segment), not the
        // trailing lint's Syntax — every && segment must succeed for the chain
        // to pass, so the strongest evidence is what was earned.
        assert_eq!(
            verification_command_tier("npm test && npm run lint"),
            Some(VerificationTier::Tests)
        );
        // A trailing non-verifier does not discard a strong earlier segment.
        assert_eq!(
            verification_command_tier("cargo test && echo done"),
            Some(VerificationTier::Tests)
        );
        // `;` still reduces to the last command (only its exit code is
        // reported by the shell).
        assert_eq!(verification_command_tier("cargo test; cat out.txt"), None);
        // `go build` is execution evidence (Tests), matching every other
        // toolchain (cargo/make/flutter/swift all map build → Tests).
        assert_eq!(
            verification_command_tier("go build ./..."),
            Some(VerificationTier::Tests)
        );
    }

    #[test]
    fn lsp_content_error_detection() {
        assert!(lsp_content_has_errors("[error] a.rs:1: bad"));
        assert!(lsp_content_has_errors("[Error] a.rs:1: bad"));
        assert!(lsp_content_has_errors("[ERROR] a.rs:1: bad"));
        assert!(lsp_content_has_errors("[eRrOr] a.rs:1: bad"));
        assert!(!lsp_content_has_errors("[warning] a.rs:1: maybe"));
        assert!(!lsp_content_has_errors("No diagnostics."));
    }
}
