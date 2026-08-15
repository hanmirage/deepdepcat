/**
 * ModelProviderSettings — LLM provider configuration with model fetching.
 *
 * Features:
 * - Provider cards with expand/collapse, inline editing
 * - "Fetch models" button: calls GET {baseUrl}/models to auto-populate model list
 * - Loading spinner, error message, success count badge
 * - Add provider form with "Fetch" to test + populate before saving
 * - Supports OpenAI, Anthropic, and Gemini API formats
 *
 * State managed by useSettingsStore.
 */

import { useState, useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  useSettingsStore,
  contextWindowOptions,
  type ApiFormat,
  type ModelConfig,
} from "@/stores/settingsStore";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Plus, Trash2, RefreshCw, ChevronDown, ChevronRight,
  Loader2, Download, AlertCircle, X,
} from "lucide-react";
import { SettingSelect } from "@/components/settings/SettingSelect";
import { ModelProviderAddForm, renderFetchStatus, type FetchState } from "@/components/settings/ModelProviderAddForm";
import { SecretInput } from "@/components/settings/ModelProviderAddForm";
import { circuitBreakerApi, type CircuitBreakerState } from "@/lib/tauri";
import { cn } from "@/lib/utils";

export interface ModelProviderSettingsProps {
  className?: string;
}

/** Password input with a show/hide toggle — used for every API key field. */

/** Map settingsStore error codes to localized copy; unknown errors pass through. */
function translateFetchError(
  err: string | null | undefined,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  switch (err) {
    case "PROVIDER_NOT_FOUND":
      return t("settings.modelProviders.errProviderNotFound");
    case "MISSING_BASE_URL":
      return t("settings.modelProviders.errMissingBaseUrl");
    case "NO_MODELS_FETCHED":
      return t("settings.modelProviders.errNoModels");
    default:
      return err ?? t("settings.modelProviders.fetchFailed");
  }
}

export function ModelProviderSettings({ className }: ModelProviderSettingsProps) {
  const { t } = useTranslation();
  const providers = useSettingsStore((s) => s.providers);
  const addProvider = useSettingsStore((s) => s.addProvider);
  const updateProvider = useSettingsStore((s) => s.updateProvider);
  const removeProvider = useSettingsStore((s) => s.removeProvider);
  const toggleProvider = useSettingsStore((s) => s.toggleProvider);
  const addModel = useSettingsStore((s) => s.addModel);
  const removeModel = useSettingsStore((s) => s.removeModel);
  const updateModel = useSettingsStore((s) => s.updateModel);
  const fetchModels = useSettingsStore((s) => s.fetchModels);
  const fetchModelsByConfig = useSettingsStore((s) => s.fetchModelsByConfig);
  const lastSyncError = useSettingsStore((s) => s.lastSyncError);
  const clearSyncError = useSettingsStore((s) => s.clearSyncError);

  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);

  // Expand the DeepSeek provider by default (it's pre-configured — user just
  // needs to paste an API key). Only on first mount.
  useEffect(() => {
    const deepseek = useSettingsStore.getState().providers.find((p) => p.id === "deepseek");
    if (deepseek) setExpandedId(deepseek.id);
  }, []);

  // Add form state
  const [newName, setNewName] = useState("");
  const [newUrl, setNewUrl] = useState("");
  const [newKey, setNewKey] = useState("");
  const [newFormat, setNewFormat] = useState<ApiFormat>("openai");
  const [newModels, setNewModels] = useState<ModelConfig[]>([]);

  // Fetch state per provider
  const [fetchStates, setFetchStates] = useState<Record<string, FetchState>>({});
  const [newFetchState, setNewFetchState] = useState<FetchState>({ loading: false, error: null, success: null });

  // ── Timers ─────────────────────────────────────────────────
  // Clear previous success-badge timers before scheduling a new one, so a
  // slow second fetch can't have its badge wiped by the first fetch's timer.
  const fetchTimersRef = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const newFetchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clear every pending timer on unmount so none fires after disposal.
  // The refs are read AT cleanup time (not captured on mount — capturing the
  // .current value would always be the initial null and leak the timers set
  // later by requestDelete / fetch actions).
  useEffect(() => {
    return () => {
      for (const timer of Object.values(fetchTimersRef.current)) clearTimeout(timer);
      if (deleteTimerRef.current) clearTimeout(deleteTimerRef.current);
      if (newFetchTimerRef.current) clearTimeout(newFetchTimerRef.current);
    };
  }, []);

  // ── Two-step delete confirmation ───────────────────────────
  // First click arms the button (turns red), second click deletes; auto-
  // disarms after 3 seconds. Prevents accidental provider/model loss.
  type DeleteTarget = { kind: "provider"; id: string } | { kind: "model"; providerId: string; modelId: string };
  const [armedDelete, setArmedDelete] = useState<DeleteTarget | null>(null);
  const deleteTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const targetKey = (t: DeleteTarget) =>
    t.kind === "provider" ? `p:${t.id}` : `m:${t.providerId}:${t.modelId}`;
  const requestDelete = (target: DeleteTarget) => {
    if (armedDelete && targetKey(armedDelete) === targetKey(target)) {
      if (target.kind === "provider") removeProvider(target.id);
      else removeModel(target.providerId, target.modelId);
      setArmedDelete(null);
      if (deleteTimerRef.current) clearTimeout(deleteTimerRef.current);
      return;
    }
    setArmedDelete(target);
    if (deleteTimerRef.current) clearTimeout(deleteTimerRef.current);
    deleteTimerRef.current = setTimeout(() => setArmedDelete(null), 3000);
  };
  const isArmed = (target: DeleteTarget) =>
    armedDelete !== null && targetKey(armedDelete) === targetKey(target);

  // ── Inline "add model" form (replaces window.prompt) ───────
  const [manualAddFor, setManualAddFor] = useState<string | null>(null);
  const [manualAddName, setManualAddName] = useState("");
  const [manualAddCtx, setManualAddCtx] = useState("");
  const commitManualAdd = (providerId: string) => {
    const name = manualAddName.trim();
    if (name) {
      // The entered value is the API model name (e.g. "deepseek-v4-flash") —
      // it is sent verbatim as `model`, so a synthetic id would be rejected.
      const ctx = Math.min(Math.max(parseInt(manualAddCtx, 10) || 1_000_000, 1000), 100_000_000);
      addModel(providerId, { id: name, name, contextWindow: ctx });
    }
    setManualAddFor(null);
    setManualAddName("");
    setManualAddCtx("");
  };

  // Circuit breaker states (polled every 5 seconds)
  const [cbStates, setCbStates] = useState<Record<string, CircuitBreakerState>>({});
  useEffect(() => {
    const fetch = () => circuitBreakerApi.getStates().then((arr) => {
      const map: Record<string, CircuitBreakerState> = {};
      for (const item of arr) map[item.provider] = item;
      setCbStates(map);
    }).catch(() => {});
    fetch();
    const id = setInterval(fetch, 5000);
    return () => clearInterval(id);
  }, []);

  const allFormatOptions = [
    { value: "openai", label: t("settings.modelProviders.formatOpenai") },
    { value: "anthropic", label: t("settings.modelProviders.formatAnthropic") },
    { value: "responses", label: t("settings.modelProviders.formatResponses") },
    { value: "gemini", label: t("settings.modelProviders.formatGemini") },
    { value: "custom", label: t("settings.modelProviders.formatCustom") },
  ];
  // DeepSeek speaks exactly three wire protocols — no Gemini/custom clutter.
  // Default stays OpenAI compatible; picking Responses restricts the model
  // picker to deepseek-v4-flash (backend capability match).
  const formatOptionsFor = (providerId: string) =>
    providerId === "deepseek"
      ? allFormatOptions.filter((o) => ["openai", "anthropic", "responses"].includes(o.value))
      : allFormatOptions;

  // ── Fetch handler for existing provider ──────────────────
  const handleFetch = useCallback(async (providerId: string) => {
    const prevTimer = fetchTimersRef.current[providerId];
    if (prevTimer) clearTimeout(prevTimer);
    setFetchStates((prev) => ({ ...prev, [providerId]: { loading: true, error: null, success: null } }));
    const result = await fetchModels(providerId);
    setFetchStates((prev) => ({
      ...prev,
      [providerId]: result.success
        ? { loading: false, error: null, success: t("settings.modelProviders.fetchSuccess", { count: result.count }) }
        : { loading: false, error: translateFetchError(result.error, t), success: null },
    }));
    if (result.success) {
      fetchTimersRef.current[providerId] = setTimeout(() => {
        setFetchStates((prev) => ({ ...prev, [providerId]: { loading: false, error: null, success: null } }));
        delete fetchTimersRef.current[providerId];
      }, 3000);
    }
  }, [fetchModels, t]);

  // ── Fetch handler for add form ───────────────────────────
  const handleNewFetch = useCallback(async () => {
    if (newFetchTimerRef.current) clearTimeout(newFetchTimerRef.current);
    if (!newUrl) {
      setNewFetchState({ loading: false, error: t("settings.modelProviders.fillBaseUrlFirst"), success: null });
      return;
    }
    setNewFetchState({ loading: true, error: null, success: null });
    const result = await fetchModelsByConfig(newUrl, newKey, newFormat);
    if (result.success && result.models.length > 0) {
      setNewModels(result.models);
      setNewFetchState({ loading: false, error: null, success: t("settings.modelProviders.fetchSuccess", { count: result.models.length }) });
      newFetchTimerRef.current = setTimeout(() => setNewFetchState({ loading: false, error: null, success: null }), 3000);
    } else {
      setNewFetchState({ loading: false, error: translateFetchError(result.error, t), success: null });
    }
  }, [newUrl, newKey, newFormat, fetchModelsByConfig, t]);

  // ── Add provider ─────────────────────────────────────────
  const handleAddProvider = () => {
    const name = newName.trim();
    const url = newUrl.trim();
    if (!name || !url) return;
    // Duplicate-name guard: a second provider with the same name/id would
    // collide in the backend config and in the model picker.
    const dup = useSettingsStore.getState().providers.find(
      (p) => p.name.toLowerCase() === name.toLowerCase() || p.id === name,
    );
    if (dup) {
      setNewFetchState({
        loading: false,
        error: t("settings.modelProviders.duplicateProvider", { name: dup.name }),
        success: null,
      });
      return;
    }
    addProvider({
      name,
      baseUrl: url,
      apiKey: newKey.trim(),
      apiFormat: newFormat,
    }, newModels);
    setNewName(""); setNewUrl(""); setNewKey(""); setNewFormat("openai"); setNewModels([]);
    setNewFetchState({ loading: false, error: null, success: null });
    setShowAddForm(false);
  };

  // ── Fetch status badge ───────────────────────────────────

  return (
    <div className={cn("space-y-4", className)}>
      {lastSyncError && (
        <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2">
          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
          <p className="min-w-0 flex-1 break-words text-[11px] text-destructive">
            {t("settings.modelProviders.syncFailed", { error: lastSyncError })}
          </p>
          <button
            onClick={clearSyncError}
            className="shrink-0 rounded p-0.5 text-destructive/70 transition-colors hover:bg-destructive/10 hover:text-destructive"
            aria-label={t("common.close")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}
      <div className="flex items-start justify-between">
        <p className="text-xs text-muted-foreground">
          {t("settings.modelProviders.headerDesc")}
        </p>
      </div>

      {/* ── Provider cards (DeepSeek pre-configured, expanded by default) ── */}
      {providers.length > 0 && (
        <div className="space-y-3">
          {providers.map((provider) => {
            const isExpanded = expandedId === provider.id;
            const fs = fetchStates[provider.id] ?? { loading: false, error: null, success: null };
            return (
              <div key={provider.id} className="overflow-hidden rounded-lg border border-border">
                <div className="flex items-center gap-2 p-3">
                  <button
                    onClick={() => setExpandedId(isExpanded ? null : provider.id)}
                    className="text-muted-foreground hover:text-foreground"
                    aria-label={isExpanded ? t("common.collapse") : t("common.expand")}
                  >
                    {isExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                  </button>
                  <div className="flex-1">
                    <div className="flex items-center gap-1.5">
                      <p className="text-sm font-semibold">{provider.name}</p>
                      {(() => {
                        // Circuit breaker key = provider ID (the backend maps
                        // provider.id → its config name); the display name is
                        // editable and must NOT drive the lookup.
                        const cb = cbStates[provider.id.toLowerCase()];
                        if (!cb) return null;
                        const colorClass = cb.state === "closed"
                          ? "bg-emerald-500"
                          : cb.state === "open"
                            ? "bg-red-500"
                            : "bg-yellow-500";
                        const label = cb.state === "closed"
                          ? t("settings.modelProviders.circuitClosed")
                          : cb.state === "open"
                            ? t("settings.modelProviders.circuitOpen")
                            : t("settings.modelProviders.circuitHalfOpen");
                        return (
                          <span
                            className={cn("h-2 w-2 rounded-full", colorClass)}
                            title={label}
                          />
                        );
                      })()}
                    </div>
                    <p className="text-[10px] text-muted-foreground">
                      {provider.models.length}{t("settings.modelProviders.models")} · {provider.apiFormat}
                      {!provider.enabled && t("settings.modelProviders.disabled")}
                    </p>
                  </div>
                  <Switch
                    checked={provider.enabled}
                    onCheckedChange={(v) => toggleProvider(provider.id, v)}
                  />
                  <button
                    onClick={() => requestDelete({ kind: "provider", id: provider.id })}
                    className={cn(
                      "flex h-6 items-center gap-1 rounded px-1.5 text-[10px] transition-colors",
                      isArmed({ kind: "provider", id: provider.id })
                        ? "bg-destructive/10 text-destructive"
                        : "text-muted-foreground hover:text-destructive",
                    )}
                    title={isArmed({ kind: "provider", id: provider.id })
                      ? t("settings.modelProviders.confirmDelete")
                      : t("settings.modelProviders.deleteProvider")}
                    aria-label={t("settings.modelProviders.deleteProvider")}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    {isArmed({ kind: "provider", id: provider.id }) && t("settings.modelProviders.confirmDelete")}
                  </button>
                </div>

                {isExpanded && (
                  <div className="space-y-3 border-t border-border p-3">
                    {(() => {
                      const cb = cbStates[provider.id.toLowerCase()];
                      if (cb && cb.state !== "closed") {
                        return (
                          <div className="flex items-center gap-2 rounded-md border border-yellow-500/30 bg-yellow-500/5 px-2 py-1.5">
                            <AlertCircle className="h-3 w-3 shrink-0 text-yellow-500" />
                            <span className="text-[11px] text-yellow-600 dark:text-yellow-400">
                              {t("settings.modelProviders.circuitTripped")}: {cb.state}
                            </span>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="ml-auto h-6 gap-1 px-2 text-[10px]"
                              onClick={() => circuitBreakerApi.reset(provider.id.toLowerCase())}
                            >
                              <RefreshCw className="h-2.5 w-2.5" />
                              {t("settings.modelProviders.resetCircuit")}
                            </Button>
                          </div>
                        );
                      }
                      return null;
                    })()}
                    <div>
                      <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.name")}</label>
                      <Input
                        value={provider.name}
                        onChange={(e) => updateProvider(provider.id, { name: e.target.value })}
                        className="h-8 text-xs"
                      />
                    </div>

                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.baseUrl")}</label>
                        <Input
                          value={provider.baseUrl}
                          onChange={(e) => updateProvider(provider.id, { baseUrl: e.target.value })}
                          placeholder="https://api.example.com/v1"
                          className="h-8 text-xs"
                        />
                      </div>
                      <div>
                        <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.apiKey")}</label>
                        <SecretInput
                          value={provider.apiKey}
                          onChange={(v) => updateProvider(provider.id, { apiKey: v })}
                          placeholder={t("settings.modelProviders.apiKeyPlaceholder")}
                          className="h-8 text-xs"
                        />
                      </div>
                    </div>

                    <div>
                      <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.apiFormat")}</label>
                      <SettingSelect
                        value={provider.apiFormat}
                        onChange={(v) => updateProvider(provider.id, { apiFormat: v as ApiFormat })}
                        options={formatOptionsFor(provider.id)}
                        className="w-full"
                      />
                    </div>

                    <div className="flex items-center gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-8 gap-1.5 text-xs"
                        onClick={() => handleFetch(provider.id)}
                        disabled={fs.loading || !provider.baseUrl}
                      >
                        {fs.loading ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <Download className="h-3.5 w-3.5" />
                        )}
                        {fs.loading ? t("settings.modelProviders.fetching") : t("settings.modelProviders.fetchModels")}
                      </Button>
 {renderFetchStatus(fs)}
                    </div>

                    <div>
                      <label className="mb-1 block text-[10px] text-muted-foreground">
                        {t("settings.modelProviders.modelList", { count: provider.models.length })}
                      </label>
                      {provider.models.length > 0 && (
                        <div className="space-y-1">
                          {provider.models.map((model) => (
                            <div
                              key={model.id}
                              className="flex items-center gap-2 rounded-md border border-border bg-background px-2 py-1.5"
                            >
                              <span className="flex-1 text-xs font-medium">{model.name}</span>
                              <SettingSelect
                                value={String(model.contextWindow)}
                                onChange={(v) =>
                                  updateModel(provider.id, model.id, {
                                    contextWindow: Number(v),
                                  })
                                }
                                options={contextWindowOptions(model.contextWindow)}
                                className="h-6 w-24 text-xs"
                              />
                              <span className="text-[10px] text-muted-foreground">
                                {t("settings.modelProviders.contextWindow")}
                              </span>
                              <button
                                onClick={() => requestDelete({ kind: "model", providerId: provider.id, modelId: model.id })}
                                className={cn(
                                  "flex h-5 items-center gap-1 rounded px-1 text-[9px] transition-colors",
                                  isArmed({ kind: "model", providerId: provider.id, modelId: model.id })
                                    ? "bg-destructive/10 text-destructive"
                                    : "text-muted-foreground hover:text-destructive",
                                )}
                                title={isArmed({ kind: "model", providerId: provider.id, modelId: model.id })
                                  ? t("settings.modelProviders.confirmDelete")
                                  : t("settings.modelProviders.deleteModel")}
                                aria-label={t("settings.modelProviders.deleteModel")}
                              >
                                <Trash2 className="h-3 w-3" />
                                {isArmed({ kind: "model", providerId: provider.id, modelId: model.id }) && t("settings.modelProviders.confirmDelete")}
                              </button>
                            </div>
                          ))}
                        </div>
                      )}
                      {manualAddFor === provider.id ? (
                        <div className="mt-2 flex items-center gap-2">
                          <Input
                            autoFocus
                            value={manualAddName}
                            onChange={(e) => setManualAddName(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") commitManualAdd(provider.id);
                              if (e.key === "Escape") {
                                setManualAddFor(null);
                                setManualAddName("");
                                setManualAddCtx("");
                              }
                            }}
                            placeholder={t("settings.modelProviders.manualAddPrompt")}
                            className="h-7 flex-1 text-xs"
                          />
                          <SettingSelect
                            value={manualAddCtx || "1000000"}
                            onChange={setManualAddCtx}
                            options={contextWindowOptions(Number(manualAddCtx) || 1_000_000)}
                            className="h-7 w-24 text-xs"
                          />
                          <Button
                            variant="outline"
                            size="sm"
                            className="h-7 shrink-0 px-2 text-[10px]"
                            disabled={!manualAddName.trim()}
                            onClick={() => commitManualAdd(provider.id)}
                          >
                            <Plus className="h-3 w-3" />
                            {t("common.confirm")}
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-7 shrink-0 px-2 text-[10px]"
                            onClick={() => {
                              setManualAddFor(null);
                              setManualAddName("");
                              setManualAddCtx("");
                            }}
                          >
                            {t("common.cancel")}
                          </Button>
                        </div>
                      ) : (
                        <Button
                          variant="outline"
                          size="sm"
                          className="mt-2 w-full"
                          onClick={() => {
                            setManualAddFor(provider.id);
                            setManualAddName("");
                          }}
                        >
                          <Plus className="h-3 w-3" />
                          {t("settings.modelProviders.manualAddModel")}
                        </Button>
                      )}
                    </div>
                  </div>
                )}
              </div>
              );
            })}
        </div>
      )}

      <ModelProviderAddForm
        show={showAddForm}
        onShowChange={setShowAddForm}
        newName={newName}
        setNewName={setNewName}
        newUrl={newUrl}
        setNewUrl={setNewUrl}
        newKey={newKey}
        setNewKey={setNewKey}
        newFormat={newFormat}
        setNewFormat={setNewFormat}
        newModels={newModels}
        setNewModels={setNewModels}
        newFetchState={newFetchState}
        handleNewFetch={handleNewFetch}
        handleAddProvider={handleAddProvider}
        onCancel={() => {
          setShowAddForm(false);
          setNewFetchState({ loading: false, error: null, success: null });
        }}
        formatOptions={allFormatOptions}
      />

      </div>
  );
}
