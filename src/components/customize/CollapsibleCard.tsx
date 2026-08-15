/**
 * CollapsibleCard — collapsible card shared by all right-panel cards.
 *
 * Header: chevron + icon + title + optional badge; the whole header is
 * clickable to expand/collapse the body. Replaces the four duplicated
 * card-header implementations (AgentActivity/Skills/Connectors/Plugins).
 */

import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, type LucideIcon } from "lucide-react";
import { Card, CardHeader, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

export interface CollapsibleCardProps {
  icon: LucideIcon;
  title: string;
  /** Content rendered at the right end of the header (e.g. a count badge). */
  badge?: ReactNode;
  defaultOpen?: boolean;
  /** Extra classes for the body container (defaults to `space-y-2`). */
  contentClassName?: string;
  children?: ReactNode;
}

export function CollapsibleCard({
  icon: Icon,
  title,
  badge,
  defaultOpen = true,
  contentClassName,
  children,
}: CollapsibleCardProps) {
  const [expanded, setExpanded] = useState(defaultOpen);
  const contentId = `collapsible-card-${title.replace(/\s+/g, "-").toLowerCase()}`;

  return (
    <Card>
      <CardHeader className="p-0">
        {/* The whole header is the expand control — a real <button> so
            keyboard users can Tab to it and activate with Enter/Space. */}
        <button
          onClick={() => setExpanded(!expanded)}
          aria-expanded={expanded}
          aria-controls={contentId}
          className="flex w-full cursor-pointer items-center gap-2 rounded-t-lg p-4 text-left transition-colors hover:bg-muted/40"
        >
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          )}
          <Icon className="h-4 w-4 shrink-0 text-primary" />
          <span className="text-sm font-semibold leading-none tracking-tight">
            {title}
          </span>
          {badge && (
            <Badge variant="secondary" className="ml-auto text-[9px]">
              {badge}
            </Badge>
          )}
        </button>
      </CardHeader>
      {/* Smooth expand/collapse via grid-template-rows (0fr→1fr) — the same
          pattern as ReasoningBlock, so every collapsible in the app animates
          with one consistent 260ms ease-out. Always mounted (state toggles
          rows), so the content animates instead of popping in/out. */}
      <div className={cn("reasoning-expand", expanded && "reasoning-expand-open")}>
        <div className="reasoning-expand-inner">
          <CardContent id={contentId} className={cn("space-y-2", contentClassName)}>
            {children}
          </CardContent>
        </div>
      </div>
    </Card>
  );
}
