export const meta = {
  name: 'code-side-deep-audit',
  description: 'Parallel deep audit of DeepDepCat Code agent across 9 un-audited dimensions, with adversarial verification of every finding',
  phases: [
    { title: 'Audit', detail: '9 finder agents, one per un-audited dimension' },
    { title: 'Verify', detail: 'one skeptic per finding, refute-by-reading the actual code' },
  ],
}

const FINDING = {
  type: 'object',
  additionalProperties: false,
  properties: {
    title: { type: 'string' },
    file: { type: 'string' },
    line: { type: 'integer' },
    category: { type: 'string', enum: ['correctness', 'efficiency', 'robustness', 'maintainability', 'model-quality', 'security'] },
    summary: { type: 'string' },
    failure_scenario: { type: 'string' },
    confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
  },
  required: ['title', 'file', 'line', 'category', 'summary', 'failure_scenario'],
}

const FINDINGS_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    findings: { type: 'array', items: FINDING },
    notes: { type: 'string' },
  },
  required: ['findings'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    real: { type: 'boolean' },
    severity: { type: 'string', enum: ['high', 'medium', 'low'] },
    reason: { type: 'string' },
  },
  required: ['real', 'severity', 'reason'],
}

const GROUND_RULES = `You are auditing the Rust backend of DeepDepCat (Tauri 2 desktop AI agent) for REAL, actionable defects and optimizations in the "Code" agent capability. Find concrete issues that make the agent less capable, less correct, or less robust, not vague code smells.

GROUND RULES:
- READ the actual code before reporting. Use Grep -n or Read to confirm exact file:line numbers.
- Every finding MUST have a concrete failure_scenario: given X input/state, the system produces Y wrong output / crashes / wastes Z resources.
- No vague "could be cleaner" / "consider refactoring" / style / naming findings.
- Return 0..6 findings. Quality over quantity. If you find nothing real, return an empty array; that is a valid, valuable result. Do NOT pad.
- ALREADY FIXED / AUDITED, do NOT re-report unless a genuinely NEW issue: tool-result aggregate budget in chat_state; compaction estimate mismatch in compact_if_needed; frozen dynamic context in context_phase; completion hard-brake bypass in stop_gates; TurnEnd on failure paths; prefire estimate; subagent semaphore deadlock and timeout ghost workers (spawn/coordinator); skills activation bugs; code-verify nudge; shell detection ordering in bash.rs.
- RED LINE, do NOT flag these as needing changes (owned by a parallel session, read-only): src-tauri/src/permissions/**, src-tauri/src/tools/builtin/depwork/**, and frontend src/**. If you see a real bug there, mention it only in notes, not in findings.
- Do NOT propose anything that violates the deepseek-native protected markers or collapses Code/Depwork isolation.
- Categories: correctness (wrong behavior), efficiency (real waste at scale), robustness (edge cases/races/error handling), maintainability (real footguns, not style), model-quality (degrades the model's ability to understand/act: misleading tool descriptions, missing context, ambiguous outputs), security.`

const DIMENSIONS = [
  {
    key: 'token-budget-effort',
    scope: 'src-tauri/src/agent/token.rs, budget.rs, intent_effort.rs, chat_state/mod.rs (token estimation + budget enforcement, NOT the aggregate budget)',
    focus: `Investigate: (1) token counting correctness, does it match how the model actually counts (UTF-8 multi-byte, CJK, emoji, code blocks)? Off-by-one or systematic undercount means budget silently exceeded. (2) budget/cost enforcement, are the hard-stop thresholds (step/token/cost) actually enforced everywhere, or can a path bypass them? (3) effort to reasoning-effort mapping correctness. (4) any place token count is estimated with a stale/empty tool list, producing a wrong compaction decision.`,
  },
  {
    key: 'compaction',
    scope: 'src-tauri/src/agent/compaction/ (mod.rs, item.rs, select.rs, sampler.rs, templates.rs, history/filter.rs, validate.rs, types.rs)',
    focus: `Investigate: (1) dedup correctness, can the dedup/summary drop a tool_call but keep its result (or vice versa), leaving the model confused? (2) sampler bias, does sampling drop high-value context (goals, user intent, recent turns, verification results)? (3) summary loss, does the D&C chunked summary preserve cross-turn dependencies? (4) threshold arithmetic, off-by-one or integer overflow in the prune/re-measure/summarize flow. (5) chunk boundaries, does chunking split a tool_call plus its result across different summary chunks? (6) externalization correctness.`,
  },
  {
    key: 'verification-evaluator-reflexion',
    scope: 'src-tauri/src/agent/agent_loop/verification.rs, evaluator.rs, reflexion.rs, gates.rs, run/stop_gates.rs',
    focus: `Investigate: (1) verification tier logic (None/Syntax/Tests), is pass/fail detected correctly? False positives (passing when it should fail) or false negatives (failing on clean code)? (2) does Syntax verification actually compile/parse or just do a superficial check? (3) gate correctness, can a gate be satisfied when it should not, or block progress when it should not? (4) reflexion loop termination, can it loop forever or lose information? (5) evaluator scoring correctness, is the score meaningfully computed or does it double-count / miss evidence? (6) edited_code_unverified logic in stop_gates, document-file classification correctness.`,
  },
  {
    key: 'agent-loop-toolbatch',
    scope: 'src-tauri/src/agent/agent_loop/run/ (request_phase.rs, tool_phase.rs, context_phase.rs, housekeeping.rs, background.rs, state.rs, mod.rs), agent_loop/mod.rs, agent_loop/tool_batch/ (mod.rs, parallel.rs, serial.rs, concurrent.rs, orchestrate.rs, support.rs)',
    focus: `Investigate: (1) loop lifecycle correctness, is TurnEnd emitted on EVERY exit path (success, stream error, tool error, cancel, stop gate)? (2) context composition, is dynamic context plus tail suffix recomposed correctly each iteration, no stale or stale-duplicate content? (3) tool batch orchestration, parallel vs serial correctness, dependency/ordering handling, error isolation (one tool failure poisoning the batch?), result assembly. (4) housekeeping correctness. (5) state transitions, can LoopState get into an inconsistent state?`,
  },
  {
    key: 'intent-routing',
    scope: 'src-tauri/src/agent/intent/ (mod.rs, classify.rs, route.rs, signals.rs, spec.rs, text.rs, types.rs, intent_effort.rs)',
    focus: `Investigate: (1) classification accuracy and edge cases, are there inputs misclassified that would route to the wrong mode/toolset? (2) routing correctness, does classification map to the right Code vs Depwork toolset/skill set? (3) signal detection false positives/negatives. (4) effort scaling, does effort map correctly to reasoning-effort plus budget? (5) spec parsing correctness. (6) does the intent system degrade on short/ambiguous user messages?`,
  },
  {
    key: 'streaming-recovery-sanitize',
    scope: 'src-tauri/src/agent/streaming.rs, recovery.rs, sanitize.rs, token.rs (streaming/counting), core/stream.rs, core/types/stream.rs',
    focus: `Investigate: (1) partial-parse handling, can a truncated tool_call JSON at a chunk boundary be mis-parsed or dropped? (2) retry/backoff correctness, exponential backoff with jitter, retry-on-which-errors, retry storm risk, no-retry-on-4xx. (3) sanitization correctness, does sanitize corrupt legit content (code, CJK, emoji, control chars in strings)? (4) streaming truncation/throttling correctness, can a stream be cut mid-token or mid-tool-call? (5) does recovery correctly restore state after a failed turn?`,
  },
  {
    key: 'multiagent-workflow',
    scope: 'src-tauri/src/agent/multi_agent/ (mod.rs, fork.rs, coordinator.rs, spawn.rs, types.rs), agent/workflow/ (mod.rs, executor.rs), tools/builtin/agent_tool.rs, tools/builtin/workflow_tool.rs',
    focus: `Investigate (semaphore deadlock + timeout ghost workers already fixed, look elsewhere): (1) fork correctness, does the forked child get the full needed parent context (not just a snippet)? (2) coordinator state machine correctness, can it stall on a child that errored, or skip a phase? (3) result merging, are subagent results merged without loss/duplication? (4) resource cleanup, are worker tasks/sessions/permits always released on every exit path? (5) workflow executor DSL correctness, error propagation, step ordering, dependency resolution. (6) error propagation from child to parent, is the parent told WHY a child failed, or just "failed"?`,
  },
  {
    key: 'code-tools-quality',
    scope: 'src-tauri/src/tools/builtin/ (bash.rs, edit_file.rs, search_replace.rs, apply_patch.rs, read_file.rs, read_file_document.rs, read_file_image.rs, read_file_pdf.rs, grep.rs, glob.rs, list_dir.rs, write_file.rs, code_search.rs, diff_preview.rs, stale_edit.rs, cursor_rules_on_read.rs, file_operation_lock.rs, todo_write.rs, plan_mode.rs, memory_ops.rs), tools/ (mod.rs, dispatch.rs, registry.rs, background.rs, failure_guidance.rs, reminders.rs, stale_edit.rs)',
    focus: `Investigate: (1) tool correctness bugs and edge cases, empty file, binary file, very large file, line-ending/encoding, path normalization, symlinks. (2) TOOL DESCRIPTION quality, the model decides when/how to call a tool by reading its description plus schema; misleading, incomplete, or contradictory descriptions directly degrade capability. Flag missing param descriptions, wrong type hints, missing "when NOT to use". (3) error messages returned to the model, are they actionable (tell the model what to do next) or dead-ends? (4) dispatch/routing correctness, cancellation handling, stale-edit detection correctness. (5) does search_replace / apply_patch handle non-unique or fuzzy matches safely?`,
  },
  {
    key: 'context-state-reminders',
    scope: 'src-tauri/src/agent/context.rs, chat_state/ (mod.rs, snapshot.rs), system_reminder.rs, prompt_queue.rs, interjection/mod.rs, agent_loop/run/reminders.rs, tools/reminders.rs, notification.rs',
    focus: `Investigate: (1) context window estimation correctness, is the estimate used for compaction/cache decisions accurate, or does it drift from reality? (2) tail injection / cache stability, does the composed context stay byte-stable across turns so DeepSeek prefix KV cache actually hits, or is there a per-turn variable that breaks the cache? (3) system reminder dedup, can the same reminder be injected repeatedly? (4) prompt queue ordering. (5) interjection correctness, can an interjection fire at the wrong time or clobber user content? (6) reminder dedup/spam.`,
  },
]

function verifyPrompt(d, f) {
  return `You are a skeptical code reviewer. A finder agent claims the following defect in DeepDepCat (dimension: ${d.key}).

CLAIM:
- title: ${f.title}
- file: ${f.file}
- line: ${f.line}
- category: ${f.category}
- summary: ${f.summary}
- failure_scenario: ${f.failure_scenario}

Your job is to REFUTE it if possible. Read the actual file at the cited line (and surrounding context) with Read/Grep. Then decide:

- real = true ONLY if, after reading the code, this is a genuine, non-trivial issue worth fixing (a real correctness/robustness/efficiency/model-quality/security problem in the actual code path). If the claim is wrong, the code already handles it, it only triggers on impossible input, or it is a trivial style nit, real = false.
- severity: high = wrong behavior/crash/data-loss/security in a common path; medium = real correctness/efficiency/model-quality issue in a less-common path; low = minor but real.
- reason: 1-2 sentences stating what the code actually does and whether the claim holds (cite what you read).

Do NOT invent issues, and do NOT confirm a claim you could not verify by reading the code. If you cannot verify it, set real=false and say so.`
}

return (async () => {
  const results = await pipeline(
    DIMENSIONS,
    d => agent(
      `${GROUND_RULES}\n\nDIMENSION: ${d.key}\nSCOPE (files to read): ${d.scope}\n\n${d.focus}\n\nReturn findings[] and notes (coverage + what you skipped).`,
      { label: `audit:${d.key}`, phase: 'Audit', schema: FINDINGS_SCHEMA }
    ).then(r => ({ ...d, findings: (r && r.findings) || [], notes: (r && r.notes) || '' })),
    d => parallel(d.findings.map(f => () =>
      agent(verifyPrompt(d, f), { label: `verify:${f.file}:${f.line}`, phase: 'Verify', schema: VERDICT_SCHEMA })
        .then(v => ({ ...f, dimension: d.key, verdict: v || { real: false, severity: 'low', reason: 'verifier returned null' } }))
    ))
  )

  const confirmed = results.flat().filter(Boolean).filter(f => f.verdict && f.verdict.real)
  const sevRank = { high: 0, medium: 1, low: 2 }
  confirmed.sort((a, b) => sevRank[a.verdict.severity] - sevRank[b.verdict.severity])

  return {
    confirmedCount: confirmed.length,
    confirmed,
    coverage: results.map(d => ({ dimension: d.key, found: d.findings.length, notes: d.notes })),
  }
})()
