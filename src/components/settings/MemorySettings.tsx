/**
 * MemorySettings — memory store management.
 *
 * Lists stored memories (from the persistent FTS5 store) with a delete
 * action, shows the total count, and exposes the dream synthesis trigger
 * ("整理记忆" compresses raw memories into structured knowledge via the
 * configured model).
 */

import { useState, useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Trash2, Sparkles, RefreshCw, Plus, Search, FileText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SettingRow } from "@/components/settings/SettingRow";
import { NumberField } from "@/components/settings/NumberField";
import { useConfigSection } from "@/hooks/useConfigSection";
import { memoryApi, type MemoryFileInfo, type ProcedureFilesView } from "@/lib/tauri";
import type { Memory, MemorySearchResult } from "@/types";
import { cn } from "@/lib/utils";

/** One standing-memory/procedure file card (path, entries, chars, mtime). */
function FileStatusCard({
  label,
  info,
}: {
  label: string;
  info: MemoryFileInfo | null;
}) {
  const { t } = useTranslation();
  if (!info) return null;
  return (
    <div className="rounded-md border border-border bg-background px-3 py-2">
      <p className="text-xs font-medium">{label}</p>
      {info.exists ? (
        <>
          <p
            className="mt-0.5 truncate font-mono text-[10px] text-muted-foreground"
            title={info.path}
          >
            {info.path}
          </p>
          <p className="mt-1 text-[10px] text-muted-foreground">
            {t("settings.memory.memoryFileEntries", { count: info.entries })} ·{" "}
            {t("settings.memory.memoryFileChars", { count: info.chars })}
            {info.modified_at_ms
              ? ` · ${t("settings.memory.memoryFileModified", {
                  time: new Date(info.modified_at_ms).toLocaleString(),
                })}`
              : ""}
          </p>
        </>
      ) : (
        <p className="mt-0.5 text-[10px] text-muted-foreground">
          {t("settings.memory.memoryFileMissing")}
        </p>
      )}
    </div>
  );
}

/** ── Standing memory (MEMORY.md) status ───────────────────── */
function MemoryFilesSection() {
  const { t } = useTranslation();
  const [files, setFiles] = useState<{
    user: MemoryFileInfo;
    project: MemoryFileInfo | null;
  } | null>(null);

  useEffect(() => {
    void memoryApi
      .getMemoryFiles()
      .then(setFiles)
      .catch(() => {
        /* best-effort — status card stays hidden on failure */
      });
  }, []);

  return (
    <section className="space-y-2">
      <div className="flex items-center gap-1.5">
        <FileText className="h-4 w-4 text-primary/70" />
        <h3 className="text-sm font-semibold">{t("settings.memory.memoryFilesTitle")}</h3>
      </div>
      <p className="text-xs text-muted-foreground">{t("settings.memory.memoryFilesDesc")}</p>
      <div className="grid gap-2 sm:grid-cols-2">
        {files && (
          <FileStatusCard label={t("settings.memory.memoryFileUser")} info={files.user} />
        )}
        {files && (
          <FileStatusCard label={t("settings.memory.memoryFileProject")} info={files.project} />
        )}
      </div>
    </section>
  );
}

/** ── Procedural memory (procedures.md) status ─────────────── */
function ProcedureFilesSection() {
  const { t } = useTranslation();
  const [files, setFiles] = useState<ProcedureFilesView | null>(null);

  useEffect(() => {
    void memoryApi
      .getProcedureFiles()
      .then(setFiles)
      .catch(() => {
        /* best-effort — status card stays hidden on failure */
      });
  }, []);

  return (
    <section className="space-y-2">
      <div className="flex items-center gap-1.5">
        <FileText className="h-4 w-4 text-primary/70" />
        <h3 className="text-sm font-semibold">{t("settings.memory.procedureFilesTitle")}</h3>
      </div>
      <p className="text-xs text-muted-foreground">{t("settings.memory.procedureFilesDesc")}</p>
      <div className="grid gap-2 sm:grid-cols-2">
        {files && (
          <FileStatusCard label={t("settings.memory.memoryFileUser")} info={files.user} />
        )}
        {files && (
          <FileStatusCard label={t("settings.memory.memoryFileProject")} info={files.project} />
        )}
      </div>
    </section>
  );
}

/** 记忆检索权重 — 直接读写后端 memory.search_weight_* 配置。 */
interface MemoryWeights {
  search_weight_bm25: number;
  search_weight_cosine: number;
  search_weight_recency: number;
  search_recency_half_life_hours: number;
}

function WeightSlider({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
}) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-24 shrink-0 text-[10px] text-muted-foreground">{label}</span>
      <input
        type="range"
        min={0}
        max={1}
        step={0.05}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        aria-label={label}
        className="h-1 flex-1 cursor-pointer accent-primary"
      />
      <span className="w-10 shrink-0 text-right font-mono text-[10px] text-muted-foreground">
        {value.toFixed(2)}
      </span>
    </div>
  );
}

/** 记忆检索权重编辑器 — 滑块直接写后端 memory 配置。
 *  滑块拖动频率很高:本地状态即时更新,后端 patch 防抖合并为一次写。 */
function MemoryWeightsEditor() {
  const { t } = useTranslation();
  const { load, patch } = useConfigSection();
  const [weights, setWeights] = useState<MemoryWeights | null>(null);
  const patchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    void (async () => {
      const memory = await load("memory");
      if (!memory) return;
      setWeights({
        search_weight_bm25: Number(memory.search_weight_bm25 ?? 0.4),
        search_weight_cosine: Number(memory.search_weight_cosine ?? 0.4),
        search_weight_recency: Number(memory.search_weight_recency ?? 0.2),
        search_recency_half_life_hours: Number(memory.search_recency_half_life_hours ?? 168),
      });
    })();
    return () => {
      if (patchTimerRef.current) clearTimeout(patchTimerRef.current);
    };
  }, [load]);

  const persist = (data: Partial<MemoryWeights>) => {
    setWeights((prev) => {
      const next = { ...prev!, ...data };
      // Debounced backend write — dragging a slider fires 20+ patches/s.
      if (patchTimerRef.current) clearTimeout(patchTimerRef.current);
      patchTimerRef.current = setTimeout(() => {
        patchTimerRef.current = null;
        void patch("memory", next);
      }, 300);
      return next;
    });
  };

  if (!weights) return null;

  return (
    <div className="space-y-2">
      <WeightSlider
        label={t("settings.general.weightBm25")}
        value={weights.search_weight_bm25}
        onChange={(v) => void persist({ search_weight_bm25: v })}
      />
      <WeightSlider
        label={t("settings.general.weightCosine")}
        value={weights.search_weight_cosine}
        onChange={(v) => void persist({ search_weight_cosine: v })}
      />
      <WeightSlider
        label={t("settings.general.weightRecency")}
        value={weights.search_weight_recency}
        onChange={(v) => void persist({ search_weight_recency: v })}
      />
      <SettingRow
        searchKey="settings.general.recencyHalfLife"
        label={t("settings.general.recencyHalfLife")}
        description={t("settings.general.recencyHalfLifeDesc")}
      >
        <NumberField
          min={1}
          value={weights.search_recency_half_life_hours}
          onCommit={(v) => persist({ search_recency_half_life_hours: v })}
        />
      </SettingRow>
      <p className="text-[10px] text-muted-foreground/60">
        {t("settings.general.memoryWeightsDesc")}
      </p>
    </div>
  );
}

/** 记忆检索权重 — 渲染在「记忆」分类，检索参数本就属于记忆范畴。 */
function MemoryWeightsSection() {
  const { t } = useTranslation();
  return (
    <section className="space-y-2">
      <div>
        <h3 className="text-sm font-semibold">{t("settings.general.memoryWeights")}</h3>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {t("settings.general.memoryWeightsDesc")}
        </p>
      </div>
      <MemoryWeightsEditor />
    </section>
  );
}

export function MemorySettings() {
  const { t } = useTranslation();
  const [memories, setMemories] = useState<Memory[]>([]);
  const [count, setCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [dreaming, setDreaming] = useState(false);
  const [dreamResult, setDreamResult] = useState<string | null>(null);
  const [dreamError, setDreamError] = useState(false);

  // ── Search ─────────────────────────────────────────────
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<MemorySearchResult[] | null>(null);
  const [searching, setSearching] = useState(false);

  // ── Manual store ───────────────────────────────────────
  const [draftContent, setDraftContent] = useState("");
  const [draftCategory, setDraftCategory] = useState("fact");
  const [saving, setSaving] = useState(false);
  const [storeError, setStoreError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [list, total] = await Promise.all([
        memoryApi.listMemories(100),
        memoryApi.getMemoryCount(),
      ]);
      setMemories(list);
      setCount(total);
    } catch {
      setMemories([]);
      setCount(0);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleSearch = useCallback(async () => {
    if (!query.trim()) {
      setSearchResults(null);
      return;
    }
    setSearching(true);
    try {
      setSearchResults(await memoryApi.searchMemories(query.trim(), 10));
    } catch {
      setSearchResults([]);
    } finally {
      setSearching(false);
    }
  }, [query]);

  const handleStore = useCallback(async () => {
    if (!draftContent.trim()) return;
    setSaving(true);
    setStoreError(null);
    try {
      await memoryApi.storeMemory(draftContent.trim(), draftCategory);
      setDraftContent("");
      await load();
    } catch (e) {
      // Keep the draft so the user does not lose the text — but SAY so,
      // otherwise the user believes the memory was saved.
      setStoreError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }, [draftContent, draftCategory, load]);

  const handleDelete = useCallback(
    async (id: number) => {
      try {
        await memoryApi.deleteMemory(id);
        setMemories((prev) => prev.filter((m) => m.id !== id));
        setCount((c) => Math.max(0, c - 1));
        setSearchResults((prev) =>
          prev ? prev.filter((r) => r.memory.id !== id) : prev,
        );
      } catch {
        // Keep the entry — the backend rejected the delete.
      }
    },
    [],
  );

  const handleDream = useCallback(async () => {
    setDreaming(true);
    setDreamResult(null);
    setDreamError(false);
    try {
      const result = await memoryApi.triggerDream();
      setDreamResult(
        t("settings.memory.dreamDone", {
          source: result.source_count,
          synthesized: result.synthesized_count,
        }),
      );
      await load();
    } catch (e) {
      setDreamResult(e instanceof Error ? e.message : String(e));
      setDreamError(true);
    } finally {
      setDreaming(false);
    }
  }, [t, load]);

  return (
    <div className="space-y-6">
      <MemoryFilesSection />
      <ProcedureFilesSection />
      <MemoryWeightsSection />
      <section>
        <h3 className="mb-3 text-sm font-semibold">{t("settings.memory.title")}</h3>

        {/* ── Search ─────────────────────────────────────────── */}
        <div className="mb-3 flex gap-2">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void handleSearch();
            }}
            placeholder={t("settings.memory.searchPlaceholder")}
            className="h-8 text-xs"
          />
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            onClick={() => void handleSearch()}
            disabled={searching || !query.trim()}
          >
            {searching ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Search className="h-3.5 w-3.5" />
            )}
            {t("settings.memory.search")}
          </Button>
        </div>

        {searchResults !== null && (
          <div className="mb-3 space-y-1.5">
            <p className="text-[10px] text-muted-foreground">
              {t("settings.memory.searchResults", { count: searchResults.length })}
            </p>
            {searchResults.map((r) => (
              <SearchResultRow key={r.memory.id} result={r} onDelete={handleDelete} />
            ))}
          </div>
        )}

        {/* ── Manual store ───────────────────────────────────── */}
        <div className="mb-3 rounded-md border border-border bg-background p-2.5">
          <div className="flex gap-2">
            <Input
              value={draftContent}
              onChange={(e) => setDraftContent(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void handleStore();
              }}
              placeholder={t("settings.memory.storePlaceholder")}
              className="h-8 flex-1 text-xs"
            />
            <select
              value={draftCategory}
              onChange={(e) => setDraftCategory(e.target.value)}
              aria-label={t("settings.memory.category", { defaultValue: "分类" })}
              className="h-8 rounded-md border border-border bg-background px-2 text-xs text-foreground"
            >
              <option value="fact">{t("settings.memory.categoryFact", { defaultValue: "事实" })}</option>
              <option value="preference">{t("settings.memory.categoryPreference", { defaultValue: "偏好" })}</option>
              <option value="project">{t("settings.memory.categoryProject", { defaultValue: "项目" })}</option>
            </select>
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5 text-xs"
              onClick={() => void handleStore()}
              disabled={saving || !draftContent.trim()}
            >
              {saving ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Plus className="h-3.5 w-3.5" />
              )}
              {t("settings.memory.store")}
            </Button>
          </div>
          {storeError && (
            <p className="mt-1.5 text-[10px] text-destructive">
              {t("settings.memory.storeFailed", { defaultValue: "保存失败：{{error}}" }).replace("{{error}}", storeError)}
            </p>
          )}
        </div>

        <div className="mb-3 flex items-center justify-between">
          <p className="text-xs text-muted-foreground">
            {t("settings.memory.total", { count })}
          </p>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1.5 text-xs"
              onClick={() => void load()}
              disabled={loading}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              {t("settings.memory.refresh")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1.5 text-xs"
              onClick={() => void handleDream()}
              disabled={dreaming || memories.length === 0}
            >
              {dreaming ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Sparkles className="h-3.5 w-3.5" />
              )}
              {t("settings.memory.dream")}
            </Button>
          </div>
        </div>

        {dreamResult && (
          <p
            className={cn(
              "mb-3 rounded-md border p-2 text-[11px]",
              dreamError
                ? "border-destructive/30 bg-destructive/5 text-destructive"
                : "border-primary/30 bg-primary/5 text-primary",
            )}
          >
            {dreamResult}
          </p>
        )}

        {loading ? (
          <div className="flex items-center justify-center py-10">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : memories.length === 0 ? (
          <p className="rounded-md bg-muted/40 px-3 py-4 text-center text-xs text-muted-foreground">
            {t("settings.memory.empty")}
          </p>
        ) : (
          <div className="space-y-1.5">
            {memories.map((m) => (
              <div
                key={m.id}
                className="flex items-start gap-2 rounded-md border border-border bg-background px-2.5 py-2"
              >
                <div className="min-w-0 flex-1">
                  <div className="mb-0.5 flex items-center gap-2">
                    <span className="rounded bg-secondary px-1.5 py-0.5 text-[9px] font-medium text-secondary-foreground">
                      {m.category}
                    </span>
                    <span className="text-[10px] text-muted-foreground">
                      {new Date(m.created_at).toLocaleString()}
                    </span>
                  </div>
                  <p className="line-clamp-2 text-xs text-foreground/90">{m.content}</p>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-6 w-6 shrink-0 px-0 text-muted-foreground hover:text-destructive"
                  onClick={() => void handleDelete(m.id)}
                  aria-label={t("settings.memory.delete")}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

/** Single search result row with relevance score + delete. */
function SearchResultRow({
  result,
  onDelete,
}: {
  result: MemorySearchResult;
  onDelete: (id: number) => Promise<void>;
}) {
  const { t } = useTranslation();
  const m = result.memory;
  return (
    <div className="flex items-start gap-2 rounded-md border border-primary/25 bg-primary/5 px-2.5 py-2">
      <div className="min-w-0 flex-1">
        <div className="mb-0.5 flex items-center gap-2">
          <span className="rounded bg-secondary px-1.5 py-0.5 text-[9px] font-medium text-secondary-foreground">
            {m.category}
          </span>
          <span className="text-[10px] text-muted-foreground">
            {t("settings.memory.score", { score: result.score.toFixed(2) })}
          </span>
        </div>
        <p className="line-clamp-2 text-xs text-foreground/90">{m.content}</p>
      </div>
      <Button
        variant="ghost"
        size="sm"
        className="h-6 w-6 shrink-0 px-0 text-muted-foreground hover:text-destructive"
        onClick={() => void onDelete(m.id)}
        aria-label={t("settings.memory.delete")}
      >
        <Trash2 className="h-3.5 w-3.5" />
      </Button>
    </div>
  );
}
