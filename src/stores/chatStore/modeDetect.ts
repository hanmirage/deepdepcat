/**
 * Chat store — split by concern (types / prefs / mode detection / stream state).
 */

import type { InteractionMode } from "@/types";
import type { ChatWorkMode } from "./types";

const PLAN_KEYWORDS = [
  /看看/, /分析/, /探索/, /了解/, /review/, /检查/, /梳理/, /过一遍/, /查/,
] as readonly RegExp[];

const USER_CONFIRM_WORDS = [
  /^(是|OK|好|行|可以|没问题|确认|执行|按这个|开始|干|搞|整)\s*$/i,
] as readonly RegExp[];

// Depwork keyword tables — academic/office document work. With the unified
// 3-mode set (只读 / 接受编辑 / 完全放行), analysis and modification intents
// default to 接受编辑 (edits auto-approved, other tools prompt), and
// free-form chat defaults to 只读 (read_only). The permission layer, not the
// mode label, enforces execution.
const DEPWORK_ANALYSIS_KEYWORDS = [
  /分析/, /总结/, /摘要/, /提炼/, /提取/, /比较/, /对比/, /评价/, /评估/,
  /analyze/i, /summarize/i, /extract/i, /compare/i, /evaluate/i, /review/i,
  // 与后端意图层（intent.rs Research）对齐：调研/文献/资料类 → 更改前提问
  /调研/, /研究/, /文献/, /竞品/, /市场分析/, /查资料/, /搜集/, /资料整理/,
  /research/i, /investigate/i, /literature/i,
] as readonly RegExp[];

const DEPWORK_CONFIRM_KEYWORDS = [
  /修改/, /编辑/, /改写/, /润色/, /翻译/, /生成/, /创建/, /写/,
  /edit/i, /modify/i, /rewrite/i, /polish/i, /translate/i, /generate/i, /create/i,
  // 与后端意图层（intent.rs ContentCreation）对齐：创作/内容类 → 更改前提问
  /文案/, /脚本/, /分镜/, /封面/, /海报/, /小红书/, /公众号/, /抖音/, /视频/,
  /创作/, /PPT/, /幻灯片/, /推文/, /script/i, /copywriting/i, /deck/i,
] as readonly RegExp[];

/** Detect the interaction mode from user message text, per work mode. */
export function detectMode(mode: ChatWorkMode, text: string): InteractionMode {
  if (mode === "depwork") {
    if (
      DEPWORK_CONFIRM_KEYWORDS.some((re) => re.test(text)) ||
      DEPWORK_ANALYSIS_KEYWORDS.some((re) => re.test(text))
    ) {
      return "accept_edits"; // document work — edits auto-approved, others prompt
    }
    return "read_only"; // free-form chat — read-only
  }
  // Code: read-only exploration drops into read_only; everything else stays
  // on the accept_edits default.
  if (PLAN_KEYWORDS.some((re) => re.test(text))) return "read_only";
  return "accept_edits"; // default — edits auto-approved, dangerous ops prompt
}

/** True when the user's message is a bare confirmation (read_only → accept_edits). */
export function isConfirmationReply(text: string): boolean {
  return USER_CONFIRM_WORDS.some((re) => re.test(text.trim()));
}

/** Normalize a stored/restored mode value (localStorage, session row, or
 *  backend wire string) to the current 3-mode InteractionMode. Legacy values
 *  from before the collapse ("plan"/"chat_only"/"confirm"/"auto"/"manual"/
 *  "bypass") migrate to their closest equivalent. */
export function normalizeInteractionMode(value: unknown): InteractionMode {
  switch (value) {
    case "read_only":
    case "plan":
    case "chat_only":
      return "read_only";
    case "accept_edits":
    case "confirm":
    case "manual":
    case "default":
      return "accept_edits";
    case "full_access":
    case "auto":
    case "bypass":
      return "full_access";
    default:
      return "accept_edits";
  }
}

// ── Subagent UI record ─────────────────────────────────────
// Live state of one spawned subagent, updated from stream events. Kept OUT
// of the message blocks — subagents are surfaced in the activity panel and
// on their linked agent tool card, not as injected text (the parent message
// only gets a one-line result summary when the subagent finishes).
