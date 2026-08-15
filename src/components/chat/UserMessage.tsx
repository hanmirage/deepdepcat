/**
 * UserMessage — 用户消息气泡（右对齐，极淡主色底）
 *
 * 设计要点：
 * - 右侧气泡（用户产品决策：用户发送消息在右边）
 * - 极淡主色底（primary/5）与助手消息区分
 * - 悬停显示时间戳
 * - 支持上下文 Chips 展示
 */

import { memo, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder, Globe, Copy, Check, Undo2, Pencil } from "lucide-react";
import { cn } from "@/lib/utils";
import type { ContextChip } from "@/types";
import { formatTime } from "@/lib/utils";
import { splitFencedCode } from "@/lib/userMessageBlocks";
import { CodeBlock } from "@/components/chat/CodeBlock";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { useAppStore } from "@/stores/appStore";
import { focusChatTextarea } from "@/lib/refineSelection";

const CHIP_ICON: Record<ContextChip["type"], typeof FileText> = {
  file: FileText,
  folder: Folder,
  url: Globe,
  paper: FileText,
};

interface UserMessageProps {
  /** Message id — the component subscribes to its store object directly,
   *  so unchanged messages never re-render during streaming flushes. */
  messageId: string;
}

export const UserMessage = memo(
  function UserMessage({ messageId }: UserMessageProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [showTimestamp, setShowTimestamp] = useState(false);
  const deleteMessage = useChatStore((s) => s.deleteMessage);
  const depworkDeleteMessage = useDepworkChatStore((s) => s.deleteMessage);
  const appMode = useAppStore((s) => s.mode);
  // Same structural-cast story as AssistantMessage: depwork messages must be
  // recalled from the depwork store or the action silently no-ops.
  const recallMessage = appMode === "depwork" ? depworkDeleteMessage : deleteMessage;
  // Message-level subscription: both stores are subscribed, the active
  // mode's array drives the render. Unchanged message objects keep their
  // reference, so zustand's Object.is check skips re-renders entirely.
  const chatStore = appMode === "depwork" ? useDepworkChatStore : useChatStore;
  const message = useStore(chatStore, (s) => s.messages.find((m) => m.id === messageId));

  if (!message) return null;

  // 提取文本内容
  const textContent = message.blocks
    .filter((b) => b.type === "text")
    .map((b) => b.content)
    .join("\n");

  const handleCopy = async () => {
    await navigator.clipboard.writeText(textContent);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  // Edit & resend: truncate the conversation AT this message (the store's
  // deleteMessage persists the truncation on the backend), then restore the
  // text into the input for review. The user presses Enter to resend.
  const handleEdit = async () => {
    await recallMessage(message.id);
    const store = appMode === "depwork" ? useDepworkChatStore : useChatStore;
    store.getState().setInputText(textContent);
    focusChatTextarea();
  };

  return (
    <div
      className="group flex w-full flex-col gap-1"
      onMouseEnter={() => setShowTimestamp(true)}
      onMouseLeave={() => setShowTimestamp(false)}
    >
      {/* 上下文 Chips */}
      {message.contextChips && message.contextChips.length > 0 && (
        <div className="mb-1 flex flex-wrap justify-end gap-1.5">
          {message.contextChips.map((chip) => {
            const Icon = CHIP_ICON[chip.type];
            return (
              <span
                key={chip.id}
                className="flex items-center gap-1 rounded-md bg-primary/10 px-2 py-0.5 text-[11px] text-primary/80"
                title={chip.dataUrl ? chip.name : chip.path}
              >
                <Icon className="h-3 w-3" />
                <span className="max-w-[160px] truncate">{chip.name}</span>
              </span>
            );
          })}
        </div>
      )}

      {/* 消息行 — 气泡右对齐；操作按钮在气泡左侧（悬停显示） */}
      <div className="flex w-full items-start justify-end gap-2">
        {/* 操作按钮（悬停显示，气泡左侧） */}
        <span className="flex shrink-0 items-center gap-0.5 pt-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
          <button
            onClick={() => void handleEdit()}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title={t("chat.editHint", { defaultValue: "编辑并重发" })}
            aria-label={t("chat.editHint", { defaultValue: "编辑并重发" })}
          >
            <Pencil className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={() => void recallMessage(message.id)}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-destructive"
            title={t("chat.recall", { defaultValue: "撤回" })}
            aria-label={t("chat.recall", { defaultValue: "撤回" })}
          >
            <Undo2 className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={handleCopy}
            className={cn(
              "rounded p-1 transition-colors",
              copied ? "text-green-500" : "text-muted-foreground hover:bg-muted hover:text-foreground",
            )}
            title={copied ? t("chat.copied") : t("chat.copy")}
            aria-label={copied ? t("chat.copied") : t("chat.copy")}
          >
            {copied ? (
              <Check className="h-3.5 w-3.5" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
          </button>
          {showTimestamp && (
            <span className="ml-1 text-[10px] text-muted-foreground">
              {formatTime(message.timestamp)}
            </span>
          )}
        </span>
        <div
          className={cn(
            "min-w-0 max-w-[85%] rounded-lg bg-primary/5 px-4 py-2.5 break-words",
            "text-foreground/90",
          )}
        >
          {/* 内容 — fenced code blocks render as real code blocks; the rest
              stays plain text (the user's wording is never reinterpreted
              as markdown). */}
          {splitFencedCode(textContent).map((part, i) =>
            part.kind === "code" ? (
              <CodeBlock key={i} code={part.text} language={part.lang} />
            ) : (
              <div key={i} className="whitespace-pre-wrap text-sm leading-relaxed">
                {part.text}
              </div>
            ),
          )}
        </div>
      </div>
    </div>
  );
  },
  // User messages are immutable once sent — id compare is enough
  // (the component re-renders only when its store object changes).
  (prev, next) => prev.messageId === next.messageId,
);
