/**
 * McpCredentialDialog — OAuth credential editor for an MCP server.
 *
 * The token endpoint + client id are stored alongside the tokens so the
 * backend can auto-renew expired access tokens via the OAuth2 refresh
 * grant (see `McpCredentialStore::refresh_expired`).
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Save, Trash2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { McpServerConfig, McpCredentialInput } from "@/types";

export interface McpCredentialDialogProps {
  server: McpServerConfig;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (input: McpCredentialInput) => Promise<void>;
  onDelete: () => Promise<void>;
  hasCredential: boolean;
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  type = "text",
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: string;
}) {
  return (
    <label className="block space-y-1">
      <span className="text-[11px] font-medium text-muted-foreground">{label}</span>
      <Input
        type={type}
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className="h-8 text-xs"
      />
    </label>
  );
}

export function McpCredentialDialog({
  server,
  open,
  onOpenChange,
  onSave,
  onDelete,
  hasCredential,
}: McpCredentialDialogProps) {
  const { t } = useTranslation();
  const [tokenEndpoint, setTokenEndpoint] = useState("");
  const [clientId, setClientId] = useState("");
  const [accessToken, setAccessToken] = useState("");
  const [refreshToken, setRefreshToken] = useState("");
  const [tokenType, setTokenType] = useState("Bearer");
  const [expiresAt, setExpiresAt] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The dialog stays mounted (open toggles visibility), so the form fields
  // only initialize on first mount. Re-open must start blank — otherwise a
  // cancelled edit's stale token/client id (or a previously saved value)
  // persists, misleading the next edit.
  useEffect(() => {
    if (!open) return;
    setTokenEndpoint("");
    setClientId("");
    setAccessToken("");
    setRefreshToken("");
    setTokenType("Bearer");
    setExpiresAt("");
    setSaving(false);
    setError(null);
  }, [open]);

  const handleSave = async () => {
    if (!accessToken.trim()) {
      setError(t("settings.mcp.credentialAccessTokenRequired", { defaultValue: "访问令牌必填" }));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await onSave({
        tokenEndpoint: tokenEndpoint.trim(),
        clientId: clientId.trim(),
        accessToken: accessToken.trim(),
        refreshToken: refreshToken.trim(),
        tokenType: tokenType.trim() || "Bearer",
        expiresAt: expiresAt ? new Date(expiresAt).toISOString() : "",
      });
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    setSaving(true);
    setError(null);
    try {
      await onDelete();
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle className="text-sm">
            {t("settings.mcp.credentialTitle", { defaultValue: "OAuth 凭据" })} — {server.name}
          </DialogTitle>
          <DialogDescription className="text-[11px]">
            {t("settings.mcp.credentialDesc", {
              defaultValue:
                "填入令牌与续期信息。token endpoint + client id 用于过期后自动刷新 access token。",
            })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2.5">
          <Field
            label={t("settings.mcp.credentialTokenEndpoint", { defaultValue: "Token Endpoint（续期用）" })}
            value={tokenEndpoint}
            onChange={setTokenEndpoint}
            placeholder="https://example.com/oauth/token"
          />
          <Field
            label={t("settings.mcp.credentialClientId", { defaultValue: "Client ID（续期用）" })}
            value={clientId}
            onChange={setClientId}
            placeholder="optional"
          />
          <Field
            label={t("settings.mcp.credentialAccessToken", { defaultValue: "Access Token" })}
            value={accessToken}
            onChange={setAccessToken}
            type="password"
          />
          <div className="grid grid-cols-2 gap-2.5">
            <Field
              label={t("settings.mcp.credentialTokenType", { defaultValue: "Token 类型" })}
              value={tokenType}
              onChange={setTokenType}
              placeholder="Bearer"
            />
            <Field
              label={t("settings.mcp.credentialExpiresAt", { defaultValue: "过期时间" })}
              value={expiresAt}
              onChange={setExpiresAt}
              type="datetime-local"
            />
          </div>
          <Field
            label={t("settings.mcp.credentialRefreshToken", { defaultValue: "Refresh Token（可选）" })}
            value={refreshToken}
            onChange={setRefreshToken}
            type="password"
          />
        </div>

        {error && <p className="text-[11px] text-destructive">{error}</p>}

        <DialogFooter className="gap-2">
          {hasCredential && (
            <Button
              variant="destructive"
              size="sm"
              className="mr-auto gap-1 text-[11px]"
              disabled={saving}
              onClick={() => void handleDelete()}
            >
              <Trash2 className="h-3 w-3" />
              {t("settings.mcp.credentialDelete", { defaultValue: "删除凭据" })}
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="text-[11px]"
            disabled={saving}
            onClick={() => onOpenChange(false)}
          >
            {t("common.cancel")}
          </Button>
          <Button size="sm" className="gap-1 text-[11px]" disabled={saving} onClick={() => void handleSave()}>
            {saving ? <Loader2 className="h-3 w-3 animate-spin" /> : <Save className="h-3 w-3" />}
            {t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
