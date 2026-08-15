/**
 * ScrollToBottom — floating circular button.
 *
 * Appears when the user scrolls up in the message list.
 * Clicking scrolls back to the latest message with smooth behavior.
 */

import { ArrowDown } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

export interface ScrollToBottomProps {
  visible: boolean;
  onClick: () => void;
  className?: string;
}

export function ScrollToBottom({ visible, onClick, className }: ScrollToBottomProps) {
  const { t } = useTranslation();
  return (
    <button
      onClick={onClick}
      className={cn(
        "absolute bottom-1 left-1/2 z-10 flex h-8 w-8 -translate-x-1/2 items-center justify-center",
        "rounded-full border border-border bg-card shadow-lg backdrop-blur-sm",
        "text-muted-foreground transition-all duration-200 hover:text-foreground",
        visible
          ? "opacity-100"
          : "pointer-events-none translate-y-2 opacity-0",
        className,
      )}
      aria-label={t("chat.scrollToBottom")}
    >
      <ArrowDown className="h-4 w-4" />
    </button>
  );
}
