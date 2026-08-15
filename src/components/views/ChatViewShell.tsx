/**
 * ChatViewShell — shared layout shell for Code and Depwork chat views.
 *
 * Handles the common structure:
 *   - Content area: empty → UnifiedWelcome, else MessageList + ChatInput
 *   - PermissionDialog overlay + AskUserDialog
 *   - Compaction toast
 *
 * Each mode provides its own store-specific selectors.
 * This component does NOT import chatStore or depworkChatStore directly.
 */

import { usePermissionEvents } from "@/hooks/usePermissionEvents";
import { useAutoReviewEvents } from "@/hooks/useAutoReviewEvents";
import { useAskUserEvents } from "@/hooks/useAskUserEvents";
import { usePlanApprovalEvents, } from "@/hooks/usePlanApprovalEvents";
import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import { Loader2, X } from "lucide-react";
import { MessageList } from "@/components/chat/MessageList";
import { ChatInput } from "@/components/chat/ChatInput";
import { UnifiedWelcome } from "@/components/chat/UnifiedWelcome";
import { PermissionDialog } from "@/components/chat/PermissionDialog";
import { AutoReviewDenialCard } from "@/components/chat/AutoReviewDenialCard";
import { AskUserDialog } from "@/components/chat/AskUserDialog";
import { ElicitationDialog } from "@/components/chat/ElicitationDialog";
import { TaskCompletedBanner } from "@/components/chat/TaskCompletedBanner";
import { OfficeTypingHint } from "@/components/chat/OfficeTypingHint";
import { FileDropOverlay } from "@/components/chat/FileDropOverlay";
import { PlanApprovalPanel } from "@/components/chat/PlanApprovalPanel";
import type { AppMode } from "@/config/constants";
import type { UIMessage } from "@/types";
import type { CompactionRecord } from "@/stores/chatStore/types";

export interface PendingElicitation {
  elicitationId: string;
  serverName: string;
  message: string;
}

interface ChatViewShellProps {
  mode: AppMode;
  /** Array of message objects (same UIMessage type for both surfaces since
   *  the #79 store merge). */
  messages: UIMessage[];
  /** Recent compaction records — rendered as dividers in the timeline. */
  compactions?: CompactionRecord[];
  isEmpty: boolean;
  /** True while a selected session's history is being fetched — shows the
   *  loading skeleton instead of flashing the welcome page. */
  loading?: boolean;
  notification: string | null;
  dismissNotification: () => void;
  /** Current session id — scopes plan approvals and pending interactions. */
  sessionId?: string | null;
  /** Pending MCP elicitation request (null when none). */
  pendingElicitation: PendingElicitation | null;
  /** Respond to a pending MCP elicitation request. */
  respondElicitation: (
    elicitationId: string,
    action: "accept" | "decline" | "cancel",
    content?: unknown,
  ) => Promise<void>;
  /** Custom empty-state hero rendered instead of UnifiedWelcome (e.g. the
   *  Depwork drop zone). When set, the caller is responsible for rendering
   *  the ChatInput inside it. */
  emptyHero?: ReactNode;
  /** 定点修改：选中助手消息内容后交回引用草稿（调用方写入对应 store 的输入框）。 */
  onRefineSelection?: (draft: string) => void;
}

export function ChatViewShell({
  mode,
  messages,
  compactions = [],
  isEmpty,
  loading = false,
  notification,
  dismissNotification,
  sessionId,
  pendingElicitation,
  respondElicitation,
  emptyHero,
  onRefineSelection,
}: ChatViewShellProps) {
  const { t } = useTranslation();
  usePermissionEvents();
  useAutoReviewEvents();
  useAskUserEvents();
  usePlanApprovalEvents(sessionId);

  /** Loading state for session restore — a quiet skeleton beats a blank
   *  flash while history is fetched from the backend. */
  function SessionLoading() {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2">
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        <p className="text-xs text-muted-foreground">{t("chat.loadingSession")}</p>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="relative flex min-h-0 flex-1 flex-col">
        {/* ── Content area: welcome hero or message list ──
            flex-col is REQUIRED: MessageList sizes itself with flex-1,
            so without a column flex context its h-full would overflow the
            area and the list's tail would slide under the input row. */}
        <div className="relative flex min-h-0 flex-1 flex-col">
          {isEmpty ? (
            loading ? (
              <SessionLoading />
            ) : (
              emptyHero ?? <UnifiedWelcome mode={mode} />
            )
          ) : (
            <>
              <MessageList
                messages={messages}
                compactions={compactions}
                onRefineSelection={onRefineSelection}
              />
            </>
          )}

          <FileDropOverlay mode={mode} />

          {/* Decision dialogs in the empty state — anchored to the bottom
              of the content area (they can only fire once a conversation
              exists, so this path is effectively unreachable; the non-empty
              anchor below is the real one). */}
          {isEmpty && (
            <div className="pointer-events-none absolute inset-x-0 bottom-3 z-40 flex justify-center">
              <div className="pointer-events-auto w-[min(672px,calc(100%-2rem))]">
                <PermissionDialog sessionId={sessionId} />
                <AutoReviewDenialCard sessionId={sessionId} />
                <AskUserDialog />
                <PlanApprovalPanel />
              </div>
            </div>
          )}
        </div>

        {/* ── Input row: decision panels float DIRECTLY above the input,
            anchored to its row — no magic bottom offsets, so chips, queue
            notices or a taller textarea can never cover them (and the
            panels can never cover the input). ── */}
        {!isEmpty && (
          <div className="relative shrink-0">
            <div className="pointer-events-none absolute inset-x-0 bottom-full z-40 flex justify-center pb-2">
              <div className="pointer-events-auto w-[min(672px,calc(100%-2rem))]">
                <PermissionDialog sessionId={sessionId} />
                <AutoReviewDenialCard sessionId={sessionId} />
                <AskUserDialog />
                <PlanApprovalPanel />
              </div>
            </div>
            <ChatInput compact mode={mode} />
          </div>
        )}

        {pendingElicitation && (
          <ElicitationDialog
            elicitationId={pendingElicitation.elicitationId}
            serverName={pendingElicitation.serverName}
            message={pendingElicitation.message}
            respond={respondElicitation}
          />
        )}

        {/* Stacked floating hints (banner + typing hint share one column
            so they never overlap on the screen). The notification toast
            joins the same column — absolute bottom-20 duplicates would
            stack ON TOP of each other. */}
        <div className="absolute bottom-20 left-1/2 z-20 flex w-[min(560px,90%)] -translate-x-1/2 flex-col items-center gap-1.5">
          <OfficeTypingHint className="shrink-0" />
          <TaskCompletedBanner
            mode={mode}
            className="w-full"
          />
          {notification && (
            <div className="w-full rounded-lg border border-border bg-popover px-4 py-2 text-xs text-popover-foreground shadow-popover animate-in fade-in slide-in-from-bottom duration-300">
              <div className="flex items-center gap-2">
                <span className="min-w-0 flex-1">{notification}</span>
                <button
                  onClick={dismissNotification}
                  className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
                  aria-label={t("common.dismiss", { defaultValue: "Dismiss" })}
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
