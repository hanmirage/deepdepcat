/**
 * InitConfig — Step 3: Initial configuration before entering the main app.
 *
 * Three cards: select default model, open workspace folder, choose theme.
 */

import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen, Globe, Palette, Check, LogIn, Loader2, UserCheck, Settings2, ChevronLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { useAppStore } from "@/stores/appStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useAuthStore } from "@/stores/authStore";
import { cn } from "@/lib/utils";

export interface InitConfigProps {
  onComplete: () => void;
  /** Return to the guide page (shows a back button). */
  onBack?: () => void;
}

export function InitConfig({ onComplete, onBack }: InitConfigProps) {
  const { t } = useTranslation();

  const theme = useAppStore((s) => s.theme);
  const toggleTheme = useAppStore((s) => s.toggleTheme);
  const workspacePath = useAppStore((s) => s.workspacePath);
  const openWorkspaceDialog = useAppStore((s) => s.openWorkspaceDialog);

  const providers = useSettingsStore((s) => s.providers);
  // Derive the model list in useMemo — providers is a stable reference, so
  // the selector never returns a new object (which would trigger an infinite
  // re-render via Zustand v5's useSyncExternalStore).
  const models = useMemo(
    () =>
      providers.flatMap((p) =>
        p.enabled ? p.models.map((m) => ({ name: m.name, provider: p.name })) : []
      ),
    [providers],
  );

  // ── Auth state (direct password login) ──
  const authUser = useAuthStore((s) => s.user);
  const authError = useAuthStore((s) => s.error);
  const authErrorKind = useAuthStore((s) => s.errorKind);
  const authLogin = useAuthStore((s) => s.loginWithPassword);
  const authLoginLoading = useAuthStore((s) => s.loginLoading);

  const [loginEmail, setLoginEmail] = useState("");
  const [loginPassword, setLoginPassword] = useState("");
  // Controlled model select — defaultValue would leave the option list and
  // the store out of sync when provider models change during onboarding.
  const [selectedModelName, setSelectedModelName] = useState("");

  const handlePasswordLogin = (e: React.FormEvent) => {
    e.preventDefault();
    if (authLoginLoading) return;
    if (!loginEmail.trim() || !loginPassword) return;
    void authLogin(loginEmail.trim(), loginPassword);
  };

  const setCodeModel = useChatStore((s) => s.setSelectedModel);
  const setDepworkModel = useDepworkChatStore((s) => s.setSelectedModel);
  const chatLoadModels = useChatStore((s) => s.loadModels);
  const depworkLoadModels = useDepworkChatStore((s) => s.loadModels);

  // Latest-selection guard: loadModels is async, so a fast A→B switch can
  // resolve out of order — A's late response must not reset the store to A
  // after the user already picked B.
  const modelSelectGen = useRef(0);

  const handleSelectModel = (modelName: string) => {
    const gen = ++modelSelectGen.current;
    // loadModels rebuilds the full ModelWithPricing list; after that we match
    // and set both stores so the selected model stays in sync with the user
    // setting in the main UI. Match by id OR name — ids are the real API
    // model names, display names may repeat across providers.
    Promise.all([chatLoadModels(), depworkLoadModels()])
      .then(() => {
        if (gen !== modelSelectGen.current) return; // a newer pick superseded this one
        const chatModel = useChatStore.getState().models.find(
          (m) => m.id === modelName || m.name === modelName,
        );
        const depworkModel = useDepworkChatStore.getState().models.find(
          (m) => m.id === modelName || m.name === modelName,
        );
        if (chatModel) setCodeModel(chatModel);
        if (depworkModel) setDepworkModel(depworkModel);
        setSelectedModelName(modelName);
      })
      .catch(() => {
        // Model fetch failed (provider unreachable) — the selection simply
        // doesn't apply. Never leave an unhandled rejection.
      });
  };

  const displayWorkspaceName = workspacePath
    ? workspacePath.split(/[\\/]/).pop() ?? workspacePath
    : null;

  const configuredProviderCount = providers.filter((p) => p.enabled).length;
  const hasModels = models.length > 0;

  // Steps completion
  const step1 = hasModels || configuredProviderCount > 0;
  const step2 = !!workspacePath;
  const allDone = step1 && step2;

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4">
        <div>
          <h2 className="text-lg font-semibold text-foreground">
            {t("onboarding.initTitle")}
          </h2>
          <p className="text-sm text-muted-foreground">
            {t("onboarding.initSubtitle")}
          </p>
        </div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>{t("onboarding.stepLabel", { current: 3, total: 3 })}</span>
        </div>
      </div>

      {/* Progress — theme is always set, so completion starts at 1/3. */}
      <div className="px-6 pb-2">
        <Progress
          value={Math.round((((step1 ? 1 : 0) + (step2 ? 1 : 0) + 1) / 3) * 100)}
          className="h-1"
        />
      </div>

      {/* Config cards */}
      <div className="flex-1 px-6 pb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
          {/* Card 1: Model */}
          <div className={cn(
            "rounded-xl border border-border bg-card/50 p-4",
            "transition-all duration-200",
          )}>
            <div className="mb-3 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Globe className="h-4 w-4 text-primary" />
                <span className="text-sm font-medium">{t("onboarding.selectModel")}</span>
              </div>
              {step1 && <Check className="h-4 w-4 text-primary" />}
            </div>

            {hasModels ? (
              <select
                className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm text-foreground focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
                value={selectedModelName}
                onChange={(e) => {
                  if (e.target.value) handleSelectModel(e.target.value);
                }}
              >
                <option value="" disabled>{t("onboarding.selectModelPlaceholder")}</option>
                {models.map((m) => (
                  // Key includes the provider — the same model name can
                  // exist under multiple providers.
                  <option key={`${m.provider}-${m.name}`} value={m.name}>
                    {m.name} ({m.provider})
                  </option>
                ))}
              </select>
            ) : (
              <div className="space-y-2">
                <p className="text-xs text-muted-foreground">
                  {t("onboarding.noModelsConfigured")}
                </p>
                {/* NOTE: opening Settings is impossible while onboarding is
                    active (AppShell isn't mounted), so this button enters
                    the main UI — the ModelSetupCard there guides the real
                    configuration flow. */}
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full gap-1.5 text-xs"
                  onClick={onComplete}
                >
                  <Settings2 className="h-3.5 w-3.5" />
                  {t("onboarding.skipForNow")}
                </Button>
                <p className="text-center text-[10px] text-muted-foreground/60">
                  {t("onboarding.skipHint")}
                </p>
              </div>
            )}
          </div>

          {/* Card 2: Workspace */}
          <div className={cn(
            "rounded-xl border border-border bg-card/50 p-4",
            "transition-all duration-200",
          )}>
            <div className="mb-3 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <FolderOpen className="h-4 w-4 text-primary" />
                <span className="text-sm font-medium">{t("onboarding.selectWorkspace")}</span>
              </div>
              {step2 && <Check className="h-4 w-4 text-primary" />}
            </div>

            {displayWorkspaceName ? (
              <div className="space-y-2">
                <p className="text-xs text-foreground/80">{displayWorkspaceName}</p>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full text-xs"
                  onClick={openWorkspaceDialog}
                >
                  {t("onboarding.changeWorkspace")}
                </Button>
              </div>
            ) : (
              <Button
                variant="outline"
                size="sm"
                className="w-full"
                onClick={openWorkspaceDialog}
              >
                <FolderOpen className="h-3.5 w-3.5" />
                <span>{t("onboarding.openWorkspace")}</span>
              </Button>
            )}
          </div>

          {/* Card 3: Theme */}
          <div className={cn(
            "rounded-xl border border-border bg-card/50 p-4",
            "transition-all duration-200",
          )}>
            <div className="mb-3 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Palette className="h-4 w-4 text-primary" />
                <span className="text-sm font-medium">{t("onboarding.selectTheme")}</span>
              </div>
              <Check className="h-4 w-4 text-primary" />
            </div>

            <div className="flex gap-2">
              <Button
                variant={theme === "light" ? "default" : "outline"}
                size="sm"
                className="flex-1 text-xs"
                onClick={() => theme === "dark" && toggleTheme()}
              >
                {t("onboarding.themeLight")}
              </Button>
              <Button
                variant={theme === "dark" ? "default" : "outline"}
                size="sm"
                className="flex-1 text-xs"
                onClick={() => theme === "light" && toggleTheme()}
              >
                {t("onboarding.themeDark")}
              </Button>
            </div>
          </div>

          {/* Card 4: Login (direct email+password) — OPTIONAL, so it sits
              LAST: the core "get started" cards (model/workspace/theme) come
              first, sign-in is a nice-to-have, not a gate. */}
          <div className={cn(
            "rounded-xl border border-border bg-card/50 p-4",
            "transition-all duration-200",
          )}>
            <div className="mb-3 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <LogIn className="h-4 w-4 text-primary" />
                <span className="text-sm font-medium">{t("onboarding.signIn")}</span>
                <span className="rounded bg-muted px-1.5 py-0.5 text-[9px] font-medium text-muted-foreground">
                  {t("onboarding.loginOptional")}
                </span>
              </div>
              {authUser && <UserCheck className="h-4 w-4 text-primary" />}
            </div>

            {authUser ? (
              <div className="space-y-2">
                <p className="text-xs text-foreground/80">
                  {t("onboarding.signedInAs", { name: authUser.username })}
                </p>
              </div>
            ) : (
              <form onSubmit={handlePasswordLogin} className="space-y-2">
                <p className="text-xs text-muted-foreground">{t("onboarding.signInHint")}</p>
                <Input
                  type="email"
                  autoComplete="email"
                  placeholder={t("sidebar.loginEmailPlaceholder")}
                  value={loginEmail}
                  onChange={(e) => setLoginEmail(e.target.value)}
                  disabled={authLoginLoading}
                  className="h-8 text-xs"
                />
                <Input
                  type="password"
                  autoComplete="current-password"
                  placeholder={t("sidebar.loginPasswordPlaceholder")}
                  value={loginPassword}
                  onChange={(e) => setLoginPassword(e.target.value)}
                  disabled={authLoginLoading}
                  className="h-8 text-xs"
                />
                {authError && authErrorKind !== "none" && (
                  <p className="text-[10px] text-destructive">
                    {authErrorKind !== "unknown"
                      ? t(`onboarding.authError.${authErrorKind}`)
                      : authError}
                  </p>
                )}
                <Button
                  type="submit"
                  size="sm"
                  className="w-full gap-1.5"
                  disabled={authLoginLoading}
                >
                  {authLoginLoading ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <LogIn className="h-3.5 w-3.5" />
                  )}
                  {authLoginLoading
                    ? t("sidebar.loginSubmitting")
                    : t("sidebar.loginSubmit")}
                </Button>
              </form>
            )}
          </div>
        </div>

        {/* Hint */}
        <p className="mt-4 text-center text-xs text-muted-foreground">
          {t("onboarding.tips")}
        </p>
      </div>

      {/* Bottom bar — the CTA stays clickable but states clearly when the
          user is skipping unfinished configuration. */}
      <div className="flex items-center justify-between border-t border-border px-6 py-3">
        <div className="flex items-center gap-2">
          {onBack && (
            <Button
              variant="ghost"
              size="sm"
              className="gap-1 text-xs text-muted-foreground"
              onClick={onBack}
            >
              <ChevronLeft className="h-3.5 w-3.5" />
              {t("onboarding.backToGuide")}
            </Button>
          )}
          <span className="text-xs text-muted-foreground">
            {allDone ? t("onboarding.ready") : t("onboarding.suggestOpenWorkspace")}
          </span>
        </div>
        <Button
          size="sm"
          className="bg-primary text-primary-foreground shadow-md shadow-primary/25 hover:scale-105"
          onClick={onComplete}
          title={allDone ? undefined : t("onboarding.skipHint")}
        >
          {allDone ? t("onboarding.startUsing") : t("onboarding.skipForNow")}
        </Button>
      </div>
    </div>
  );
}
