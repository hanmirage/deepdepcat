/**
 * SidebarFooter — the account identity area at the bottom of the sidebar.
 *
 * One consistent container across all three states (same padding, same row
 * height — switching states never shifts the layout):
 * - Signed in: avatar with a live status dot on its corner + username /
 *   status line + gear. Click opens the account menu.
 * - Signed out: quiet identity row ("sign in · sync sessions & settings").
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { LogOut, Settings, UserCircle2 } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAgentStatus } from "@/hooks/useAgentStatus";
import { useAppStore } from "@/stores/appStore";
import { useAuthStore } from "@/stores/authStore";
import { PersonalCenterDialog } from "@/components/sidebar/PersonalCenterDialog";
import { LoginDialog } from "@/components/sidebar/LoginDialog";
import { cn } from "@/lib/utils";
import appIcon from "/icon.png";

// ── Agent status dot config ───────────────────────────────

const STATUS_CONFIG: Record<string, { labelKey: string; dot: string }> = {
  idle:         { labelKey: "sidebar.statusIdle",      dot: "bg-muted-foreground/40" },
  thinking:     { labelKey: "sidebar.statusThinking",  dot: "bg-primary" },
  tool_running: { labelKey: "sidebar.statusRunning",   dot: "bg-primary" },
  connecting:   { labelKey: "sidebar.statusConnecting", dot: "bg-amber-500 dark:bg-amber-400" },
  error:        { labelKey: "sidebar.statusError",     dot: "bg-destructive" },
};

const DEFAULT_STATUS = { labelKey: "sidebar.statusIdle", dot: "bg-muted-foreground/40" };

// ── Settings gear (shared by every state) ─────────────────

function SettingsButton() {
  const { t } = useTranslation();
  const setSettingsOpen = useAppStore((s) => s.setSettingsOpen);
  return (
    <button
      onClick={() => setSettingsOpen(true)}
      className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
      aria-label={t("layout.settings")}
    >
      <Settings className="h-3.5 w-3.5" />
    </button>
  );
}

// ── Signed-in row ─────────────────────────────────────────

function SignedInRow({
  onMenuChange,
  onOpenCenter,
}: {
  onMenuChange: (open: boolean) => void;
  onOpenCenter: () => void;
}) {
  const { t } = useTranslation();
  const agentStatus = useAgentStatus();
  const user = useAuthStore((s) => s.user);
  const verifyLogin = useAuthStore((s) => s.verifyLogin);
  const logout = useAuthStore((s) => s.logout);

  const cfg = STATUS_CONFIG[agentStatus] ?? DEFAULT_STATUS;
  const isActive = agentStatus === "tool_running" || agentStatus === "thinking";
  const statusLabel = t(cfg.labelKey);
  const hasAvatar = Boolean(user?.avatar);

  if (!user) return null;

  return (
    <div className="flex items-center gap-1.5 border-t border-[hsl(var(--sidebar-border))] px-2 py-2">
      <DropdownMenu onOpenChange={onMenuChange}>
        <DropdownMenuTrigger asChild>
          <button
            className="flex min-w-0 flex-1 items-center gap-2 rounded-md p-1 text-left transition-colors hover:bg-secondary/60"
            onClick={() => void verifyLogin()}
          >
            {/* Avatar with live status dot on the corner */}
            <span className="relative shrink-0">
              <Avatar className="h-7 w-7">
                {hasAvatar && <AvatarImage src={user.avatar!} alt={user.username} />}
                <AvatarFallback className="bg-secondary p-0.5">
                  <img src={appIcon} alt="DeepDepCat" className="h-full w-full rounded-sm" />
                </AvatarFallback>
              </Avatar>
              <span className="absolute -bottom-[1px] -right-[1px]">
                <span className={cn("relative flex h-2 w-2 rounded-full ring-2 ring-[hsl(var(--sidebar-bg))]", cfg.dot)}>
                  {isActive && (
                    <span className={cn("absolute inline-flex h-full w-full animate-ping rounded-full opacity-40", cfg.dot)} />
                  )}
                </span>
              </span>
            </span>

            <span className="min-w-0 flex-1">
              <span className="block truncate text-xs font-medium leading-tight">
                {user.username}
              </span>
              <span className="block truncate text-[10px] leading-tight text-muted-foreground">
                {statusLabel}
              </span>
            </span>
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="top" className="w-44 mb-1">
          <div className="px-2 py-1.5">
            <p className="text-xs font-medium">{user.username}</p>
          </div>
          <DropdownMenuSeparator />
          <DropdownMenuItem className="text-xs" onClick={onOpenCenter}>
            <UserCircle2 className="mr-2 h-3.5 w-3.5" />
            {t("sidebar.personalCenter")}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem className="text-xs text-destructive" onClick={() => logout()}>
            <LogOut className="mr-2 h-3.5 w-3.5" />
            {t("sidebar.logout")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <SettingsButton />
    </div>
  );
}

// ── Signed-out row ────────────────────────────────────────

function SignedOutRow({ onSignIn }: { onSignIn: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5 border-t border-[hsl(var(--sidebar-border))] px-2 py-2">
      <button
        onClick={onSignIn}
        className="group flex min-w-0 flex-1 items-center gap-2 rounded-md p-1 text-left transition-colors hover:bg-secondary/60"
      >
        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full border border-muted-foreground/30 bg-muted/40 text-muted-foreground/70 transition-colors group-hover:border-primary/50 group-hover:text-primary">
          <UserCircle2 className="h-4 w-4" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-xs font-medium leading-tight text-foreground/90">
            {t("sidebar.signIn")}
          </span>
          <span className="block truncate text-[10px] leading-tight text-muted-foreground">
            {t("sidebar.signInSubtitle")}
          </span>
        </span>
      </button>
      <SettingsButton />
    </div>
  );
}

// ── Main footer ───────────────────────────────────────────

export function SidebarFooter() {
  const { user } = useAuthStore();
  const [, setMenuOpen] = useState(false);
  const [centerOpen, setCenterOpen] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);

  // ── Not logged in ─────────────────────────────────────
  if (!user) {
    return (
      <>
        <SignedOutRow onSignIn={() => setLoginOpen(true)} />
        <LoginDialog open={loginOpen} onOpenChange={setLoginOpen} />
      </>
    );
  }

  // ── Logged in ─────────────────────────────────────────
  return (
    <>
      <SignedInRow
        onMenuChange={setMenuOpen}
        onOpenCenter={() => {
          setMenuOpen(false);
          setCenterOpen(true);
        }}
      />
      <PersonalCenterDialog
        user={user}
        open={centerOpen}
        onOpenChange={setCenterOpen}
      />
    </>
  );
}
