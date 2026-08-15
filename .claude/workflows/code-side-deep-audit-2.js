export const meta = {
  name: 'code-side-deep-audit-2',
  description: 'Second parallel deep audit of DeepDepCat backend across 9 previously-uncovered dimensions, with adversarial verification',
  phases: [
    { title: 'Audit', detail: '9 finder agents, one per un-audited dimension' },
    { title: 'Verify', detail: 'one skeptic per finding, refute-by-reading' },
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

const GROUND_RULES = `You are auditing the Rust backend of DeepDepCat (Tauri 2 desktop AI agent) for REAL, actionable defects in areas NOT yet covered by prior audits. Find concrete issues that make the app less correct, less robust, or less secure — NOT vague code smells.

GROUND RULES:
- READ the actual code before reporting. Use Grep -n or Read to confirm exact file:line numbers.
- Every finding MUST have a concrete failure_scenario: given X input/state, the system produces Y wrong output / crashes / leaks / corrupts data.
- No vague "could be cleaner" / style / naming findings.
- Return 0..6 findings. Quality over quantity. If nothing real, return an empty array. Do NOT pad.
- ALREADY FIXED (do NOT re-report): tool-result aggregate budget; compaction estimate; frozen dynamic context; completion hard-brake; TurnEnd failure paths; prefire; subagent semaphore/timeout; skills activation; code-verify nudge; shell detection; search_replace double-apply; apply_patch CRLF/capture_file_state; LLM stream UTF-8; cost.rs hardcoded pricing; verification scan index; lsp any-op = Syntax; go build unrecognized; structured tool-call truncation; token bytes/4 CJK; intent greeting/script/double-digit/16char/budget; multi-agent partial-report/HashMap-evict/merge-back; compaction chunk tool-pair; MCP app serial drop; read_file/diff_preview size guard; file_operation_lock description; sanitize braces; "0 passed" substring; transient dedup; verification gate scratch-file filter (is_in_workspace); nudge text.
- RED LINE — do NOT flag these as needing changes (owned by a parallel session, read-only): src-tauri/src/permissions/**, src-tauri/src/tools/builtin/depwork/**, and frontend src/**. If you see a real bug there, mention it ONLY in notes, not findings.
- Do NOT propose anything that violates the // deepseek-native: protected markers or collapses Code/Depwork isolation.
- Categories: correctness, efficiency, robustness, maintainability, model-quality, security.`

const DIMENSIONS = [
  {
    key: 'core-foundations',
    scope: 'src-tauri/src/core/ (config/mod.rs + sections.rs, stream.rs, proc/win/, error.rs, encoding.rs, image_codec.rs, image_codec_validate.rs, dsml.rs, str_util.rs, pattern.rs, ids.rs, feature_flag.rs, crash.rs, managed.rs, types/)',
    focus: `Investigate: (1) config load/merge/default correctness — env overrides, malformed TOML, path resolution. (2) stream event ordering/seq — can the frontend reducer drop or reorder events? (3) proc/win process/job management — child process cleanup, kill-on-drop, handle leaks. (4) encoding decode_native_output (UTF-8 → GBK → UTF-16) correctness — can it corrupt or misdetect? (5) image_codec/validate — decompression bombs, oversized images, validation bypass. (6) dsml parse/strip — malformed tool-call markup, escaping. (7) str_util truncate/strip — char-boundary safety, off-by-one. (8) ids uniqueness/collision. (9) crash/panic handlers. (10) type serialization (serde) round-trips.`,
  },
  {
    key: 'storage-database',
    scope: 'src-tauri/src/storage/ (schema.rs, mod.rs, database/{mod,sessions,messages,events,tasks,research,settings,usage,helpers}.rs)',
    focus: `Investigate: (1) SQL correctness — parameter binding vs string interpolation, SQL injection via user-controlled strings (session ids, titles, paths, goals). (2) migration safety — version bumps, idempotent re-apply, partial-failure. (3) WAL/concurrency — Mutex<Connection> held across awaits? deadlock? (4) retention/pruning — off-by-one, deleting wrong rows. (5) message persistence — conversation_order, parent_message_id integrity, replace_messages atomicity. (6) research/tasks/settings CRUD — upsert vs insert races. (7) type coercion — i64/u64 overflow, Option handling. (8) data loss on error paths (rollback missing).`,
  },
  {
    key: 'mcp',
    scope: 'src-tauri/src/mcp/ (manager.rs, client.rs, tool_bridge.rs, mod.rs, smoke_test.rs)',
    focus: `Investigate: (1) tool dispatch correctness — request/result correlation, tool name collision, schema validation. (2) App payload propagation — mcp_app attached under metadata, dropped anywhere? (3) resource cleanup — server processes, connections, handles leaked on error/cancel. (4) protocol correctness — JSON-RPC framing, SSE parsing, partial messages. (5) server lifecycle — start/stop/restart races, stale servers. (6) error handling — one failing MCP server poisoning the whole toolset? (7) timeout/cancellation.`,
  },
  {
    key: 'hooks',
    scope: 'src-tauri/src/hooks/ (executor.rs, registry.rs, mod.rs)',
    focus: `Investigate: (1) gate vs observe semantics — can a PreToolUse gate error block a tool incorrectly? (2) timeout handling — does a slow hook stall the loop, or fail-open? (3) error isolation — one hook error breaking others? (4) registry registration — duplicate hooks, ordering, removal. (5) hook payload/context construction — missing fields. (6) async hook execution — re-entrancy, lock ordering.`,
  },
  {
    key: 'memory',
    scope: 'src-tauri/src/memory/ (learning.rs, injection.rs, watcher.rs, mod.rs, live_smoke.rs)',
    focus: `Investigate: (1) learning extraction — does it mis-learn (extract garbage into learnings.md), duplicate, or miss? (2) injection ordering/priority — memory.md + learnings.md + procedures merge correctly? (3) watcher — file-watch events, debounce, spurious reload, infinite loop (watching its own writes). (4) throttling (once/10min) correctness. (5) encoding — non-UTF-8 memory files. (6) error handling — silent failures swallowing real errors.`,
  },
  {
    key: 'skills',
    scope: 'src-tauri/src/skills/ (loader.rs, activation.rs, types.rs, bundled.rs, mod.rs)',
    focus: `Investigate: (1) frontmatter parsing — malformed YAML, missing fields, bad when_to_use, name/description extraction. (2) mode-dir scanning — ~/.deepdepcat/{depwork,code}/skills, symlinks, duplicate skill names across dirs. (3) activation — path + keyword + when_to_use logic, false positives/negatives. (4) allowed-tools enforcement — does the skill's tool allowlist actually gate? (5) caching — stale skill lists after file changes. (6) path traversal / malformed paths.`,
  },
  {
    key: 'workspace',
    scope: 'src-tauri/src/workspace/ (checkpoint.rs, isolation.rs, project_files.rs, project_structure.rs, mod.rs)',
    focus: `Investigate: (1) checkpoint/rewind — snapshot integrity, restore correctness, partial rewind, ghost files. (2) isolation (worktree) — merge-back correctness, dirty-tree handling, cleanup on abort. (3) project_files discovery — instruction-file priority (DEEPDEPCAT.md vs instructions.md vs CLAUDE.md), symlink loops, encoding fallback. (4) project_structure scan — ignore rules, large dirs, binary files. (5) path resolution — workspace-relative vs absolute, dot-dot traversal. (6) rewind across compaction.`,
  },
  {
    key: 'llm-infra',
    scope: 'src-tauri/src/llm/ (retry.rs, circuit_breaker.rs, sampler.rs, routing.rs, provider.rs, streaming.rs, client/{openai,anthropic,responses}.rs)',
    focus: `Investigate (client streaming UTF-8 already fixed, look elsewhere): (1) retry classification — retry-on-which-errors, 429 Retry-After, 4xx no-retry, retry storm, jitter arithmetic. (2) circuit breaker — open/close/half-open transitions, threshold counting, reset. (3) doom-loop sampler — false positives/negatives, signal accumulation. (4) routing — model→provider resolution, fallback model resolution, cache-hit paths. (5) provider protocol — deepseek-native chat/completions vs anthropic/responses, header/field correctness, usage parsing. (6) streaming parser — SSE framing, partial tool-call args, finish_reason propagation.`,
  },
  {
    key: 'commands-bootstrap',
    scope: 'src-tauri/src/commands/*.rs, bootstrap/*.rs, a2a/, acp/, automation/, scheduler/, sse.rs, browser/',
    focus: `Investigate: (1) Tauri command wiring — AppState access, lock ordering (sessions/queues/cancellation), deadlock. (2) session lifecycle — checkout/put back, busy queue, cancellation cleanup. (3) error propagation to frontend — stream Error events, TurnEnd on every path. (4) a2a/acp protocol — message framing, auth, concurrency. (5) automation/scheduler — task persistence, re-fire, dedup. (6) bootstrap init — partial-init recovery, config/db failure. (7) rewind/pause/resume command correctness. (8) observability/usage accounting.`,
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

- real = true ONLY if, after reading the code, this is a genuine, non-trivial issue worth fixing (a real correctness/robustness/efficiency/security problem in the actual code path). If the claim is wrong, already handled, only triggers on impossible input, or is a trivial style nit, real = false.
- severity: high = wrong behavior/crash/data-loss/security in a common path; medium = real issue in a less-common path; low = minor but real.
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
    )).then(verified => ({ dimension: d.key, notes: d.notes, found: d.findings.length, verified }))
  )

  const confirmed = results.flatMap(r => r.verified).filter(Boolean).filter(f => f.verdict && f.verdict.real)
  const sevRank = { high: 0, medium: 1, low: 2 }
  confirmed.sort((a, b) => sevRank[a.verdict.severity] - sevRank[b.verdict.severity])

  return {
    confirmedCount: confirmed.length,
    confirmed,
    coverage: results.map(r => ({ dimension: r.dimension, found: r.found, confirmed: r.verified.filter(f => f && f.verdict && f.verdict.real).length, notes: r.notes })),
  }
})()
