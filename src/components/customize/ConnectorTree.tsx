/**
 * ConnectorTree — tree view of connector resources & permissions.
 *
 * Shows a nested structure of:
 * - Folders / files (with expand/collapse)
 * - Databases
 * - API endpoints
 *
 * Each node displays its access level (read-only / read-write)
 * with a toggle switch.
 */

import { useState } from "react";
import {
  Folder,
  File,
  Database,
  ChevronDown,
  ChevronRight,
  Lock,
  Unlock,
  type LucideIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import type { Permission } from "@/types";
import { cn, shortPath } from "@/lib/utils";

const TYPE_ICONS: Record<string, LucideIcon> = {
  folder: Folder,
  file: File,
  database: Database,
  api: Unlock,
};

interface ConnectorTreeProps {
  permissions: Permission[];
}

export function ConnectorTree({ permissions }: ConnectorTreeProps) {
  return (
    <div className="ml-3 space-y-0.5 border-l border-border pl-2">
      {permissions.map((perm) => (
        <PermissionNode key={perm.resource} permission={perm} />
      ))}
    </div>
  );
}

function PermissionNode({ permission }: { permission: Permission }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const Icon = TYPE_ICONS[permission.resource_type] ?? File;
  const isReadWrite = permission.access === "read-write";

  return (
    <div>
      <div className="flex items-center gap-1.5 rounded-md px-1 py-1 hover:bg-secondary/50 transition-colors">
        {/* Expand/collapse (only for folders) */}
        {permission.resource_type === "folder" ? (
          <button
            onClick={() => setOpen(!open)}
            className="shrink-0 rounded p-0.5 hover:bg-muted"
            aria-expanded={open}
            aria-label={open ? t("common.collapse") : t("common.expand")}
          >
            {open ? (
              <ChevronDown className="h-3 w-3 text-muted-foreground" />
            ) : (
              <ChevronRight className="h-3 w-3 text-muted-foreground" />
            )}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}

        <Icon className="h-3 w-3 shrink-0 text-muted-foreground" />

        <span className="flex-1 truncate text-[11px]">
          {shortPath(permission.resource)}
        </span>

        {/* Access badge */}
        <div className="flex items-center gap-1">
          {isReadWrite ? (
            <Unlock className="h-2.5 w-2.5 text-green-500" />
          ) : (
            <Lock className="h-2.5 w-2.5 text-amber-500" />
          )}
          <Badge
            variant={isReadWrite ? "success" : "warning"}
            className="text-[8px] px-1 py-0"
          >
            {isReadWrite ? "RW" : "RO"}
          </Badge>
        </div>

        {/* Enabled/disabled state — backend-owned status. Displayed as a dot
            because the backend has no per-permission toggle command; a
            switch here would be a non-functional control. */}
        <span
          className={cn(
            "ml-0.5 h-1.5 w-1.5 shrink-0 rounded-full",
            permission.enabled ? "bg-emerald-500" : "bg-muted-foreground/40",
          )}
          title={permission.enabled ? t("common.enabled") : t("common.disabled")}
        />
      </div>
    </div>
  );
}
