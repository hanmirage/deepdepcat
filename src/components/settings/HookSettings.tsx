/**
 * HookSettings — hook management settings page.
 *
 * Lists user-level hooks from hooks.toml. Allows adding hooks (event +
 * type + payload), toggling them, and deleting them.
 */

import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, Plus, Trash2, Loader2, Zap, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { SettingSelect } from "@/components/settings/SettingSelect";
import { hookApi, type HookDefinition, type HookPreview, type HookView } from "@/lib/tauri";
import { cn } from "@/lib/utils";

export interface HookSettingsProps {
  className?: string;
}

/** Hook types selectable in the UI. */
const HOOK_TYPES = [
  { value: "command", label: "Command" },
  { value: "prompt", label: "Prompt (LLM)" },
  { value: "agent", label: "Agent" },
  { value: "http", label: "HTTP" },
];

/** Get the payload field label for a hook type. */
function payloadLabel(type: string): string {
  switch (type) {
    case "command":
      return "Command";
    case "prompt":
      return "Prompt";
    case "agent":
      return "Prompt";
    case "http":
      return "URL";
    default:
      return "Payload";
  }
}

/** Stable identity for a hook across list reorders/deletes — the list index
 *  shifts after a delete, so index-based keys would show the WRONG preview
 *  and wrong labels for every row below the deleted one. */
function hookKey(hook: HookDefinition): string {
  const content = hook.command ?? hook.prompt ?? hook.url ?? "";
  return `${hook.event}|${hook.type}|${content}`;
}

export function HookSettings({ className }: HookSettingsProps) {
  const { t } = useTranslation();
  const [hooks, setHooks] = useState<HookView[]>([]);
  const [events, setEvents] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [showAdd, setShowAdd] = useState(false);
  const [saving, setSaving] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [armedDelete, setArmedDelete] = useState<string | null>(null);

  // Add-form state
  const [newEvent, setNewEvent] = useState("PreToolUse");
  const [newType, setNewType] = useState("command");
  const [newPayload, setNewPayload] = useState("");
  const [newCondition, setNewCondition] = useState("");
  const [newTimeout, setNewTimeout] = useState("5000");
  const [projectHooks, setProjectHooks] = useState<HookView[]>([]);
  const [projectHooksEnabled, setProjectHooksEnabled] = useState(false);

  const loadHooks = useCallback(async () => {
    setLoading(true);
    try {
      const [list, evts, project, enabled] = await Promise.all([
        hookApi.list(),
        hookApi.listEvents(),
        hookApi.listProjectHooks(),
        hookApi.getProjectHooksEnabled(),
      ]);
      setHooks(list);
      setProjectHooks(project);
      setProjectHooksEnabled(enabled);
      // Default the add-form event to the first available one only when the
      // current selection isn't offered anymore.
      setNewEvent((prev) => (evts.length > 0 && !evts.includes(prev) ? evts[0] : prev));
      setEvents(evts);
      // Pre-fetch redacted previews for every hook so the rows never render
      // raw secrets (URL query tokens, embedded API keys) — the backend
      // expands env vars and masks sensitive values before returning.
      const results = await Promise.allSettled(
        list.map((hook) => hookApi.preview(hook)),
      );
      const previews: Record<string, HookPreview> = {};
      list.forEach((hook, idx) => {
        const r = results[idx];
        if (r.status === "fulfilled") previews[hookKey(hook)] = r.value;
      });
      setPreviews(previews);
    } catch {
      setHooks([]);
      setActionError(t("settings.hooksLoadFailed", { defaultValue: "加载 Hook 失败" }));
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void loadHooks();
  }, [loadHooks]);

  const handleAdd = useCallback(async () => {
    if (!newPayload.trim()) return;
    setSaving(true);
    setActionError(null);
    try {
      await hookApi.save({
        event: newEvent,
        type: newType,
        command: newType === "command" ? newPayload.trim() : null,
        prompt: newType === "prompt" || newType === "agent" ? newPayload.trim() : null,
        url: newType === "http" ? newPayload.trim() : null,
        condition: newCondition.trim() || null,
        timeout_ms: Math.min(Math.max(parseInt(newTimeout, 10) || 5000, 100), 600_000),
        enabled: true,
      });
      setNewPayload("");
      setNewCondition("");
      setShowAdd(false);
      void loadHooks();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }, [newEvent, newType, newPayload, newCondition, newTimeout, loadHooks]);

  const handleDelete = useCallback(
    async (hook: HookDefinition) => {
      const key = hookKey(hook);
      // Two-step confirm: first click arms, second click (within 3s) deletes.
      // Hooks execute commands — a mis-click here has real consequences.
      if (armedDelete !== key) {
        setArmedDelete(key);
        setTimeout(() => setArmedDelete((cur) => (cur === key ? null : cur)), 3000);
        return;
      }
      setArmedDelete(null);
      setActionError(null);
      try {
        const content =
          hook.command ?? hook.prompt ?? hook.url ?? "";
        await hookApi.delete(hook.event, hook.type, content);
        setHooks((prev) => prev.filter((h) => hookKey(h) !== key));
      } catch (e) {
        setActionError(e instanceof Error ? e.message : String(e));
      }
    },
    [armedDelete],
  );

  const handleToggle = useCallback(
    async (hook: HookDefinition, enabled: boolean) => {
      setActionError(null);
      try {
        await hookApi.save({ ...hook, enabled });
        // Optimistic local update — a full loadHooks() re-fetches the
        // previews of EVERY hook (N round-trips) on a single toggle.
        setHooks((prev) =>
          prev.map((h) => (hookKey(h) === hookKey(hook) ? { ...h, enabled } : h)),
        );
      } catch (e) {
        setActionError(e instanceof Error ? e.message : String(e));
      }
    },
    [],
  );

  const handleProjectToggle = useCallback(async (enabled: boolean) => {
    setProjectHooksEnabled(enabled);
    try {
      await hookApi.setProjectHooksEnabled(enabled);
      // Re-fetch the audit list so the UI reflects the live state.
      const project = await hookApi.listProjectHooks();
      setProjectHooks(project);
    } catch {
      setProjectHooksEnabled(!enabled);
      setActionError(t("settings.hooksLoadFailed", { defaultValue: "保存项目 Hook 开关失败" }));
    }
  }, [t]);

  /** Short human label for a hook. Uses the REDACTED preview when available
   *  so raw secrets never render in the list. */
  const hookLabel = (hook: HookDefinition, key: string): string => {
    const preview = previews[key];
    if (preview) {
      const p = hook.command !== null && hook.command !== undefined
        ? preview.command
        : hook.url !== null && hook.url !== undefined
          ? preview.url
          : preview.prompt;
      if (p) return p.length > 60 ? p.slice(0, 60) + "..." : p;
    }
    const payload = hook.command ?? hook.prompt ?? hook.url ?? "";
    return payload.length > 60 ? payload.slice(0, 60) + "..." : payload;
  };

  // ── 变量展开预览：展开环境变量并脱敏，仅展示、不执行 ──
  const [previews, setPreviews] = useState<Record<string, HookPreview>>({});
  const [previewLoading, setPreviewLoading] = useState<Record<string, boolean>>({});

  const togglePreview = useCallback(async (hook: HookDefinition, key: string) => {
    if (previews[key]) {
      setPreviews((s) => {
        const next = { ...s };
        delete next[key];
        return next;
      });
      return;
    }
    setPreviewLoading((s) => ({ ...s, [key]: true }));
    try {
      const preview = await hookApi.preview(hook);
      setPreviews((s) => ({ ...s, [key]: preview }));
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setPreviewLoading((s) => ({ ...s, [key]: false }));
    }
  }, [previews]);

  const previewText = (hook: HookDefinition, p: HookPreview): string =>
    hook.command !== null && hook.command !== undefined
      ? p.command ?? ""
      : hook.url !== null && hook.url !== undefined
        ? p.url ?? ""
        : p.prompt ?? "";

  return (
    <div className={cn("space-y-4", className)}>
      {actionError && (
        <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2">
          <p className="min-w-0 flex-1 break-words text-[11px] text-destructive">{actionError}</p>
          <button
            onClick={() => setActionError(null)}
            className="shrink-0 rounded p-0.5 text-destructive/70 hover:bg-destructive/10 hover:text-destructive"
            aria-label={t("common.close")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}
      <div className="flex items-center justify-between">
        <p className="text-xs text-muted-foreground">
          {t("settings.hooksDesc")}
        </p>
        <Button
          variant="outline"
          size="sm"
          className="h-8 gap-1 text-xs"
          onClick={() => setShowAdd(!showAdd)}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("common.add")}
        </Button>
      </div>
      <p className="rounded-md bg-muted/40 px-3 py-2 text-[10px] text-muted-foreground">
        {t("settings.hooksTrustHint", {
          defaultValue:
            "Hook 必须先被信任才会执行（按内容指纹记忆，修改后需重新信任）。在设置里保存即视为信任；项目 Hook 需在这里点「信任」。",
        })}
      </p>

      {/* ── Project hooks: opt-in master switch + read-only audit ── */}
      <section className="rounded-lg border border-border bg-card p-3">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs font-medium">
              {t("settings.hooksProjectTitle", { defaultValue: "项目级 Hook（.deepdepcat/hooks.toml）" })}
            </p>
            <p className="mt-0.5 text-[10px] text-muted-foreground">
              {t("settings.hooksProjectDesc", {
                defaultValue:
                  "项目 Hook 会执行任意命令，默认关闭。从仓库克隆的项目必须由你明确开启后才会生效，且这里只读展示、不可编辑。",
              })}
            </p>
          </div>
          <Switch
            checked={projectHooksEnabled}
            onCheckedChange={(v) => void handleProjectToggle(v)}
            aria-label={t("settings.hooksProjectToggle", { defaultValue: "启用项目 Hook" })}
          />
        </div>
        {projectHooks.length > 0 ? (
          <div className="mt-2 space-y-1">
            {projectHooks.map((hook, idx) => (
              <div
                key={`${hook.event}|${hook.type}|${hook.command ?? hook.prompt ?? hook.url ?? ""}|${idx}`}
                className="flex items-center gap-2 rounded border border-border bg-muted/30 px-2.5 py-1.5"
              >
                <span className="rounded bg-secondary px-1.5 py-0.5 font-mono text-[10px] text-secondary-foreground">
                  {hook.event}
                </span>
                <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                  {hook.type}
                </span>
                <button
                  onClick={() =>
                    void (hook.trusted
                      ? hookApi
                          .untrust(hook.fingerprint)
                          .then(loadHooks)
                          .catch(() => {
                            /* best-effort — list stays as-is on failure */
                          })
                      : hookApi
                          .trust(hook.fingerprint)
                          .then(loadHooks)
                          .catch(() => {
                            /* best-effort — list stays as-is on failure */
                          }))
                  }
                  className={cn(
                    "shrink-0 rounded px-1.5 py-0.5 text-[9px] font-medium",
                    hook.trusted
                      ? "bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20"
                      : "bg-amber-500/10 text-amber-700 hover:bg-amber-500/20 dark:text-amber-400",
                  )}
                  title={
                    hook.trusted
                      ? t("settings.hooksRevokeTrust", { defaultValue: "撤销信任（停止执行）" })
                      : t("settings.hooksTrust", { defaultValue: "信任此 Hook" })
                  }
                >
                  {hook.trusted
                    ? t("settings.hooksTrusted", { defaultValue: "已信任" })
                    : t("settings.hooksUntrusted", { defaultValue: "未信任 · 点击信任" })}
                </button>
                <p className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
                  {hookLabel(hook, `project-${idx}`)}
                </p>
                {!projectHooksEnabled && (
                  <span className="shrink-0 rounded bg-amber-500/10 px-1.5 py-0.5 text-[9px] font-medium text-amber-700 dark:text-amber-400">
                    {t("settings.hooksProjectDisabled", { defaultValue: "未启用" })}
                  </span>
                )}
              </div>
            ))}
          </div>
        ) : (
          <p className="mt-2 rounded bg-muted/30 px-2.5 py-2 text-[10px] text-muted-foreground">
            {t("settings.hooksProjectEmpty", { defaultValue: "当前工作区没有 .deepdepcat/hooks.toml" })}
          </p>
        )}
      </section>

      {showAdd && (
        <div className="space-y-3 rounded-lg border border-border bg-card p-3">
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">
                {t("settings.hooksEvent")}
              </label>
              <SettingSelect
                value={newEvent}
                onChange={(v) => setNewEvent(v)}
                options={events.map((e) => ({ value: e, label: e }))}
                className="w-full"
              />
            </div>
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">
                {t("settings.hooksType")}
              </label>
              <SettingSelect
                value={newType}
                onChange={(v) => setNewType(v)}
                options={HOOK_TYPES}
                className="w-full"
              />
            </div>
          </div>

          <div>
            <label className="mb-1 block text-[10px] text-muted-foreground">
              {payloadLabel(newType)}
            </label>
            <Input
              value={newPayload}
              onChange={(e) => setNewPayload(e.target.value)}
              placeholder={
                newType === "command"
                  ? "echo 'blocked' && exit 1"
                  : newType === "http"
                    ? "https://example.com/webhook"
                    : "Is this action safe? Reply ALLOW or DENY:reason"
              }
              className="h-8 text-xs"
            />
          </div>

          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">
                {t("settings.hooksCondition")}
              </label>
              <Input
                value={newCondition}
                onChange={(e) => setNewCondition(e.target.value)}
                placeholder={'tool_name == "bash"'}
                className="h-8 text-xs"
              />
            </div>
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">
                {t("settings.hooksTimeout")}
              </label>
              <Input
                type="number"
                min={100}
                max={600000}
                value={newTimeout}
                onChange={(e) => setNewTimeout(e.target.value)}
                className="h-8 text-xs"
              />
            </div>
          </div>

          <div className="flex gap-2">
            <Button
              size="sm"
              className="h-8 text-xs"
              disabled={!newPayload.trim() || saving}
              onClick={() => void handleAdd()}
            >
              {saving ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Plus className="h-3.5 w-3.5" />
              )}
              {t("common.save")}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="h-8 text-xs"
              onClick={() => setShowAdd(false)}
            >
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      )}

      {loading ? (
        <div className="flex items-center gap-2 py-4 text-xs text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          {t("common.loading")}
        </div>
      ) : hooks.length === 0 ? (
        <p className="rounded-md bg-muted/40 px-3 py-3 text-[11px] text-muted-foreground">
          {t("settings.hooksEmpty")}
        </p>
      ) : (
        <div className="space-y-1.5">
          {hooks.map((hook) => {
            const key = hookKey(hook);
            return (
            <div
              key={key}
              className="flex items-center gap-2 rounded-md border border-border bg-background px-2.5 py-2"
            >
              <Zap
                className={cn(
                  "h-3.5 w-3.5 shrink-0",
                  hook.enabled ? "text-primary" : "text-muted-foreground/40",
                )}
              />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="rounded bg-secondary px-1.5 py-0.5 font-mono text-[10px] text-secondary-foreground">
                    {hook.event}
                  </span>
                  <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                    {hook.type}
                  </span>
                  <button
                    onClick={() =>
                      void (hook.trusted
                        ? hookApi
                            .untrust(hook.fingerprint)
                            .then(loadHooks)
                            .catch(() => {
                              /* best-effort — list stays as-is on failure */
                            })
                        : hookApi
                            .trust(hook.fingerprint)
                            .then(loadHooks)
                            .catch(() => {
                              /* best-effort — list stays as-is on failure */
                            }))
                    }
                    className={cn(
                      "shrink-0 rounded px-1.5 py-0.5 text-[9px] font-medium",
                      hook.trusted
                        ? "bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20"
                        : "bg-amber-500/10 text-amber-700 hover:bg-amber-500/20 dark:text-amber-400",
                    )}
                    title={
                      hook.trusted
                        ? t("settings.hooksRevokeTrust", { defaultValue: "撤销信任（停止执行）" })
                        : t("settings.hooksTrust", { defaultValue: "信任此 Hook" })
                    }
                  >
                    {hook.trusted
                      ? t("settings.hooksTrusted", { defaultValue: "已信任" })
                      : t("settings.hooksUntrusted", { defaultValue: "未信任 · 点击信任" })}
                  </button>
                </div>
                <p className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
                  {hookLabel(hook, key)}
                </p>
                {hook.condition && (
                  <p className="truncate text-[10px] text-muted-foreground/60">
                    if {hook.condition}
                  </p>
                )}
                {previews[key] && (
                  <pre className="mt-1.5 max-h-24 overflow-auto whitespace-pre-wrap rounded bg-muted/50 p-1.5 font-mono text-[10px] leading-relaxed text-muted-foreground">
                    {previewText(hook, previews[key]) || "(no expandable fields)"}
                  </pre>
                )}
              </div>
              <button
                onClick={() => void togglePreview(hook, key)}
                className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:text-foreground"
                title={t("settings.hooksPreview")}
              >
                {previewLoading[key] ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : previews[key] ? (
                  <ChevronDown className="h-3.5 w-3.5" />
                ) : (
                  <ChevronRight className="h-3.5 w-3.5" />
                )}
              </button>
              <Switch
                checked={hook.enabled}
                onCheckedChange={(v) => void handleToggle(hook, v)}
              />
              <button
                onClick={() => void handleDelete(hook)}
                className={cn(
                  "shrink-0 rounded p-1 transition-colors",
                  armedDelete === key
                    ? "bg-destructive/10 text-destructive"
                    : "text-muted-foreground hover:text-destructive",
                )}
                aria-label={
                  armedDelete === key
                    ? t("settings.hooksConfirmDelete", { defaultValue: "再次点击确认删除" })
                    : t("common.delete")
                }
                title={
                  armedDelete === key
                    ? t("settings.hooksConfirmDelete", { defaultValue: "再次点击确认删除" })
                    : undefined
                }
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
