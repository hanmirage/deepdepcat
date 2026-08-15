/**
 * AgentBehaviorMenu — the permission-mode selector for the input bar.
 *
 * One pill button (只读 / 接受编辑 / 完全放行) with per-session persistence.
 * Execution strategy and persona moved to Settings → Agent.
 */

import { useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronDown,
  ClipboardList,
  FilePen,
  ShieldCheck,
  Zap,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { permissionApi, sessionApi } from "@/lib/tauri";
import type { InteractionMode, PermissionMode } from "@/types";
import type { DepworkInteractionMode } from "@/types/depwork";
import type { AppMode } from "@/config/constants";
import { cn } from "@/lib/utils";

interface ModeOption {
  id: InteractionMode;
  label: string;
  description: string;
  icon: typeof ShieldCheck;
  /** Backend PermissionMode this option maps to. */
  backend: PermissionMode;
}

const MODE_OPTIONS: ModeOption[] = [
  {
    id: "read_only",
    label: "chat.modeReadOnly",
    description: "chat.modeReadOnlyDesc",
    icon: ClipboardList,
    backend: "read_only",
  },
  {
    id: "accept_edits",
    label: "chat.modeAcceptEdits",
    description: "chat.modeAcceptEditsDesc",
    icon: FilePen,
    backend: "accept_edits",
  },
  {
    id: "full_access",
    label: "chat.modeFullAccess",
    description: "chat.modeAutoDesc",
    icon: Zap,
    backend: "full_access",
  },
];

/** Backend permission-mode wire string → frontend interaction-mode id.
 *  Legacy wire strings collapse onto the current 3 modes. */
function backendModeToInteraction(mode: string): InteractionMode | null {
  switch (mode) {
    case "read_only":
    case "plan":
    case "chat_only":
      return "read_only";
    case "accept_edits":
    case "manual":
    case "default":
      return "accept_edits";
    case "full_access":
    case "full-access":
    case "bypass":
    case "auto":
      return "full_access";
    default:
      return null;
  }
}

function ModeSection({
  activeMode,
  onSelect,
}: {
  activeMode: string;
  onSelect: (opt: ModeOption) => void;
}) {
  const { t } = useTranslation();
  const options = MODE_OPTIONS;
  return (
    <>
      <DropdownMenuLabel>{t("chat.interactionMode")}</DropdownMenuLabel>
      <DropdownMenuSeparator />
      {options.map((opt) => (
        <DropdownMenuItem
          key={opt.id}
          onClick={() => onSelect(opt)}
          className="flex items-start gap-2 py-2"
        >
          <opt.icon className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <div className="flex-1">
            <p className="text-xs font-medium">{t(opt.label)}</p>
            <p className="text-[10px] text-muted-foreground">{t(opt.description)}</p>
          </div>
          {activeMode === opt.id && <Check className="mt-0.5 h-3.5 w-3.5 text-primary" />}
        </DropdownMenuItem>
      ))}
    </>
  );
}

export interface AgentBehaviorMenuProps {
  className?: string;
  /** Which chat store to bind. "code" → chatStore, "depwork" → depworkChatStore. */
  mode?: AppMode;
}

export function AgentBehaviorMenu({ className, mode = "code" }: AgentBehaviorMenuProps) {
  const { t } = useTranslation();
  const isDepwork = mode === "depwork";
  const chatMode = useChatStore((s) => s.interactionMode);
  const setChatMode = useChatStore((s) => s.setInteractionMode);
  const depworkMode = useDepworkChatStore((s) => s.interactionMode);
  const setDepworkMode = useDepworkChatStore((s) => s.setInteractionMode);
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);

  const activeMode = isDepwork ? depworkMode : chatMode;
  const activeSessionId = isDepwork ? depworkSessionId : codeSessionId;
  const options = MODE_OPTIONS;
  const current = options.find((m) => m.id === activeMode) ?? options[0];
  const readOnly = current.backend === "read_only";

  // Per-session mode scope — copied from ModeComboBox unchanged.
  useEffect(() => {
    if (!activeSessionId) return;
    let cancelled = false;
    void (async () => {
      let persisted: string | undefined;
      try {
        const s = await sessionApi.getSession(activeSessionId);
        persisted = s?.permission_mode;
      } catch {
        // keep the menu default
      }
      if (cancelled) return;
      if (persisted) {
        const mapped = backendModeToInteraction(persisted);
        if (mapped) {
          const currentValue = isDepwork
            ? useDepworkChatStore.getState().interactionMode
            : useChatStore.getState().interactionMode;
          if (mapped !== currentValue) {
            if (isDepwork) {
              setDepworkMode(mapped as DepworkInteractionMode);
            } else {
              setChatMode(mapped as InteractionMode);
            }
          }
        }
        return;
      }
      // No persisted session mode → mirror the backend GLOBAL mode for
      // display, but do NOT write it into the session row. Writing here was
      // the "一直处于计划模式" bug: the code-side fallback default used to
      // resolve to plan, which permanently locked the session read-only.
      try {
        const globalMode = await permissionApi.getMode();
        if (cancelled || !globalMode) return;
        const mapped = backendModeToInteraction(globalMode);
        if (!mapped) return;
        const currentValue = isDepwork
          ? useDepworkChatStore.getState().interactionMode
          : useChatStore.getState().interactionMode;
        if (mapped !== currentValue) {
          if (isDepwork) {
            setDepworkMode(mapped as DepworkInteractionMode);
          } else {
            setChatMode(mapped as InteractionMode);
          }
        }
      } catch {
        // keep the menu default
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- session switch only
  }, [activeSessionId]);

  const handleSelectMode = useCallback(
    (opt: ModeOption) => {
      if (isDepwork) {
        setDepworkMode(opt.id);
      } else {
        setChatMode(opt.id);
      }
      // No session yet → keep the choice LOCAL only. Writing it without a
      // session id would change the GLOBAL mode and leak the selection into
      // every other conversation (the old scope bug). The next created
      // session inherits the combo's current mode into its own session row.
      if (activeSessionId) {
        void permissionApi.setMode(opt.backend, activeSessionId);
      }
    },
    [isDepwork, setChatMode, setDepworkMode, activeSessionId],
  );

  const triggerLabel = t(current.label);

  return (
    <div className="relative flex items-center">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            className={cn(
              "h-7 gap-1 rounded-full border border-border/60 bg-muted/20 px-2 text-xs",
              "hover:bg-muted/40 hover:text-foreground",
              readOnly && "border-amber-500/30 text-amber-700 dark:text-amber-400",
              className,
            )}
            aria-label={t("chat.interactionMode")}
          >
            <current.icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span className="max-w-[9rem] truncate">{triggerLabel}</span>
            {readOnly && (
              <span className="rounded-sm bg-amber-500/15 px-1 text-[10px] font-medium text-amber-600 dark:text-amber-400">
                {t("chat.modeReadOnly")}
              </span>
            )}
            <ChevronDown className="h-3 w-3 text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-[70vh] w-72 overflow-auto">
          <ModeSection activeMode={activeMode} onSelect={handleSelectMode} />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
