/**
 * PermissionsSettings — governance hub:
 * 1. Durable "always allow" grants (audit + single revoke + clear all)
 * 2. Settings rules (allow/deny/ask) with hot reload — no restart
 * 3. Plugin policy layer (available / blocked)
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Loader2, Plus, RefreshCw, RotateCcw, ShieldCheck, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { permissionApi, circuitBreakerApi, type PermissionGrant, type CircuitBreakerState } from "@/lib/tauri";
import type { PermissionMode } from "@/types";
import { cn } from "@/lib/utils";

interface RulesState {
  allow: string[];
  deny: string[];
  ask: string[];
}

const MODE_OPTIONS: { id: PermissionMode; label: string }[] = [
  { id: "read_only", label: "chat.modeReadOnly" },
  { id: "accept_edits", label: "chat.modeAcceptEdits" },
  { id: "full_access", label: "chat.modeFullAccess" },
];

/** ── Default permission mode (global) ─────────────────────── */
function DefaultModeSection() {
  const { t } = useTranslation();
  const [mode, setMode] = useState<PermissionMode | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void permissionApi
      .getMode()
      .then((m) => setMode(m ?? "accept_edits"))
      .catch(() => {
        /* best-effort — default mode used */
      });
  }, []);

  const select = async (next: PermissionMode) => {
    if (next === mode || saving) return;
    setSaving(true);
    try {
      await permissionApi.setMode(next); // no sessionId → global default
      setMode(next);
    } catch {
      // best-effort — mode stays on failure
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="space-y-3">
      <div>
        <h3 className="text-sm font-semibold">{t("settings.permissions.defaultModeTitle")}</h3>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {t("settings.permissions.defaultModeDesc")}
        </p>
      </div>
      <div className="flex flex-wrap gap-2">
        {MODE_OPTIONS.map((opt) => (
          <button
            key={opt.id}
            onClick={() => void select(opt.id)}
            disabled={saving}
            className={cn(
              "flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-xs transition-colors",
              mode === opt.id
                ? "border-primary/50 bg-primary/10 text-primary"
                : "border-border bg-background text-muted-foreground hover:bg-muted/40",
            )}
          >
            {mode === opt.id && <Check className="h-3.5 w-3.5" />}
            {t(opt.label)}
          </button>
        ))}
      </div>
    </section>
  );
}

/** ── Durable grants: audit + revoke ─────────────────────────── */
function GrantsSection() {
  const { t } = useTranslation();
  const [grants, setGrants] = useState<PermissionGrant[] | null>(null);

  const load = useCallback(async () => {
    try {
      setGrants(await permissionApi.listGrants());
    } catch {
      // Backend unavailable — keep last grants (null → empty section).
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (tool: string, pattern: string) => {
    try {
      await permissionApi.removeGrant(tool, pattern);
    } catch {
      // best-effort — keep the grant listed on failure
    }
    void load();
  };

  const clearAll = async () => {
    try {
      await permissionApi.clearGrants();
    } catch {
      // best-effort — keep grants on failure
    }
    void load();
  };

  return (
    <section className="space-y-3">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="flex items-center gap-1.5 text-sm font-semibold">
            <ShieldCheck className="h-4 w-4 text-primary/70" />
            {t("settings.permissions.grantsTitle")}
          </h3>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("settings.permissions.grantsDesc")}
          </p>
        </div>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="sm"
            className="h-7 gap-1.5 text-xs"
            onClick={() => void load()}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("settings.memory.refresh")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="h-7 gap-1.5 text-xs text-destructive hover:text-destructive"
            onClick={() => void clearAll()}
            disabled={!grants || grants.length === 0}
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t("settings.permissions.clearAll")}
          </Button>
        </div>
      </div>

      {grants === null ? (
        <div className="flex justify-center py-8">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : grants.length === 0 ? (
        <p className="rounded-md bg-muted/40 px-3 py-4 text-center text-xs text-muted-foreground">
          {t("settings.permissions.grantsEmpty")}
        </p>
      ) : (
        <div className="space-y-1.5">
          {grants.map((g) => (
            <div
              key={`${g.tool_name}:${g.pattern}`}
              className="flex items-center gap-2 rounded-md border border-border bg-background px-3 py-2"
            >
              <Badge variant="secondary" className="shrink-0 font-mono text-[10px]">
                {g.tool_name}
              </Badge>
              <code className="min-w-0 flex-1 truncate font-mono text-[11px]">{g.pattern}</code>
              <span className="shrink-0 text-[10px] text-muted-foreground">
                {new Date(g.created_at).toLocaleString()}
              </span>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={t("settings.permissions.revoke")}
                onClick={() => void remove(g.tool_name, g.pattern)}
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

/** ── Settings rules: allow / deny / ask, hot-reloaded ───────── */
function RuleEditor({
  kind,
  rules,
  onChange,
}: {
  kind: "allow" | "deny" | "ask";
  rules: string[];
  onChange: (kind: "allow" | "deny" | "ask", next: string[]) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");

  const add = () => {
    const trimmed = draft.trim();
    if (!trimmed || rules.includes(trimmed)) return;
    onChange(kind, [...rules, trimmed]);
    setDraft("");
  };

  const color =
    kind === "deny"
      ? "border-l-red-500/70"
      : kind === "allow"
        ? "border-l-emerald-500/70"
        : "border-l-amber-500/70";

  return (
    <div className="rounded-md border border-border bg-background p-3">
      <p className="mb-2 text-xs font-medium">
        {t(`settings.permissions.ruleKind.${kind}`)}
      </p>
      <div className="mb-2 space-y-1">
        {rules.length === 0 ? (
          <p className="text-[11px] text-muted-foreground">
            {t("settings.permissions.rulesEmpty")}
          </p>
        ) : (
          rules.map((r) => (
            <div
              key={r}
              className={cn(
                "flex items-center gap-2 rounded border-l-2 bg-muted/30 px-2 py-1",
                color,
              )}
            >
              <code className="min-w-0 flex-1 truncate font-mono text-[11px]">{r}</code>
              <button
                className="text-muted-foreground hover:text-destructive"
                onClick={() => onChange(kind, rules.filter((x) => x !== r))}
                aria-label={t("settings.permissions.removeRule")}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          ))
        )}
      </div>
      <div className="flex gap-2">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") add();
          }}
          placeholder={t("settings.permissions.rulePlaceholder")}
          className="h-7 min-w-0 flex-1 rounded border border-border bg-background px-2 font-mono text-[11px] outline-none focus:border-primary"
        />
        <Button variant="outline" size="sm" className="h-7 gap-1 text-xs" onClick={add}>
          <Plus className="h-3 w-3" />
          {t("settings.permissions.addRule")}
        </Button>
      </div>
    </div>
  );
}

function RulesSection() {
  const { t } = useTranslation();
  const [rules, setRules] = useState<RulesState | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        const view = await permissionApi.getRules();
        setRules({ allow: view.allow, deny: view.deny, ask: view.ask });
      } catch {
        /* best-effort — rules section stays empty */
      }
    })();
  }, []);

  const update = (kind: "allow" | "deny" | "ask", next: string[]) => {
    setRules((prev) => (prev ? { ...prev, [kind]: next } : prev));
    setSaved(false);
  };

  const save = async () => {
    if (!rules) return;
    setSaving(true);
    try {
      await permissionApi.setRules(rules.allow, rules.deny, rules.ask);
      setSaved(true);
    } catch {
      // best-effort — no success badge on failure
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="space-y-3">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-sm font-semibold">{t("settings.permissions.rulesTitle")}</h3>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("settings.permissions.rulesDesc")}
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 text-xs"
          onClick={() => void save()}
          disabled={!rules || saving}
        >
          {saving ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <ShieldCheck className="h-3.5 w-3.5" />
          )}
          {t("settings.permissions.saveRules")}
        </Button>
      </div>
      {saved && (
        <p className="text-[11px] text-emerald-600">
          {t("settings.permissions.rulesSaved")}
        </p>
      )}
      {rules === null ? (
        <div className="flex justify-center py-8">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : (
        <div className="grid gap-3 md:grid-cols-3">
          <RuleEditor kind="deny" rules={rules.deny} onChange={update} />
          <RuleEditor kind="allow" rules={rules.allow} onChange={update} />
          <RuleEditor kind="ask" rules={rules.ask} onChange={update} />
        </div>
      )}
    </section>
  );
}

/** ── Plugin policy: available / blocked ─────────────────────── */
function PolicySection() {
  const { t } = useTranslation();
  const [policy, setPolicy] = useState<Record<string, string> | null>(null);
  const [draftId, setDraftId] = useState("");
  const [draftAction, setDraftAction] = useState<"blocked" | "available">("blocked");

  const load = useCallback(async () => {
    try {
      setPolicy(await permissionApi.listPluginPolicy());
    } catch {
      /* best-effort — policy section stays empty */
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const apply = async (id: string, action: string) => {
    try {
      await permissionApi.setPluginPolicy(id, action);
    } catch {
      /* best-effort — keep current policy on failure */
    }
    void load();
  };

  const add = () => {
    const id = draftId.trim();
    if (!id) return;
    void apply(id, draftAction);
    setDraftId("");
  };

  const entries = Object.entries(policy ?? {}).sort(([a], [b]) => a.localeCompare(b));

  return (
    <section className="space-y-3">
      <div>
        <h3 className="text-sm font-semibold">{t("settings.permissions.policyTitle")}</h3>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {t("settings.permissions.policyDesc")}
        </p>
      </div>
      <div className="flex gap-2">
        <input
          value={draftId}
          onChange={(e) => setDraftId(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") add();
          }}
          placeholder={t("settings.permissions.policyPlaceholder")}
          className="h-7 min-w-0 flex-1 rounded border border-border bg-background px-2 font-mono text-[11px] outline-none focus:border-primary"
        />
        <select
          value={draftAction}
          onChange={(e) => setDraftAction(e.target.value as "blocked" | "available")}
          className="h-7 rounded border border-border bg-background px-2 text-[11px] outline-none"
        >
          <option value="blocked">blocked</option>
          <option value="available">available</option>
        </select>
        <Button variant="outline" size="sm" className="h-7 gap-1 text-xs" onClick={add}>
          <Plus className="h-3 w-3" />
          {t("settings.permissions.addRule")}
        </Button>
      </div>
      {policy === null ? (
        <div className="flex justify-center py-6">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : entries.length === 0 ? (
        <p className="rounded-md bg-muted/40 px-3 py-4 text-center text-xs text-muted-foreground">
          {t("settings.permissions.policyEmpty")}
        </p>
      ) : (
        <div className="space-y-1.5">
          {entries.map(([id, action]) => (
            <div
              key={id}
              className="flex items-center gap-2 rounded-md border border-border bg-background px-3 py-2"
            >
              <code className="min-w-0 flex-1 truncate font-mono text-[11px]">{id}</code>
              <Badge
                variant={action === "blocked" ? "destructive" : "secondary"}
                className="text-[9px]"
              >
                {action}
              </Badge>
              <button
                className="text-[10px] text-muted-foreground hover:text-foreground"
                onClick={() =>
                  void apply(id, action === "blocked" ? "available" : "blocked")
                }
              >
                {t(
                  action === "blocked"
                    ? "settings.permissions.unblock"
                    : "settings.permissions.block",
                )}
              </button>
              <button
                className="text-muted-foreground hover:text-destructive"
                onClick={() => void apply(id, "available")}
                aria-label={t("settings.permissions.removeRule")}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

/** ── Auto-Review: independent reviewer for gray-zone asks ──── */
function AutoReviewSection() {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void permissionApi
      .getAutoReviewEnabled()
      .then(setEnabled)
      .catch(() => {
        /* best-effort — toggle stays off */
      });
  }, []);

  const toggle = async (next: boolean) => {
    setSaving(true);
    try {
      await permissionApi.setAutoReviewEnabled(next);
      setEnabled(next);
    } catch {
      // best-effort — state stays on failure
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="space-y-2">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold">{t("settings.permissions.autoReviewTitle")}</h3>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("settings.permissions.autoReviewDesc")}
          </p>
        </div>
        <Switch
          checked={enabled ?? false}
          disabled={enabled === null || saving}
          onCheckedChange={(next) => void toggle(next)}
        />
      </div>
      <p className="rounded-md bg-muted/40 px-3 py-2 text-[11px] text-muted-foreground">
        {t("settings.permissions.autoReviewHint")}
      </p>
    </section>
  );
}

/** ── LLM circuit breakers: observe + reset tripped providers ── */
function CircuitBreakersSection() {
  const { t } = useTranslation();
  const [breakers, setBreakers] = useState<CircuitBreakerState[]>([]);
  const [loading, setLoading] = useState(true);
  const [resetError, setResetError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        setBreakers(await circuitBreakerApi.getStates());
      } catch {
        setBreakers([]);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  const handleReset = async (provider: string) => {
    setResetError(null);
    try {
      await circuitBreakerApi.reset(provider);
      setBreakers(await circuitBreakerApi.getStates());
    } catch {
      setResetError(t("settings.general.circuitResetFailed", { defaultValue: "重置失败" }));
    }
  };

  return (
    <section className="space-y-3">
      <div>
        <h3 className="text-sm font-semibold">{t("settings.general.circuitBreakers")}</h3>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {t("settings.general.circuitBreakersDesc", { defaultValue: "LLM 提供商熔断器状态与重置" })}
        </p>
      </div>
      {resetError && <p className="text-[10px] text-destructive">{resetError}</p>}
      {loading ? (
        <div className="flex justify-center py-6">
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        </div>
      ) : breakers.length === 0 ? (
        <p className="rounded-md bg-muted/40 px-3 py-2 text-[11px] text-muted-foreground">
          {t("settings.general.noProviders")}
        </p>
      ) : (
        <div className="space-y-1.5">
          {breakers.map((b) => (
            <div
              key={b.provider}
              className="flex items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2"
            >
              <div className="flex min-w-0 items-center gap-2">
                <span
                  className={cn(
                    "h-2 w-2 shrink-0 rounded-full",
                    b.state === "closed"
                      ? "bg-emerald-500"
                      : b.state === "half_open"
                        ? "bg-amber-500"
                        : "bg-destructive",
                  )}
                />
                <span className="truncate text-xs font-medium">{b.provider}</span>
                <span className="text-[10px] text-muted-foreground">
                  {t(`settings.general.circuitState.${b.state}`)}
                </span>
              </div>
              {b.state !== "closed" && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-6 shrink-0 gap-1 px-2 text-[10px] text-muted-foreground hover:text-foreground"
                  onClick={() => void handleReset(b.provider)}
                >
                  <RotateCcw className="h-3 w-3" />
                  {t("settings.general.circuitReset")}
                </Button>
              )}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

export function PermissionsSettings() {
  return (
    <div className="space-y-8">
      <DefaultModeSection />
      <GrantsSection />
      <RulesSection />
      <PolicySection />
      <AutoReviewSection />
      <CircuitBreakersSection />
    </div>
  );
}
