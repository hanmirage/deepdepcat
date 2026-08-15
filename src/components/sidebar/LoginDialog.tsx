/**
 * LoginDialog — direct email+password sign-in against the website account,
 * with an in-app two-step registration (send verification code → verify
 * email → auto-fill login).
 *
 * All calls go through the Rust side (the website API has no CORS).
 */

import { useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, LogIn, UserPlus, Mail, ShieldCheck } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { useAuthStore } from "@/stores/authStore";
import { registerApi } from "@/lib/tauri";

interface LoginDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

type View = "login" | "register" | "verify";

export function LoginDialog({ open, onOpenChange }: LoginDialogProps) {
  const { t } = useTranslation();
  const loginWithPassword = useAuthStore((s) => s.loginWithPassword);
  const loginLoading = useAuthStore((s) => s.loginLoading);
  const errorKind = useAuthStore((s) => s.errorKind);
  const error = useAuthStore((s) => s.error);
  const clearError = useAuthStore((s) => s.clearError);
  const serverUrl = useAuthStore((s) => s.serverUrl);

  const [view, setView] = useState<View>("login");

  // ── Login fields ──────────────────────────────────────────
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  // ── Register fields ───────────────────────────────────────
  const [regName, setRegName] = useState("");
  const [regEmail, setRegEmail] = useState("");
  const [regPassword, setRegPassword] = useState("");
  const [regCode, setRegCode] = useState("");
  const [regMessage, setRegMessage] = useState<string | null>(null);
  const [regError, setRegError] = useState<string | null>(null);
  const [regSending, setRegSending] = useState(false);
  const [regVerifying, setRegVerifying] = useState(false);

  // Reset local fields each time the dialog opens. The auth error lives in
  // the shared store — a failure from another surface (onboarding, profile)
  // must not leak into this dialog, so clear it on both open and close.
  const handleOpenChange = useCallback(
    (next: boolean) => {
      clearError();
      if (!next) {
        setEmail("");
        setPassword("");
        setRegName("");
        setRegEmail("");
        setRegPassword("");
        setRegCode("");
        setRegMessage(null);
        setRegError(null);
        setView("login");
      }
      onOpenChange(next);
    },
    [onOpenChange, clearError],
  );

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (loginLoading) return;
    const trimmed = email.trim();
    if (!trimmed || !password) return;
    void loginWithPassword(trimmed, password).then((ok) => {
      if (ok) onOpenChange(false);
    });
  };

  // Friendly copy per classified failure.
  const errorText = (() => {
    switch (errorKind) {
      case "rejected":
        return t("sidebar.loginErrorRejected");
      case "network":
        return t("sidebar.loginErrorNetwork");
      case "session":
        return t("sidebar.loginErrorSession");
      case "account":
        return t("sidebar.loginErrorAccount");
      case "rate_limited":
        // The server's message ("尝试过于频繁，请 N 秒后重试") is more useful
        // than generic copy — show it when present, else fall back.
        return error || t("sidebar.loginErrorRateLimited");
      case "unknown":
        return t("sidebar.loginErrorUnknown");
      default:
        return null;
    }
  })();

  // ── Registration flow ─────────────────────────────────────
  // Step 1: send-code → server replies (with the dev-mode code echoed when
  // SMTP is off). Step 2: verify-email → account created → back to login
  // with the email pre-filled.
  const handleSendCode = (e: React.FormEvent) => {
    e.preventDefault();
    if (regSending) return;
    const name = regName.trim();
    const mail = regEmail.trim();
    if (!name || !mail || regPassword.length < 6) {
      setRegError(t("sidebar.loginRegisterFieldsInvalid", { defaultValue: "请填写昵称、邮箱,密码至少 6 位" }));
      return;
    }
    setRegSending(true);
    setRegError(null);
    setRegMessage(null);
    void registerApi
      .sendCode(serverUrl, mail, name, regPassword)
      .then((message) => {
        setRegMessage(message);
        setView("verify");
      })
      .catch((e) => {
        setRegError(typeof e === "string" ? e : t("sidebar.loginRegisterSendCodeFailed", { defaultValue: "发送验证码失败" }));
      })
      .finally(() => setRegSending(false));
  };

  const handleVerifyCode = (e: React.FormEvent) => {
    e.preventDefault();
    if (regVerifying) return;
    const code = regCode.trim().toUpperCase();
    if (!code) return;
    setRegVerifying(true);
    setRegError(null);
    void registerApi
      .verifyEmail(serverUrl, regEmail.trim(), code)
      .then(() => {
        // Account created — drop into login with the email pre-filled.
        setEmail(regEmail.trim());
        setPassword("");
        setView("login");
        setRegMessage(t("sidebar.loginRegisterSuccess", { defaultValue: "注册成功,请登录" }));
      })
      .catch((e) => {
        setRegError(typeof e === "string" ? e : t("sidebar.loginRegisterVerifyFailed", { defaultValue: "验证失败" }));
      })
      .finally(() => setRegVerifying(false));
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-base">
            {view === "login" ? (
              <>
                <LogIn className="h-4 w-4 text-primary" />
                {t("sidebar.loginDialogTitle")}
              </>
            ) : view === "register" ? (
              <>
                <UserPlus className="h-4 w-4 text-primary" />
                {t("sidebar.loginRegisterTitle", { defaultValue: "注册账号" })}
              </>
            ) : (
              <>
                <ShieldCheck className="h-4 w-4 text-primary" />
                {t("sidebar.loginVerifyEmailTitle", { defaultValue: "验证邮箱" })}
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        {view === "login" && (
          <>
            <form onSubmit={handleSubmit} className="space-y-3">
              <div className="space-y-1.5">
                <label htmlFor="login-email" className="text-xs font-medium text-foreground/80">
                  {t("sidebar.loginEmailLabel")}
                </label>
                <Input
                  id="login-email"
                  type="email"
                  autoComplete="email"
                  placeholder={t("sidebar.loginEmailPlaceholder")}
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  disabled={loginLoading}
                  required
                />
              </div>

              <div className="space-y-1.5">
                <label htmlFor="login-password" className="text-xs font-medium text-foreground/80">
                  {t("sidebar.loginPasswordLabel")}
                </label>
                <Input
                  id="login-password"
                  type="password"
                  autoComplete="current-password"
                  placeholder={t("sidebar.loginPasswordPlaceholder")}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  disabled={loginLoading}
                  required
                />
              </div>

              {regMessage && (
                <p className="text-xs text-emerald-600" role="status">
                  {regMessage}
                </p>
              )}
              {errorText && (
                <p className="text-xs text-destructive" role="alert">
                  {errorText}
                </p>
              )}

              <Button type="submit" className="w-full gap-1.5" disabled={loginLoading}>
                {loginLoading && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
                {loginLoading ? t("sidebar.loginSubmitting") : t("sidebar.loginSubmit")}
              </Button>
            </form>

            <div className="mt-1 flex items-center justify-between">
              <button
                type="button"
                onClick={() => {
                  setRegError(null);
                  setRegMessage(null);
                  setView("register");
                }}
                className="flex items-center gap-1 text-xs text-primary hover:underline"
              >
                <UserPlus className="h-3 w-3" />
                {t("sidebar.loginRegisterTitle", { defaultValue: "注册账号" })}
              </button>
              <a
                href="https://deepdepcat.hsmiai.xyz"
                target="_blank"
                rel="noopener noreferrer"
                className="text-xs text-muted-foreground hover:text-primary hover:underline"
              >
                {t("sidebar.loginRegisterLink")}
              </a>
            </div>
          </>
        )}

        {view === "register" && (
          <>
            <form onSubmit={handleSendCode} className="space-y-3">
              <div className="space-y-1.5">
                <label htmlFor="reg-name" className="text-xs font-medium text-foreground/80">
                  {t("sidebar.loginNicknameLabel")}
                </label>
                <Input
                  id="reg-name"
                  value={regName}
                  onChange={(e) => setRegName(e.target.value)}
                  disabled={regSending}
                  placeholder={t("sidebar.loginNicknamePlaceholder", { defaultValue: "你的昵称" })}
                  required
                />
              </div>
              <div className="space-y-1.5">
                <label htmlFor="reg-email" className="text-xs font-medium text-foreground/80">
                  {t("sidebar.loginEmailLabel")}
                </label>
                <Input
                  id="reg-email"
                  type="email"
                  value={regEmail}
                  onChange={(e) => setRegEmail(e.target.value)}
                  disabled={regSending}
                  placeholder="name@example.com"
                  required
                />
              </div>
              <div className="space-y-1.5">
                <label htmlFor="reg-password" className="text-xs font-medium text-foreground/80">
                  {t("sidebar.loginPasswordMinHint")}
                </label>
                <Input
                  id="reg-password"
                  type="password"
                  value={regPassword}
                  onChange={(e) => setRegPassword(e.target.value)}
                  disabled={regSending}
                  autoComplete="new-password"
                  required
                />
              </div>

              {regError && (
                <p className="text-xs text-destructive" role="alert">
                  {regError}
                </p>
              )}

              <Button type="submit" className="w-full gap-1.5" disabled={regSending}>
                {regSending && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
                {regSending
                  ? t("sidebar.loginSendingCode", { defaultValue: "发送中…" })
                  : t("sidebar.loginSendCode", { defaultValue: "发送验证码" })}
              </Button>
            </form>

            <div className="mt-1 text-center">
              <button
                type="button"
                onClick={() => {
                  setRegError(null);
                  setView("login");
                }}
                className="text-xs text-muted-foreground hover:text-primary hover:underline"
              >
                {t("sidebar.loginBackToLogin")}
              </button>
            </div>
          </>
        )}

        {view === "verify" && (
          <>
            {regMessage && (
              <p className="flex items-start gap-1.5 text-xs text-muted-foreground">
                <Mail className="mt-0.5 h-3 w-3 shrink-0" />
                {regMessage}
              </p>
            )}
            <form onSubmit={handleVerifyCode} className="space-y-3">
              <div className="space-y-1.5">
                <label htmlFor="reg-code" className="text-xs font-medium text-foreground/80">
                  {t("sidebar.loginCodeLabel")}
                </label>
                <Input
                  id="reg-code"
                  value={regCode}
                  onChange={(e) => setRegCode(e.target.value)}
                  disabled={regVerifying}
                  placeholder={t("sidebar.loginCodePlaceholder", { defaultValue: "6 位验证码" })}
                  className="font-mono uppercase"
                  maxLength={6}
                  required
                />
              </div>

              {regError && (
                <p className="text-xs text-destructive" role="alert">
                  {regError}
                </p>
              )}

              <Button type="submit" className="w-full gap-1.5" disabled={regVerifying}>
                {regVerifying && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
                {regVerifying
                  ? t("sidebar.loginVerifying", { defaultValue: "验证中…" })
                  : t("sidebar.loginVerifyCreate", { defaultValue: "验证并创建账号" })}
              </Button>
            </form>

            <div className="mt-1 text-center">
              <button
                type="button"
                onClick={() => {
                  setRegError(null);
                  setView("register");
                }}
                className="text-xs text-muted-foreground hover:text-primary hover:underline"
              >
                {t("sidebar.loginReEnter", { defaultValue: "重新填写" })}
              </button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
