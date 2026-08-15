/**
 * FileDropOverlay — full-window drag & drop surface (VS Code / Figma style).
 *
 * The Tauri window emits a native drag-drop event for ANY position on the
 * window. This overlay is the single handler: dragging files over the app
 * shows a full-window dashed veil ("release to add files"), and the drop
 * turns every path into a context chip of the active chat store.
 *
 * The chat input and drop zone no longer subscribe to drag events
 * themselves — one handler, no duplicate chips.
 */

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { UploadCloud } from "lucide-react";
import { onFileDrop, type FileDragEvent } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { AppMode } from "@/config/constants";

export interface FileDropOverlayProps {
  /** Which chat store the dropped files land in. */
  mode: AppMode;
}

export function FileDropOverlay({ mode }: FileDropOverlayProps) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  const chatStore = mode === "depwork" ? useDepworkChatStore : useChatStore;
  const addContextChip = useStore(chatStore, (s) => s.addContextChip);

  const subscribeFileDrop = useCallback(
    (handler: (e: FileDragEvent) => void) => onFileDrop(handler),
    [],
  );
  useTauriEvent(subscribeFileDrop, (e) => {
    if (e.type === "over") {
      setVisible(true);
      return;
    }
    setVisible(false);
    if (e.type === "drop") {
      const existing = useChatStore.getState().contextChips.map((c) => c.path)
        .concat(useDepworkChatStore.getState().contextChips.map((c) => c.path));
      const seen = new Set(existing);
      for (const p of e.paths) {
        // Deduplicate: dropping the same folder/file twice must not
        // produce duplicate context chips.
        if (seen.has(p)) continue;
        seen.add(p);
        const name = p.split(/[\\/]/).pop() ?? p;
        addContextChip({
          id: `file-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          type: "file",
          name,
          path: p,
        });
      }
    }
  });

  if (!visible) return null;

  return (
    <div
      className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center"
      aria-hidden="true"
    >
      <div className="flex h-[calc(100%-48px)] w-[calc(100%-48px)] flex-col items-center justify-center gap-3 rounded-2xl border-2 border-dashed border-primary/50 bg-primary/[0.06]">
        <span className="flex h-12 w-12 items-center justify-center rounded-full bg-primary/10">
          <UploadCloud className="h-6 w-6 text-primary" />
        </span>
        <p className="text-sm font-medium text-foreground">
          {t("chat.dropToAdd", { defaultValue: "松开以添加文件" })}
        </p>
        <p className="text-[11px] text-muted-foreground">
          {t("chat.dropToAddHint", { defaultValue: "txt / md / 图片 / 文档——将作为上下文发送给智能体" })}
        </p>
      </div>
    </div>
  );
}
