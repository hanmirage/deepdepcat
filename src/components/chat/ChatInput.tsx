/**
 * ChatInput — message input box (modern design).
 *
 * Layout:
 * ┌──────────────────────────────────────────┐
 * │ [📄 file.tsx ✕] [📁 src/ ✕]  ← context chips │
 * ├──────────────────────────────────────────┤
 * │ ⏳ 已排队… ✕                  ← queued chip │
 * ├──────────────────────────────────────────┤
 * │  Textarea (auto-expanding, inline / hint) │
 * ├──────────────────────────────────────────┤
 * │ [🛡模式▾] [📎]      [模型▾] [深度▾] [◌] [↑/■]│  ← toolbar
 * └──────────────────────────────────────────┘
 *
 * Left  = actions (mode label + add-context icon)
 * Right = model settings (model short name + reasoning level + usage ring
 *         + send/stop). While streaming, the STOP button stays visible at
 *         all times; typed text adds a queue/interrupt menu beside it.
 *
 * Props:
 * - compact: when true, tighter padding (used at bottom of active chat)
 * - embedded: when true, no outer border/padding (used inside UnifiedWelcome's glass card)
 * - mode: which store to bind ("code" → chatStore, "depwork" → depworkChatStore)
 *
 * Store binding uses zustand's useStore against a mode-selected store — a
 * single unconditional hook call (no conditional hooks, safe under mode
 * switches), with selectors typed against the union of both states.
 */

import { useRef, useEffect, useCallback, useState, useMemo, useLayoutEffect } from "react";
import { logWarn } from "@/lib/logger";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { ArrowUp, Bot, ClipboardList, Settings2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ModelSelector } from "@/components/chat/ModelSelector";
import { AddContextButton } from "@/components/chat/AddContextButton";
import { AgentBehaviorMenu } from "@/components/chat/AgentBehaviorMenu";
import { ReasoningSelector, type ReasoningMode } from "@/components/chat/ReasoningSelector";
import { ContextUsageRing } from "@/components/chat/ContextUsageRing";
import { ContextChips } from "@/components/chat/ContextChips";
import { SlashCommandPanel, DEFAULT_COMMANDS, type SlashCommand } from "@/components/chat/SlashCommandPanel";
import { AGENT_MODE_OPTIONS } from "@/config/agentModes";
import { useChatStore, type ChatState } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useIsSessionInPlanMode } from "@/stores/planStore";
import { useSettingsStore, buildModelsFromProviders } from "@/stores/settingsStore";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";
import { mcpApi, agentApi, onFileDrop, type FileDragEvent } from "@/lib/tauri";
import { compressImageToDataUrl, fileToDataUrl, isImageUrl, extractFirstUrl } from "@/lib/image";
import { Terminal } from "lucide-react";
import type { AppMode } from "@/config/constants";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { QueuedNotice, NoModelNotice, StreamControls } from "@/components/chat/ChatInputParts";

interface ChatInputProps {
  compact?: boolean;
  embedded?: boolean;
  /** Hide the in-input setup notice (the setup card takes over, e.g. welcome page). */
  hideSetupNotice?: boolean;
  /** Which chat store to bind. "code" → chatStore, "depwork" → depworkChatStore. */
  mode?: AppMode;
}

/** Union of the two chat states — selectors read only shared fields. The
 *  #79 factory merged both stores onto one interface, so this is just the
 *  shared type. */
type BoundChatState = ChatState;

export function ChatInput({
  compact = false,
  embedded = false,
  hideSetupNotice = false,
  mode = "code",
}: ChatInputProps) {
  const { t } = useTranslation();

  // ── Bind the correct store based on mode ─────────────────────
  // useStore is a single unconditional hook; the store argument may switch
  // at any time (it re-subscribes internally), so this is safe even if the
  // mode prop changed after mount.
  const isDepwork = mode === "depwork";
  const chatStore = isDepwork ? useDepworkChatStore : useChatStore;
  const inputText = useStore(chatStore, (s: BoundChatState) => s.inputText);
  const setInputText = useStore(chatStore, (s: BoundChatState) => s.setInputText);
  const sendMessage = useStore(chatStore, (s: BoundChatState) => s.sendMessage);
  const stopStreaming = useStore(chatStore, (s: BoundChatState) => s.stopStreaming);
  const isStreaming = useStore(chatStore, (s: BoundChatState) => s.isStreaming);
  const isPaused = useStore(chatStore, (s: BoundChatState) => s.isPaused);
  const pauseStreaming = useStore(chatStore, (s: BoundChatState) => s.pauseStreaming);
  const resumeStreaming = useStore(chatStore, (s: BoundChatState) => s.resumeStreaming);
  const queuedText = useStore(chatStore, (s: BoundChatState) => s.queuedText);
  const clearQueuedText = useStore(chatStore, (s: BoundChatState) => s.clearQueuedText);
  /** Send the queued message RIGHT NOW: move it back into the input, then
   *  interrupt-send (stop the current turn and emit it as a fresh one). If
   *  the turn already ended, the chip is stale — it just sends normally. */
  const sendQueuedNow = useCallback(() => {
    clearQueuedText();
    void sendMessage("interrupt");
  }, [clearQueuedText, sendMessage]);
  const chips = useStore(chatStore, (s: BoundChatState) => s.contextChips);
  const removeChip = useStore(chatStore, (s: BoundChatState) => s.removeContextChip);
  // Model picker reads LIVE from the settings providers — mirrors
  // Settings → Model Providers exactly (add/fetch/remove there shows up
  // here instantly, nothing hardcoded). Selection state stays in the store.
  const settingsProviders = useSettingsStore((s) => s.providers);
  const models = useMemo(() => buildModelsFromProviders(settingsProviders), [settingsProviders]);
  const selectedModel = useStore(chatStore, (s: BoundChatState) => s.selectedModel);
  const setSelectedModel = useStore(chatStore, (s: BoundChatState) => s.setSelectedModel);
  // Reasoning effort (DeepSeek) — the #79 merge unified both stores on one
  // interface, so reasoningMode is always present (the input-bar selector
  // is still code-only; depwork keeps the settings-driven value).
  const reasoningMode = useStore(chatStore, (s: BoundChatState) => s.reasoningMode);
  const setReasoningMode = useStore(chatStore, (s: BoundChatState) => s.setReasoningMode);
  const currentSessionId = useStore(chatStore, (s: BoundChatState) => s.currentSessionId);
  const isPlanMode = useIsSessionInPlanMode(currentSessionId);
  const openSettings = useAppStore((s) => s.openSettings);
  const addContextChip = useStore(chatStore, (s: BoundChatState) => s.addContextChip);
  const addUrlContext = useStore(chatStore, (s: BoundChatState) => s.addUrlContext);
  const setAgentMode = useStore(chatStore, (s: BoundChatState) => s.setAgentMode);
  const setSelectedAgent = useStore(chatStore, (s: BoundChatState) => s.setSelectedAgent);

  // Drag & drop is handled window-wide by FileDropOverlay (single handler,
  // real paths, full-window veil). The input bar itself only shows a mild
  // highlight while files are dragged over the window.
  const [isDragOver, setIsDragOver] = useState(false);
  // Transient inline error — e.g. a pasted image that failed to decode.
  // Shown above the textarea, auto-cleared, so the failure isn't silent.
  const [inlineError, setInlineError] = useState<string | null>(null);
  const inlineErrorTimer = useRef<number | null>(null);
  const showInlineError = useCallback((msg: string) => {
    setInlineError(msg);
    if (inlineErrorTimer.current) window.clearTimeout(inlineErrorTimer.current);
    inlineErrorTimer.current = window.setTimeout(() => setInlineError(null), 4000);
  }, []);
  useEffect(() => () => {
    if (inlineErrorTimer.current) window.clearTimeout(inlineErrorTimer.current);
  }, []);

  const subscribeFileDrop = useCallback(
    (handler: (e: FileDragEvent) => void) => onFileDrop(handler),
    [],
  );
  useTauriEvent(subscribeFileDrop, (e) => {
    setIsDragOver(e.type === "over");
  });

  // Reasoning mode change — the EnergySlider inside ReasoningSelector fires
  // its own WebGL burst; we just persist the chosen mode.
  const handleSelectReasoning = useCallback(
    (m: ReasoningMode) => {
      if (setReasoningMode) setReasoningMode(m);
    },
    [setReasoningMode],
  );

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // No-model attention pulse: pressing Enter without a configured model
  // flashes the setup banner instead of abruptly opening settings — the
  // user keeps their place while the fix is called out.
  const [setupAttention, setSetupAttention] = useState(false);
  useEffect(() => {
    if (!setupAttention) return;
    const timer = setTimeout(() => setSetupAttention(false), 1600);
    return () => clearTimeout(timer);
  }, [setupAttention]);

  // Keep the selection in sync with the live model list: when the selected
  // model disappears (deleted in Settings), fall back to the first available.
  useEffect(() => {
    if (!selectedModel || models.length === 0) return;
    if (models.some((m) => m.id === selectedModel.id)) return;
    setSelectedModel(models[0]);
  }, [models, selectedModel, setSelectedModel]);

  // Auto-resize textarea — on input change AND on window width change
  // (a narrower window re-wraps long lines, changing the needed height).
  const resizeTextarea = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  }, []);
  useEffect(() => {
    resizeTextarea();
    window.addEventListener("resize", resizeTextarea);
    return () => window.removeEventListener("resize", resizeTextarea);
  }, [resizeTextarea]);
  // Layout effect: the height must be set BEFORE paint, or a keystroke that
  // adds a line shows the old height for one frame (flicker while typing).
  useLayoutEffect(() => {
    resizeTextarea();
  }, [inputText, resizeTextarea]);

  // Slash commands — select fills the input with the command text
  const slashOpen = useState(false);
  const showSlashPanel = slashOpen[0] && inputText.startsWith("/") && !inputText.includes(" ");
  const setSlashOpen = slashOpen[1];

  // MCP prompt commands — lazily loaded from connected servers.
  const [mcpCommands, setMcpCommands] = useState<SlashCommand[]>([]);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const commands: SlashCommand[] = [];
      try {
        const servers = await mcpApi.listConnected();
        for (const server of servers) {
          const prompts = await mcpApi.listPrompts(server);
          for (const prompt of prompts) {
            commands.push({
              id: `mcp:${server}:${prompt.name}`,
              label: `/mcp:${server}:${prompt.name}`,
              description: `MCP ${server}: ${prompt.description || prompt.name}`,
              icon: Terminal,
              category: "other",
              action: async () => {
                try {
                  const result = (await mcpApi.getPrompt(server, prompt.name, {})) as
                    | { messages?: { role?: string; content?: { text?: string }[] }[] }
                    | undefined;
                  const text = result?.messages
                    ?.map((m) =>
                      m.content
                        ?.map((c) => c.text ?? "")
                        .filter(Boolean)
                        .join("\n"),
                    )
                    .filter(Boolean)
                    .join("\n\n");
                  setInputText(text || `/mcp:${server}:${prompt.name} `);
                } catch {
                  setInputText(`/mcp:${server}:${prompt.name} `);
                }
                setSlashOpen(false);
              },
            });
          }
        }
      } catch {
        // A failing server shouldn't drop MCP commands from the others.
      }
      if (!cancelled) setMcpCommands(commands);
    })();
    return () => {
      cancelled = true;
    };
  }, [setInputText, setSlashOpen]);

  // Persona slash commands — one per agent definition (+ /default), loaded
  // from the backend for the current work mode.
  const [personaCommands, setPersonaCommands] = useState<SlashCommand[]>([]);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const commands: SlashCommand[] = [
        {
          id: "persona-default",
          label: "/default",
          description: "默认人格",
          descriptionKey: "chat.agentPersonaDefault",
          icon: Bot,
          category: "agent",
          fillInput: false,
          action: () => setSelectedAgent(""),
        },
      ];
      try {
        // Exclude the "default" persona — the /default command above already
        // covers it, and its id would otherwise collide with "persona-default".
        const defs = (await agentApi.listDefinitions(mode)).filter(
          (d) => d.id !== "default",
        );
        for (const def of defs) {
          commands.push({
            id: `persona-${def.id}`,
            label: `/${def.id}`,
            description: def.description || def.name,
            icon: Bot,
            category: "agent",
            fillInput: false,
            action: () => setSelectedAgent(def.name),
          });
        }
      } catch {
        // Keep the /default command even if loading fails.
      }
      if (!cancelled) setPersonaCommands(commands);
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, setSelectedAgent]);

  // Execution-strategy slash commands (code only) — switch the agent's
  // "how to organize work" mode without filling the input. Depwork's
  // permission layer governs execution, so these are code-only.
  const agentModeCommands = useMemo(
    () =>
      isDepwork
        ? []
        : AGENT_MODE_OPTIONS.map((opt) => ({
            id: `agent-mode-${opt.id}`,
            label: `/${opt.id}`,
            description: opt.id,
            descriptionKey: opt.description,
            icon: opt.icon,
            category: "agent" as const,
            fillInput: false,
            action: () => setAgentMode(opt.id),
          })),
    [isDepwork, setAgentMode],
  );

  const allCommands = useCallback(
    () => [...DEFAULT_COMMANDS, ...mcpCommands, ...agentModeCommands, ...personaCommands],
    [mcpCommands, agentModeCommands, personaCommands],
  );

  const handleSelectSlash = useCallback(
    (cmd: SlashCommand) => {
      // Mode/persona switches (fillInput false) clear the leftover "/…"
      // text instead of inserting the command label.
      setInputText(cmd.fillInput !== false ? `${cmd.label} ` : "");
      setSlashOpen(false);
      cmd.action();
    },
    [setInputText, setSlashOpen],
  );

  /** True when the slash panel has at least one command matching the query. */
  const slashHasMatch = useCallback(() => {
    const q = inputText.slice(1).toLowerCase();
    return allCommands().some(
      (c) =>
        c.label.toLowerCase().includes(q) || c.description.toLowerCase().includes(q),
    );
  }, [allCommands, inputText]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // IME composition (pinyin/cangjie) — Enter commits the candidate,
    // must not send the message.
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Enter" && !e.shiftKey) {      e.preventDefault();
      // While the slash panel is open, Enter is handled by the panel — the
      // window-level listener selects the highlighted command. But when the
      // query matches nothing, close the panel and send the text as-is so
      // "/"-prefixed messages stay sendable.
      if (showSlashPanel && slashHasMatch()) return;
      if (showSlashPanel) setSlashOpen(false);
      // No model configured — don't silently drop the message (the send
      // button is disabled too; this blocks the Enter path). On the welcome
      // page the setup card is the primary call-to-action, so take the user
      // straight to settings; in a real chat, pulse the notice instead of
      // yanking focus away.
      if (!selectedModel) {
        if (embedded || hideSetupNotice) {
          openSettings("models");
        } else {
          setSetupAttention(true);
        }
        return;
      }
      sendMessage();
    }
    // "/" opens the slash panel
    if (e.key === "/" && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
      setSlashOpen(true);
    }
  };

  const hasText = inputText.trim().length > 0;
  // Show the model selector only when there are actually models to pick from.
  // (Previously gated on provider count, which could be non-zero with empty model lists.)
  const hasModels = models.length > 0;

  // Ctrl+V pastes an image (e.g. a copied screenshot): read the picture
  // directly from the WebView clipboard, compress it to a data URL and
  // attach it as a context chip — no PowerShell, no filesystem, no path
  // the model could fail to resolve. The backend transcribes it to text.
  // If the clipboard holds no image but the text IS an image URL, attach
  // it as a URL chip instead — the backend downloads it into the vision
  // pipeline.
  const handlePaste = (e: React.ClipboardEvent) => {
    const file = Array.from(e.clipboardData?.items ?? [])
      .filter((item) => item.type.startsWith("image/"))
      .map((item) => item.getAsFile())
      .find((f): f is File => f !== null);
    if (!file) {
      const text = e.clipboardData?.getData("text/plain") ?? "";
      // Only hijack a paste whose ENTIRE content is a single image URL —
      // never a sentence that happens to contain one.
      const single = text.trim();
      if (/^https?:\/\/\S+$/i.test(single) && isImageUrl(single)) {
        e.preventDefault();
        addUrlContext(single);
      }
      return;
    }
    e.preventDefault();
    const name = file.name?.trim() ? file.name : t("chat.pasteImageFallbackName");
    void compressImageToDataUrl(file)
      .catch(() => fileToDataUrl(file))
      .then((dataUrl) => {
        addContextChip({
          id: `image-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          type: "file",
          name,
          path: dataUrl.slice(0, 40),
          dataUrl,
        });
      })
      .catch((err) => {
        logWarn("ChatInput", "Failed to read pasted image:", err);
        showInlineError(t("chat.pasteImageFailed"));
      });
  };

  // HTML5 drag & drop for URL text (dragging a link or picture out of a
  // browser). Native OS file drags are handled window-wide by
  // FileDropOverlay (real paths); this only claims drops that carry URL
  // text and no files, so the webview never navigates and image links land
  // in the vision pipeline.
  const handleDragOver = (e: React.DragEvent) => {
    const types = e.dataTransfer?.types ?? [];
    const hasUrlText =
      types.includes("text/uri-list") ||
      (types.includes("text/plain") && !types.includes("Files"));
    if (!hasUrlText) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    setIsDragOver(true);
  };

  const handleDrop = (e: React.DragEvent) => {
    const types = e.dataTransfer?.types ?? [];
    const hasFiles = types.includes("Files");
    const uriList = e.dataTransfer?.getData("text/uri-list") ?? "";
    const url = extractFirstUrl(uriList || (e.dataTransfer?.getData("text/plain") ?? ""));
    if (!hasFiles && url) {
      e.preventDefault();
      addUrlContext(url);
    }
    setIsDragOver(false);
  };

  // Outer wrapper
  const outerClass = embedded
    ? "shrink-0"
    : cn("shrink-0 px-4", compact ? "pb-3 pt-1" : "pb-4");

  // ── Slash panel positioning ────────────────────────────────
  // Embedded mode (UnifiedWelcome glass card) has overflow-hidden, which
  // would clip a panel positioned above the input. When embedded, render
  // the panel through a portal anchored to the input's screen rect instead.
  const rootRef = useRef<HTMLDivElement>(null);
  const [panelPos, setPanelPos] = useState<{ left: number; width: number; bottom: number } | null>(null);

  useLayoutEffect(() => {
    if (!showSlashPanel || !embedded) {
      setPanelPos(null);
      return;
    }
    const measure = () => {
      const rect = rootRef.current?.getBoundingClientRect();
      if (!rect) return;
      setPanelPos({
        left: rect.left + 12,
        width: Math.min(448, Math.max(280, rect.width - 24)),
        bottom: window.innerHeight - rect.top + 4,
      });
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [showSlashPanel, embedded]);

  const slashPanel = showSlashPanel && (
    <SlashCommandPanel
      commands={allCommands()}
      query={inputText.slice(1)}
      onSelect={handleSelectSlash}
      onClose={() => setSlashOpen(false)}
    />
  );

  return (
    <div
      className={outerClass}
      data-chat-input-root
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      <div
        ref={rootRef}
        className={cn(
          "relative flex flex-col rounded-xl transition-[border-color,box-shadow,background-color] duration-200",
          embedded
            ? "bg-transparent"
            : "border border-border bg-card shadow-[var(--shadow-paper-md)]",
          !embedded && hasText && "border-primary/40 shadow-[var(--shadow-paper-lg)]",
          !embedded && isPlanMode && "border-amber-500/40 shadow-[var(--shadow-paper-lg)]",
          !embedded && "focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/20",
          !embedded && isDragOver && "border-primary ring-2 ring-primary/20 bg-primary/[0.03]",
        )}
      >
        {/* ── Context chips ───────────────────────────────────── */}
        <ContextChips chips={chips} onRemove={removeChip} />

        {/* ── Plan-mode indicator ───────────────────────────────
            The agent self-dispatched into plan mode (enter_plan_mode): show
            the read-only posture like Claude desktop so the user knows a plan
            is being drafted, not executed. */}
        {isPlanMode && (
          <div className="flex items-center gap-2 border-b border-amber-500/20 bg-amber-500/10 px-3 py-1.5 text-xs font-medium text-amber-700 dark:text-amber-400">
            <ClipboardList className="h-3.5 w-3.5 shrink-0" />
            <span>{t("chat.planModeIndicator")}</span>
          </div>
        )}

        {/* ── Queued-send notice ──────────────────────────────── */}
        {queuedText && (
          <QueuedNotice
            queuedText={queuedText}
            onSendNow={sendQueuedNow}
            onClear={clearQueuedText}
          />
        )}

        {/* ── No-model notice ─────────────────────────────────── */}
        {!hasModels && !hideSetupNotice && (
          <NoModelNotice
            attention={setupAttention}
            onConfigure={() => openSettings("models")}
          />
        )}

        {/* ── Transient inline error (pasted image decode failure etc.) ── */}
        {inlineError && (
          <p className="border-t border-destructive/20 bg-destructive/5 px-3 py-1.5 text-[11px] text-destructive">
            {inlineError}
          </p>
        )}

        {/* ── Textarea ────────────────────────────────────────── */}
        <Textarea
          ref={textareaRef}
          data-refine-focus
          value={inputText}
          onChange={(e) => setInputText(e.target.value)}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          placeholder={t("chat.inputPlaceholder")}
          // Focus on mount so a fresh session lands ready to type — but ONLY
          // when nothing else is focused (switching views / restoring a
          // session must never yank focus away from where the user was).
          autoFocus={!document.activeElement || document.activeElement === document.body}
          className={cn(
            "min-h-[48px] flex-1 resize-none border-0 bg-transparent px-3 text-sm shadow-none",
            "placeholder:text-muted-foreground/60",
            "focus-visible:ring-0 focus-visible:ring-offset-0",
            compact ? "py-2" : "py-3",
            embedded && "bg-transparent",
          )}
          rows={compact ? 1 : 2}
        />

        {/* ── Bottom toolbar ──────────────────────────────────── */}
        <div className={cn(
          "flex items-center justify-between gap-1 px-2 pb-1",
          embedded ? "pt-1" : "border-t border-border/40 pt-1",
        )}>
          {/* Left: one Agent-behavior menu (mode + strategy + persona) + Add context */}
          <div className="flex items-center gap-0.5">
            <AgentBehaviorMenu mode={mode} />
            <AddContextButton mode={mode} />
          </div>

          {/* Right: Model + Reasoning (code) + usage ring + Send/Stop.
              The stop button stays visible while streaming — typed text adds
              a queue/interrupt menu NEXT to it instead of replacing it. */}
          <div className="flex items-center gap-1.5">
            {hasModels ? (
              <ModelSelector
                models={models}
                selectedModel={selectedModel}
                onSelectModel={setSelectedModel}
              />
            ) : (
              <Button
                variant="ghost"
                size="sm"
                className="h-7 gap-1 px-2 text-xs text-primary hover:bg-primary/10 hover:text-primary"
                onClick={() => openSettings("models")}
              >
                <Settings2 className="h-3.5 w-3.5" />
                <span>{t("chat.configureModelButton", "配置模型")}</span>
              </Button>
            )}
            {!isDepwork && <ReasoningSelector value={reasoningMode} onChange={handleSelectReasoning} />}
            <ContextUsageRing sessionId={currentSessionId} mode={mode} />
            {isStreaming ? (
              <StreamControls
                hasText={hasText}
                isPaused={isPaused}
                onQueue={() => void sendMessage("queue")}
                onInterrupt={() => void sendMessage("interrupt")}
                onPauseResume={isPaused ? resumeStreaming : pauseStreaming}
                onStop={stopStreaming}
              />
            ) : (
              <Button
                size="icon"
                className={cn(
                  "h-7 w-7 shrink-0 rounded-full transition-[background-color,color,box-shadow,transform] duration-200",
                  hasText
                    ? "bg-primary text-primary-foreground shadow-md shadow-primary/30 hover:scale-105"
                    : "bg-muted text-muted-foreground",
                )}
                disabled={!hasText || !selectedModel}
                onClick={() => void sendMessage()}
                aria-label={t("common.send")}
                title={
                  !hasText
                    ? t("chat.sendDisabledNoText", "先输入消息")
                    : !selectedModel
                      ? t("chat.sendDisabledNoModel", "先选择模型")
                      : t("common.send")
                }
              >
                <ArrowUp className="h-3 w-3" />
              </Button>
            )}
          </div>
        </div>

        {/* ── Slash command panel (when input starts with "/") ── */}
        {showSlashPanel && !embedded && (
          <div className="absolute bottom-[calc(100%+4px)] left-3 z-30 w-full max-w-md">
            {slashPanel}
          </div>
        )}
      </div>
      {/* Embedded mode: portal the panel to <body> so the welcome card's
          overflow-hidden can't clip it; anchored to the input's rect. */}
      {embedded && showSlashPanel && panelPos && createPortal(
        <div
          className="fixed z-50"
          style={{ left: panelPos.left, width: panelPos.width, bottom: panelPos.bottom }}
        >
          {slashPanel}
        </div>,
        document.body,
      )}
    </div>
  );
}
