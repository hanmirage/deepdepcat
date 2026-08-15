/**
 * SubagentPanel — right-panel pane for subagent dispatch.
 *
 * Single pane, N cards: every dispatched subagent of the ACTIVE mode's
 * session is one SubagentCard. Source is the event-driven `subagents` map on
 * the chat store (real-time turn progress + expandable results) — the
 * panel auto-appears via notifySubagents when the agent fans out.
 */

import { useTranslation } from "react-i18next";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { SubagentCard } from "./SubagentCard";

export function SubagentPanel({ isDepwork }: { isDepwork: boolean }) {
  const { t } = useTranslation();
  // Both stores are subscribed unconditionally (Rules of Hooks), then the
  // active mode's records are selected.
  const codeSubagents = useChatStore((s) => s.subagents);
  const depworkSubagents = useDepworkChatStore((s) => s.subagents);
  const subagents = isDepwork ? depworkSubagents : codeSubagents;

  const list = Object.values(subagents).sort((a, b) => a.startedAt - b.startedAt);
  if (list.length === 0) {
    return (
      <p className="px-1 py-2 text-[11px] text-muted-foreground/60">
        {t("subagents.empty")}
      </p>
    );
  }

  return (
    <div className="space-y-1.5">
      {list.map((s) => (
        <SubagentCard key={s.subagent_id} subagent={s} />
      ))}
    </div>
  );
}
