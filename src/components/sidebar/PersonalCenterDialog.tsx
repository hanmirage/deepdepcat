/**
 * PersonalCenterDialog — account profile for the logged-in user.
 *
 * Shows the cloud avatar (falls back to the app icon), editable display name,
 * email-ish account id, and the logout action. Renaming and avatar upload sync
 * to the website account (data/users.json) — every consumer picks it up.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { LogOut, Copy, Check, Pencil, Loader2, Camera } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useAuthStore, type AuthUserInfo } from "@/stores/authStore";
import { pickImage } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import appIcon from "/icon.png";

interface PersonalCenterDialogProps {
  user: AuthUserInfo;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function PersonalCenterDialog({ user, open, onOpenChange }: PersonalCenterDialogProps) {
  const { t } = useTranslation();
  const logout = useAuthStore((s) => s.logout);
  const updateProfile = useAuthStore((s) => s.updateProfile);
  const uploadAvatar = useAuthStore((s) => s.uploadAvatar);

  const [copied, setCopied] = useState(false);
  const [editingName, setEditingName] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [savingName, setSavingName] = useState(false);
  const [uploadingAvatar, setUploadingAvatar] = useState(false);

  const hasAvatar = Boolean(user.avatar);

  const handleCopyId = async () => {
    if (!user.user_id) return;
    try {
      await navigator.clipboard.writeText(user.user_id);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard unavailable — ignore.
    }
  };

  const startRename = () => {
    setNameDraft(user.username);
    setEditingName(true);
  };

  const saveRename = async () => {
    if (savingName) return;
    const trimmed = nameDraft.trim();
    if (!trimmed || trimmed === user.username) {
      setEditingName(false);
      return;
    }
    setSavingName(true);
    const ok = await updateProfile(trimmed);
    setSavingName(false);
    if (ok) setEditingName(false);
  };

  const handlePickAvatar = async () => {
    if (uploadingAvatar) return;
    const path = await pickImage();
    if (!path) return;
    setUploadingAvatar(true);
    await uploadAvatar(path);
    setUploadingAvatar(false);
  };

  const handleLogout = () => {
    onOpenChange(false);
    void logout();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>{t("sidebar.personalCenter")}</DialogTitle>
        </DialogHeader>

        <div className="flex flex-col items-center gap-3 py-2">
          {/* Avatar + upload */}
          <div className="relative">
            <Avatar className="h-16 w-16">
              {hasAvatar && <AvatarImage src={user.avatar!} alt={user.username} />}
              <AvatarFallback className="bg-secondary">
                <img src={appIcon} alt="DeepDepCat" className="h-full w-full rounded-sm p-1" />
              </AvatarFallback>
            </Avatar>
            <button
              onClick={handlePickAvatar}
              disabled={uploadingAvatar}
              title={t("sidebar.uploadAvatar")}
              className="absolute -bottom-0.5 -right-0.5 flex h-6 w-6 items-center justify-center rounded-full border border-border bg-background text-muted-foreground shadow-sm transition-colors hover:text-foreground disabled:opacity-60"
            >
              {uploadingAvatar ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Camera className="h-3 w-3" />
              )}
            </button>
          </div>

          {/* Name (editable) */}
          <div className="text-center">
            {editingName ? (
              <div className="flex items-center gap-1.5">
                <Input
                  value={nameDraft}
                  onChange={(e) => setNameDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void saveRename();
                    if (e.key === "Escape") setEditingName(false);
                  }}
                  autoFocus
                  maxLength={30}
                  className="h-7 w-36 text-sm"
                />
                <Button size="sm" className="h-7 px-2 text-xs" onClick={() => void saveRename()} disabled={savingName}>
                  {savingName ? <Loader2 className="h-3 w-3 animate-spin" /> : t("common.save")}
                </Button>
              </div>
            ) : (
              <button
                onClick={startRename}
                className="group flex items-center justify-center gap-1 text-sm font-semibold hover:text-primary"
                title={t("sidebar.rename")}
              >
                <span>{user.username}</span>
                <Pencil className={cn("h-3 w-3 text-muted-foreground transition-opacity group-hover:opacity-100", "opacity-40")} />
              </button>
            )}
            {user.user_id && (
              <button
                onClick={handleCopyId}
                className="mt-0.5 flex items-center gap-1 text-[10px] text-muted-foreground hover:text-foreground"
                title={t("sidebar.copyId")}
              >
                <span className="max-w-[200px] truncate font-mono">{user.user_id}</span>
                {copied ? (
                  <Check className="h-3 w-3 text-green-500" />
                ) : (
                  <Copy className="h-3 w-3" />
                )}
              </button>
            )}
          </div>
        </div>

        <DialogFooter className="gap-2">
          <Button
            variant="outline"
            size="sm"
            className="flex-1 text-xs"
            onClick={() => onOpenChange(false)}
          >
            {t("common.close")}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            className="flex-1 gap-1.5 text-xs"
            onClick={handleLogout}
          >
            <LogOut className="h-3.5 w-3.5" />
            {t("sidebar.logout")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
