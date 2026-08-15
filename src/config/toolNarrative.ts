/**
 * toolNarrative — the single source of tool-call narrative language.
 *
 * Every tool card / read group reads its verb + target from here, so the
 * running/done/error copy stays consistent across ToolCallCard and ReadGroup
 * (previously two hand-rolled tables drifted apart).
 *
 * Copy is i18n-driven (keys in `toolCall.verbs`), targets are extracted
 * per tool family, and bash/run_command carry a "shell" badge.
 */

/** Per-tool verb triplet — resolved through i18n keys. */
export const TOOL_VERB_KEYS: Record<string, { running: string; done: string; error: string }> = {
  read_file: { running: "toolCall.verbReadRunning", done: "toolCall.verbReadDone", error: "toolCall.verbReadError" },
  write_file: { running: "toolCall.verbWriteRunning", done: "toolCall.verbWriteDone", error: "toolCall.verbWriteError" },
  edit_file: { running: "toolCall.verbEditRunning", done: "toolCall.verbEditDone", error: "toolCall.verbEditError" },
  search_replace: { running: "toolCall.verbEditRunning", done: "toolCall.verbEditDone", error: "toolCall.verbEditError" },
  list_dir: { running: "toolCall.verbListRunning", done: "toolCall.verbListDone", error: "toolCall.verbListError" },
  grep: { running: "toolCall.verbGrepRunning", done: "toolCall.verbGrepDone", error: "toolCall.verbGrepError" },
  glob: { running: "toolCall.verbGlobRunning", done: "toolCall.verbGlobDone", error: "toolCall.verbGlobError" },
  bash: { running: "toolCall.verbBashRunning", done: "toolCall.verbBashDone", error: "toolCall.verbBashError" },
  run_command: { running: "toolCall.verbBashRunning", done: "toolCall.verbBashDone", error: "toolCall.verbBashError" },
  web_fetch: { running: "toolCall.verbFetchRunning", done: "toolCall.verbFetchDone", error: "toolCall.verbFetchError" },
  web_fetch_depwork: { running: "toolCall.verbFetchRunning", done: "toolCall.verbFetchDone", error: "toolCall.verbFetchError" },
  web_search: { running: "toolCall.verbSearchRunning", done: "toolCall.verbSearchDone", error: "toolCall.verbSearchError" },
  memory_search: { running: "toolCall.verbMemorySearchRunning", done: "toolCall.verbMemorySearchDone", error: "toolCall.verbMemorySearchError" },
  memory_store: { running: "toolCall.verbMemoryStoreRunning", done: "toolCall.verbMemoryStoreDone", error: "toolCall.verbMemoryStoreError" },
  agent: { running: "toolCall.verbAgentRunning", done: "toolCall.verbAgentDone", error: "toolCall.verbAgentError" },
  ask_user: { running: "toolCall.verbAskRunning", done: "toolCall.verbAskDone", error: "toolCall.verbAskError" },
  task_manage: { running: "toolCall.verbTaskRunning", done: "toolCall.verbTaskDone", error: "toolCall.verbTaskError" },

  // ── Depwork (document / media processing) tools ──────────────
  doc_read: { running: "toolCall.verbDocReadRunning", done: "toolCall.verbDocReadDone", error: "toolCall.verbDocReadError" },
  docx_generate: { running: "toolCall.verbDocxRunning", done: "toolCall.verbDocxDone", error: "toolCall.verbDocxError" },
  ppt_generate: { running: "toolCall.verbPptRunning", done: "toolCall.verbPptDone", error: "toolCall.verbPptError" },
  table_process: { running: "toolCall.verbTableRunning", done: "toolCall.verbTableDone", error: "toolCall.verbTableError" },
  batch_file: { running: "toolCall.verbBatchRunning", done: "toolCall.verbBatchDone", error: "toolCall.verbBatchError" },
  ui_automate: { running: "toolCall.verbUiRunning", done: "toolCall.verbUiDone", error: "toolCall.verbUiError" },
  web_open: { running: "toolCall.verbWebOpenRunning", done: "toolCall.verbWebOpenDone", error: "toolCall.verbWebOpenError" },
  media_probe: { running: "toolCall.verbMediaRunning", done: "toolCall.verbMediaDone", error: "toolCall.verbMediaError" },
  media_convert: { running: "toolCall.verbMediaRunning", done: "toolCall.verbMediaDone", error: "toolCall.verbMediaError" },
  ocr_image: { running: "toolCall.verbOcrRunning", done: "toolCall.verbOcrDone", error: "toolCall.verbOcrError" },
  chart_generate: { running: "toolCall.verbChartRunning", done: "toolCall.verbChartDone", error: "toolCall.verbChartError" },
};

/** Tools without a dedicated verb map to a shared FAMILY verb set — the
 *  registry has ~70 tools and per-tool copy for all of them would drift.
 *  Families keep the stream readable (apply_patch ≠ research ≠ scheduler). */
export const TOOL_FAMILIES: Record<string, string> = {
  apply_patch: "patch",
  search_symbols: "index",
  file_dependencies: "index",
  lsp: "index",
  kill_task: "kill",
  memory_learn: "learn",
  monitor: "monitor",
  enter_plan_mode: "plan",
  exit_plan_mode: "plan",
  scheduler_create: "schedule",
  scheduler_list: "schedule",
  scheduler_delete: "schedule",
  todo_write: "todo",
  update_goal: "goal",
  use_tool: "meta",
  user_profile: "profile",
  visual_describe: "vision",
  wait_tasks: "wait",
  workflow: "workflow",
  browser_control: "browser",
  card_generate: "card",
  doc_consistency: "docsearch",
  docx_edit: "docedit",
  docx_search: "docsearch",
  live_doc_write: "docwrite",
  office_automate: "office",
  pdf_generate: "pdf",
  pdf_tools: "pdf",
  research_search: "research",
  research_save: "research",
  research_list: "research",
  research_remove: "research",
  research_export: "research",
  research_folder_search: "research",
  research_clip: "research",
  research_report: "research",
  research_open_access: "research",
  citation_link: "research",
  store_research_map: "storeresearch",
  store_research_xhs: "storeresearch",
  store_research_geo: "storeresearch",
  xlsx_generate: "xlsx",
  dev_browser_open: "devbrowser",
  content_pack: "pack",
};

/** Family verb triplets — i18n keys in the toolCall namespace. */
export const TOOL_FAMILY_VERB_KEYS: Record<
  string,
  { running: string; done: string; error: string }
> = {
  patch: { running: "toolCall.verbPatchRunning", done: "toolCall.verbPatchDone", error: "toolCall.verbPatchError" },
  index: { running: "toolCall.verbIndexRunning", done: "toolCall.verbIndexDone", error: "toolCall.verbIndexError" },
  kill: { running: "toolCall.verbKillRunning", done: "toolCall.verbKillDone", error: "toolCall.verbKillError" },
  learn: { running: "toolCall.verbLearnRunning", done: "toolCall.verbLearnDone", error: "toolCall.verbLearnError" },
  monitor: { running: "toolCall.verbMonitorRunning", done: "toolCall.verbMonitorDone", error: "toolCall.verbMonitorError" },
  plan: { running: "toolCall.verbPlanRunning", done: "toolCall.verbPlanDone", error: "toolCall.verbPlanError" },
  schedule: { running: "toolCall.verbScheduleRunning", done: "toolCall.verbScheduleDone", error: "toolCall.verbScheduleError" },
  todo: { running: "toolCall.verbTodoRunning", done: "toolCall.verbTodoDone", error: "toolCall.verbTodoError" },
  goal: { running: "toolCall.verbGoalRunning", done: "toolCall.verbGoalDone", error: "toolCall.verbGoalError" },
  meta: { running: "toolCall.verbMetaRunning", done: "toolCall.verbMetaDone", error: "toolCall.verbMetaError" },
  profile: { running: "toolCall.verbProfileRunning", done: "toolCall.verbProfileDone", error: "toolCall.verbProfileError" },
  vision: { running: "toolCall.verbVisionRunning", done: "toolCall.verbVisionDone", error: "toolCall.verbVisionError" },
  wait: { running: "toolCall.verbWaitRunning", done: "toolCall.verbWaitDone", error: "toolCall.verbWaitError" },
  workflow: { running: "toolCall.verbWorkflowRunning", done: "toolCall.verbWorkflowDone", error: "toolCall.verbWorkflowError" },
  browser: { running: "toolCall.verbBrowserRunning", done: "toolCall.verbBrowserDone", error: "toolCall.verbBrowserError" },
  card: { running: "toolCall.verbCardRunning", done: "toolCall.verbCardDone", error: "toolCall.verbCardError" },
  docedit: { running: "toolCall.verbDocEditRunning", done: "toolCall.verbDocEditDone", error: "toolCall.verbDocEditError" },
  docsearch: { running: "toolCall.verbDocSearchRunning", done: "toolCall.verbDocSearchDone", error: "toolCall.verbDocSearchError" },
  docwrite: { running: "toolCall.verbDocWriteRunning", done: "toolCall.verbDocWriteDone", error: "toolCall.verbDocWriteError" },
  office: { running: "toolCall.verbOfficeRunning", done: "toolCall.verbOfficeDone", error: "toolCall.verbOfficeError" },
  pdf: { running: "toolCall.verbPdfRunning", done: "toolCall.verbPdfDone", error: "toolCall.verbPdfError" },
  research: { running: "toolCall.verbResearchRunning", done: "toolCall.verbResearchDone", error: "toolCall.verbResearchError" },
  storeresearch: { running: "toolCall.verbStoreResearchRunning", done: "toolCall.verbStoreResearchDone", error: "toolCall.verbStoreResearchError" },
  xlsx: { running: "toolCall.verbXlsxRunning", done: "toolCall.verbXlsxDone", error: "toolCall.verbXlsxError" },
  pack: { running: "toolCall.verbPackRunning", done: "toolCall.verbPackDone", error: "toolCall.verbPackError" },
  devbrowser: { running: "toolCall.verbDevBrowserRunning", done: "toolCall.verbDevBrowserDone", error: "toolCall.verbDevBrowserError" },
  mcp: { running: "toolCall.verbMcpRunning", done: "toolCall.verbMcpDone", error: "toolCall.verbMcpError" },
};

export type ToolVerbState = "running" | "done" | "error";

/** The i18n key for a tool's verb in a given state. */
export function toolVerbKey(name: string, state: ToolVerbState): string {
  if (TOOL_VERB_KEYS[name]) return TOOL_VERB_KEYS[name][state];
  // MCP tools are dynamic (`mcp__<server>__<tool>`) — one shared verb set.
  if (name.startsWith("mcp__")) return TOOL_FAMILY_VERB_KEYS.mcp[state];
  const family = TOOL_FAMILIES[name];
  if (family && TOOL_FAMILY_VERB_KEYS[family]) return TOOL_FAMILY_VERB_KEYS[family][state];
  return TOOL_VERB_KEYS.task_manage[state];
}

/** Tools whose side effect is running a shell command — shown with a badge. */
const SHELL_TOOLS = new Set(["bash", "run_command"]);

export function isShellTool(name: string): boolean {
  return SHELL_TOOLS.has(name);
}

/** Read-only tools — consecutive calls collapse into one summary group
 *  (Claude-style collapsed read/search group). */
const READ_TOOLS = new Set([
  "read_file",
  "list_dir",
  "grep",
  "glob",
  "web_fetch",
  "web_fetch_depwork",
  "web_search",
  "memory_search",
]);

export function isReadTool(name: string): boolean {
  return READ_TOOLS.has(name);
}

/** Subagent type → i18n label key (agent tool narrative). */
export const AGENT_TYPE_LABEL_KEYS: Record<string, string> = {
  explore: "toolCall.agentTypeExplore",
  plan: "toolCall.agentTypePlan",
  general: "toolCall.agentTypeGeneral",
};

export function agentTypeLabelKey(agentType: string): string {
  return AGENT_TYPE_LABEL_KEYS[agentType] ?? "toolCall.agentTypeGeneral";
}

/** Built-in worker types — anything else is a CUSTOM specialist agent
 *  (the Depwork "群聊" flow summons these via the agent tool). */
const BUILTIN_AGENT_TYPES = new Set(["general", "explore", "plan", "evaluator"]);

export function isCustomSpecialist(agentType: string): boolean {
  return !BUILTIN_AGENT_TYPES.has(agentType);
}

// ── Target extraction ─────────────────────────────────────────

function get(args: Record<string, unknown>, key: string): string | null {
  return typeof args[key] === "string" && args[key] ? args[key] : null;
}

/** Last path segment (cross-platform). */
function shortName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

/** Shorten long targets (commands, queries) to fit the card line. */
function clip(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

/**
 * Summarize a subagent task for display.
 *
 * Evaluator/review tasks carry a template shell ("Independently review the
 * work done for the following task. ## Task <real task> ## Generator's
 * changes …"), so a plain prefix clip would show template boilerplate
 * instead of the actual task. This strips the template, keeps the `## Task`
 * section, collapses whitespace, and clips to `max` chars.
 */
export function summarizeAgentTask(task: string, max = 44): string {
  const trimmed = task.trim();
  if (!trimmed) return trimmed;
  let core = trimmed;
  const marker = "## Task";
  const markerIdx = trimmed.indexOf(marker);
  if (markerIdx >= 0) {
    const after = trimmed.slice(markerIdx + marker.length).trim();
    const nextSection = after.search(/\n##\s/);
    core = (nextSection >= 0 ? after.slice(0, nextSection) : after).trim();
  }
  return clip(core.replace(/\s+/g, " ").trim(), max);
}

/**
 * The human-readable target for a tool call — what the card line points at:
 * file name (edit/read/write), pattern (grep/glob), command (bash), URL,
 * query, or the subagent task.
 */
export function extractTarget(name: string, args: Record<string, unknown>): string | null {
  // Dynamic MCP tools (`mcp__server__tool`) — surface the tool segment.
  if (name.startsWith("mcp__")) {
    const parts = name.split("__");
    return clip(parts[parts.length - 1] ?? name, 24);
  }
  switch (name) {
    case "read_file":
    case "write_file":
    case "edit_file":
    case "search_replace":
    case "list_dir": {
      const path = get(args, "path");
      return path ? shortName(path) : null;
    }
    case "grep":
    case "glob": {
      return get(args, "pattern") ?? null;
    }
    case "bash":
    case "run_command": {
      const cmd = get(args, "command");
      return cmd ? clip(cmd, 48) : null;
    }
    case "web_fetch":
    case "web_fetch_depwork": {
      const url = get(args, "url");
      return url ? clip(shortName(url), 32) : null;
    }
    case "web_search": {
      const query = get(args, "query");
      return query ? clip(query, 24) : null;
    }
    case "memory_search":
    case "memory_store": {
      const query = get(args, "query");
      return query ? clip(query, 24) : null;
    }
    case "agent": {
      const task = get(args, "task");
      return task ? summarizeAgentTask(task) : null;
    }
    // ── Depwork (document / media processing) tools ─────────────
    case "doc_read":
    case "table_process":
    case "media_convert":
    case "ocr_image": {
      const path = get(args, "input") ?? get(args, "path");
      return path ? shortName(path) : null;
    }
    case "docx_generate":
    case "ppt_generate":
    case "chart_generate": {
      const out = get(args, "output") ?? get(args, "path");
      return out ? shortName(out) : null;
    }
    case "batch_file": {
      const dir = get(args, "dir") ?? get(args, "input");
      return dir ? shortName(dir) : null;
    }
    case "web_open": {
      const url = get(args, "url");
      return url ? clip(shortName(url), 32) : null;
    }
    case "media_probe": {
      const path = get(args, "input") ?? get(args, "path");
      return path ? shortName(path) : null;
    }
    case "ui_automate": {
      const action = get(args, "action") ?? get(args, "description");
      return action ? clip(action, 24) : null;
    }
    // ── Family-mapped tools (no dedicated verb, shared family narrative) ──
    case "apply_patch": {
      const path = get(args, "path");
      return path ? shortName(path) : null;
    }
    case "search_symbols": {
      const query = get(args, "query");
      return query ? clip(query, 24) : null;
    }
    case "file_dependencies":
    case "lsp": {
      const path = get(args, "path") ?? get(args, "file");
      return path ? shortName(path) : null;
    }
    case "kill_task": {
      const id = get(args, "task_id") ?? get(args, "id");
      return id ? clip(id, 24) : null;
    }
    case "update_goal": {
      const goal = get(args, "goal") ?? get(args, "description");
      return goal ? clip(goal, 32) : null;
    }
    case "use_tool": {
      const tool = get(args, "tool");
      return tool ? clip(tool, 24) : null;
    }
    case "visual_describe": {
      const path = get(args, "path") ?? get(args, "image");
      return path ? shortName(path) : null;
    }
    case "browser_control": {
      const action = get(args, "action");
      return action ? clip(action, 24) : null;
    }
    case "card_generate": {
      const out = get(args, "output") ?? get(args, "path");
      return out ? shortName(out) : null;
    }
    case "docx_edit":
    case "doc_consistency":
    case "live_doc_write": {
      const path = get(args, "path") ?? get(args, "file") ?? get(args, "input");
      return path ? shortName(path) : null;
    }
    case "docx_search": {
      const query = get(args, "query") ?? get(args, "keyword");
      return query ? clip(query, 24) : null;
    }
    case "office_automate": {
      const action = get(args, "action") ?? get(args, "operation");
      return action ? clip(action, 24) : null;
    }
    case "pdf_generate":
    case "pdf_tools": {
      const out = get(args, "output") ?? get(args, "path") ?? get(args, "file");
      return out ? shortName(out) : null;
    }
    case "research_search": {
      const query = get(args, "query") ?? get(args, "topic");
      return query ? clip(query, 24) : null;
    }
    case "research_save":
    case "research_remove":
    case "research_clip":
    case "research_open_access": {
      const id = get(args, "id") ?? get(args, "item_id");
      return id ? clip(id, 24) : null;
    }
    case "research_export":
    case "research_report": {
      const out = get(args, "output") ?? get(args, "path") ?? get(args, "format");
      return out ? clip(out, 24) : null;
    }
    case "citation_link": {
      const out = get(args, "path") ?? get(args, "format");
      return out ? clip(out, 24) : null;
    }
    case "store_research_map":
    case "store_research_xhs":
    case "store_research_geo": {
      const query = get(args, "query") ?? get(args, "keyword") ?? get(args, "city");
      return query ? clip(query, 24) : null;
    }
    case "xlsx_generate": {
      const out = get(args, "output") ?? get(args, "path");
      return out ? shortName(out) : null;
    }
    case "dev_browser_open": {
      const target = get(args, "url") ?? get(args, "path");
      return target ? clip(shortName(target), 32) : null;
    }
    case "content_pack": {
      const dir = get(args, "output_dir");
      return dir ? shortName(dir) : null;
    }
    default:
      return null;
  }
}

// ── Byte formatting ───────────────────────────────────────────

/** Human-readable byte size — 12345678 → "11.8 MB". */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  do {
    value /= 1024;
    unit++;
  } while (value >= 1024 && unit < units.length - 1);
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

// ── Elapsed time ──────────────────────────────────────────────

/** Format ms → "mm:ss" (or "h:mm:ss" past an hour). */
export function formatElapsedMs(elapsedMs: number): string {
  const secs = Math.max(0, Math.floor(elapsedMs / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}
