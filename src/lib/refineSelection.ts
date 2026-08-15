/**
 * 定点修改（selection refine）辅助 — 把用户在助手消息里选中的文本转换成
 * 待发送的输入草稿：引用选中内容，提示模型只改这一段，结尾留空行让用户
 * 补充具体修改要求（对标“指哪改哪”的交互）。
 */

import i18n from "@/i18n";

/** 选中的文本超过该长度时截断，避免把整条长消息塞进输入框。 */
export const MAX_REFINE_SELECTION_CHARS = 8000;

/** 构造输入框草稿。空选择返回空串（调用方不动作）。 */
export function buildRefineDraft(selected: string): string {
  const trimmed = selected.trim();
  if (trimmed.length === 0) return "";
  const capped =
    trimmed.length > MAX_REFINE_SELECTION_CHARS
      ? `${trimmed.slice(0, MAX_REFINE_SELECTION_CHARS)}${i18n.t("chat.refineTruncated")}`
      : trimmed;
  return i18n.t("chat.refinePrompt", { content: capped });
}

/** 把焦点还给输入区并把光标移到末尾——定点修改点击后用户直接补充要求。 */
export function focusChatTextarea(): void {
  const textarea = document.querySelector<HTMLTextAreaElement>(
    "textarea[data-refine-focus]",
  );
  if (!textarea) return;
  textarea.focus();
  const end = textarea.value.length;
  textarea.setSelectionRange(end, end);
}
