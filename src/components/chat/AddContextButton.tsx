/**
 * AddContextButton — "添加上下文" button for the chat input area.
 *
 * Opens a dropdown to add context sources:
 * - 添加文件 (add files) — native multi-file picker
 * - 添加 URL (add web page) — inline prompt
 * - 添加截图 (add screenshot) — full-screen capture, attached as an image
 *   chip (the agent reads it via visual_describe / OCR)
 *
 * (添加文件夹 was removed — folders come from the workspace picker, not
 * the input bar.)
 */

import { Plus, FilePlus, Globe, Image as ImageIcon, Info, X, Check } from "lucide-react";
import { logWarn } from "@/lib/logger";
import { useTranslation } from "react-i18next";
import { useState, useEffect, useRef } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { isTauri } from "@/lib/tauri";
import { compressImageToDataUrl } from "@/lib/image";
import type { AppMode } from "@/config/constants";

export interface AddContextButtonProps {
  /** Which chat store to bind. "code" → chatStore, "depwork" → depworkChatStore. */
  mode?: AppMode;
}

export function AddContextButton({ mode = "code" }: AddContextButtonProps) {
  const { t } = useTranslation();
  const isDepwork = mode === "depwork";
  const chatStore = isDepwork ? useDepworkChatStore : useChatStore;
  const addFileContext = useStore(chatStore, (s) => s.addFileContext);
  const addUrlContext = useStore(chatStore, (s) => s.addUrlContext);
  const addContextChip = useStore(chatStore, (s) => s.addContextChip);

  const [urlMode, setUrlMode] = useState(false);
  const [urlDraft, setUrlDraft] = useState("");
  const urlInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (urlMode) urlInputRef.current?.focus();
  }, [urlMode]);

  const submitUrl = () => {
    const url = urlDraft.trim();
    if (url) addUrlContext(url);
    setUrlDraft("");
    setUrlMode(false);
  };

  // "添加图片" — HTML5 file input (works in WebView and browser dev mode):
  // the picked picture is compressed to a data URL and attached as a chip —
  // no filesystem path involved, the backend transcribes it via the vision
  // model. Screenshots are pasted via Ctrl+V (see ChatInput handlePaste) —
  // no separate screenshot button, avoiding duplicate image channels.
  const imageInputRef = useRef<HTMLInputElement>(null);
  const handleAddImage = () => {
    imageInputRef.current?.click();
  };

  const handleImageFile = (file: File | null) => {
    if (!file) return;
    void compressImageToDataUrl(file)
      .then((dataUrl) => {
        addContextChip({
          id: `image-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          type: "file",
          name: file.name?.trim() ? file.name : t("chat.pickImageFallbackName"),
          path: dataUrl.slice(0, 40),
          dataUrl,
        });
      })
      .catch((err) => {
        logWarn("AddContextButton", "Failed to read picked image:", err);
      });
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-7 w-7 shrink-0 rounded-full border border-border/60 bg-muted/20 px-0 hover:bg-muted/40"
          aria-label={t("chat.addContext")}
        >
          <Plus className="h-3.5 w-3.5 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-48">
        <DropdownMenuLabel>{t("chat.addContext")}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {!isTauri && (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="flex items-center gap-1.5 px-2 py-1 text-[10px] text-muted-foreground">
                <Info className="h-3 w-3" />
                {t("chat.inputBrowserDevMode", { defaultValue: "文件选择仅在 Tauri 桌面端可用" })}
              </span>
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-48 text-[11px]">
              {t("chat.inputBrowserDevModeDesc", { defaultValue: "浏览器开发模式下文件 API 不可用。启动 npm run tauri dev 以使用完整功能。" })}
            </TooltipContent>
          </Tooltip>
        )}
        <DropdownMenuItem
          className="text-xs"
          onClick={addFileContext}
          disabled={!isTauri}
        >
          <FilePlus className="mr-2 h-3.5 w-3.5" />
          {t("chat.addFile")}
        </DropdownMenuItem>
        <DropdownMenuItem
          className="text-xs"
          onSelect={(e) => {
            e.preventDefault();
            setUrlMode(true);
          }}
        >
          <Globe className="mr-2 h-3.5 w-3.5" />
          {t("chat.addUrl")}
        </DropdownMenuItem>
        {urlMode && (
          <div
            className="flex items-center gap-1.5 px-2 pb-2"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                submitUrl();
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setUrlDraft("");
                setUrlMode(false);
              }
            }}
          >
            <input
              ref={urlInputRef}
              value={urlDraft}
              onChange={(e) => setUrlDraft(e.target.value)}
              placeholder={t("chat.inputUrlPrompt")}
              className="h-7 min-w-0 flex-1 rounded-md border border-border bg-background px-2 text-xs outline-none placeholder:text-muted-foreground/50 focus:border-primary/60 focus:ring-1 focus:ring-primary/30"
            />
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 shrink-0 rounded-md"
              aria-label={t("common.confirm", { defaultValue: "确认" })}
              onClick={submitUrl}
            >
              <Check className="h-3.5 w-3.5 text-emerald-500" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 shrink-0 rounded-md"
              aria-label={t("common.cancel")}
              onClick={() => {
                setUrlDraft("");
                setUrlMode(false);
              }}
            >
              <X className="h-3.5 w-3.5 text-muted-foreground" />
            </Button>
          </div>
        )}
        <DropdownMenuItem
          className="text-xs"
          onClick={() => void handleAddImage()}
        >
          <ImageIcon className="mr-2 h-3.5 w-3.5" />
          {t("chat.addImage")}
        </DropdownMenuItem>
      </DropdownMenuContent>
      <input
        ref={imageInputRef}
        type="file"
        accept="image/png,image/jpeg,image/webp,image/gif,image/bmp,image/tiff,image/x-icon"
        className="hidden"
        onChange={(e) => {
          handleImageFile(e.target.files?.[0] ?? null);
          e.target.value = "";
        }}
      />
    </DropdownMenu>
  );
}
