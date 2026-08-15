/**
 * ScheduledView — 定时任务 page.
 *
 * Create/pause/delete scheduled agent tasks, trigger runs, and review the
 * run inbox (status, summary, session, worktree cleanup). Runs are
 * unattended by design: approvals become denials and ask_user is refused.
 */

import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Clock, Loader2, Play, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useAppStore } from "@/stores/appStore";
import { describeSchedule, useScheduledStore } from "@/stores/scheduledStore";
import { useScheduledEvents } from "@/hooks/useScheduledEvents";
import { cn } from "@/lib/utils";
import type { ScheduledRunStatus, ScheduleSpec } from "@/types/scheduled";

function formatTime(ms: number | null | undefined): string {
  if (!ms && ms !== 0) return "—";
  return new Date(ms).toLocaleString();
}

function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

const STATUS_COLORS: Record<ScheduledRunStatus, string> = {
  pending: "bg-muted text-muted-foreground",
  running: "bg-primary text-primary-foreground animate-pulse",
  completed: "bg-emerald-500/15 text-emerald-600",
  failed: "bg-destructive/15 text-destructive",
  skipped: "bg-muted text-muted-foreground",
  cancelled: "bg-muted text-muted-foreground",
};

export function ScheduledView() {
  const { t } = useTranslation();
  const setScheduledOpen = useAppStore((s) => s.setScheduledOpen);
  const store = useScheduledStore();
  useScheduledEvents();

  // ── Create form ─────────────────────────────────────────────
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [kind, setKind] = useState<"interval" | "daily">("interval");
  const [everyMinutes, setEveryMinutes] = useState(60);
  const [dailyTime, setDailyTime] = useState("09:00");
  const [projectPath, setProjectPath] = useState("");
  const [useWorktree, setUseWorktree] = useState(false);
  const [persistent, setPersistent] = useState(false);
  const [workMode, setWorkMode] = useState<"code" | "depwork">("code");
  const [model, setModel] = useState("");
  const [creating, setCreating] = useState(false);
  const [notice, setNotice] = useState<{ text: string; kind: "success" | "error" } | null>(null);

  const notify = (text: string, kind: "success" | "error" = "success") =>
    setNotice({ text, kind });
  const notifyError = (e: unknown) =>
    notify(e instanceof Error ? e.message : String(e), "error");

  const schedule: ScheduleSpec = useMemo(
    () =>
      kind === "interval"
        ? { kind: "interval", every_secs: Math.max(60, Math.round(everyMinutes) * 60) }
        : { kind: "daily", time: dailyTime },
    [kind, everyMinutes, dailyTime],
  );

  const create = async () => {
    if (!name.trim() || !prompt.trim()) return;
    setCreating(true);
    try {
      await store.create({
        name: name.trim(),
        prompt: prompt.trim(),
        schedule,
        projectPath: projectPath.trim() || undefined,
        useWorktree,
        workMode,
        model: model.trim() || undefined,
        persistent,
      });
      setName("");
      setPrompt("");
      notify(t("scheduled.created"));
    } catch (e) {
      notifyError(e);
    } finally {
      setCreating(false);
    }
  };

  const runNow = async (id: string) => {
    try {
      await store.runNow(id);
      notify(t("scheduled.running"));
    } catch (e) {
      notifyError(e);
    }
  };

  const removeTask = async (id: string) => {
    if (!window.confirm(t("scheduled.confirmDeleteTask"))) return;
    try {
      await store.remove(id);
      notify(t("scheduled.deleted"));
    } catch (e) {
      notifyError(e);
    }
  };

  const cancelRun = async (runId: string) => {
    try {
      await store.cancelRun(runId);
      notify(t("scheduled.cancelled"));
    } catch (e) {
      notifyError(e);
    }
  };

  const deleteRun = async (runId: string) => {
    if (!window.confirm(t("scheduled.confirmDeleteRun"))) return;
    try {
      await store.deleteRun(runId);
    } catch (e) {
      notifyError(e);
    }
  };

  const cleanupWorktree = async (runId: string) => {
    try {
      const message = await store.cleanupWorktree(runId);
      notify(t("scheduled.cleaned", { message }));
    } catch (e) {
      notifyError(e);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-2 border-b px-4 py-2.5">
        <Button
          size="icon"
          variant="ghost"
          className="h-7 w-7"
          onClick={() => setScheduledOpen(false)}
          aria-label={t("scheduled.back")}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <Clock className="h-4 w-4 text-muted-foreground" />
        <h1 className="text-sm font-semibold">{t("scheduled.title")}</h1>
        {notice && (
          <span
            className={cn(
              "ml-auto flex items-center gap-1 text-xs",
              notice.kind === "error"
                ? "text-destructive"
                : "text-muted-foreground",
            )}
          >
            {notice.text}
            <button
              className="text-muted-foreground/50 hover:text-foreground"
              onClick={() => setNotice(null)}
              aria-label="close"
            >
              <X className="h-3 w-3" />
            </button>
          </span>
        )}
      </header>

      <ScrollArea className="flex-1">
        <div className="space-y-4 p-4">
          <p className="rounded-md bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
            {t("scheduled.unattendedHint")}
          </p>

          {/* ── Create form ─────────────────────────────────────── */}
          <section className="rounded-lg border p-3">
            <h2 className="mb-2 text-xs font-semibold text-muted-foreground">
              {t("scheduled.createTitle")}
            </h2>
            <div className="grid gap-2">
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t("scheduled.namePlaceholder")}
                className="h-8 text-xs"
              />
              <Textarea
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                placeholder={t("scheduled.promptPlaceholder")}
                className="min-h-[64px] text-xs"
              />
              <div className="flex flex-wrap items-center gap-2">
                <label className="text-xs text-muted-foreground">{t("scheduled.scheduleLabel")}</label>
                <select
                  value={kind}
                  onChange={(e) => setKind(e.target.value as "interval" | "daily")}
                  className="h-8 rounded-md border bg-background px-2 text-xs"
                >
                  <option value="interval">{t("scheduled.intervalOption")}</option>
                  <option value="daily">{t("scheduled.dailyOption")}</option>
                </select>
                {kind === "interval" ? (
                  <Input
                    type="number"
                    min={1}
                    value={everyMinutes}
                    onChange={(e) => setEveryMinutes(Number(e.target.value) || 60)}
                    className="h-8 w-24 text-xs"
                    aria-label={t("scheduled.everyMinutesLabel")}
                  />
                ) : (
                  <Input
                    value={dailyTime}
                    onChange={(e) => setDailyTime(e.target.value)}
                    className="h-8 w-24 text-xs"
                    aria-label={t("scheduled.dailyTimeLabel")}
                  />
                )}
                <select
                  value={workMode}
                  onChange={(e) => setWorkMode(e.target.value as "code" | "depwork")}
                  className="h-8 rounded-md border bg-background px-2 text-xs"
                  aria-label={t("scheduled.workModeLabel")}
                >
                  <option value="code">{t("scheduled.codeOption")}</option>
                  <option value="depwork">{t("scheduled.depworkOption")}</option>
                </select>
              </div>
              <Input
                value={projectPath}
                onChange={(e) => setProjectPath(e.target.value)}
                placeholder={t("scheduled.projectLabel")}
                className="h-8 text-xs"
              />
              <Input
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder={t("scheduled.modelLabel")}
                className="h-8 text-xs"
              />
              <label className="flex items-center gap-2 text-xs text-muted-foreground">
                <Switch checked={useWorktree} onCheckedChange={setUseWorktree} disabled={persistent} />
                {t("scheduled.worktreeLabel")}
              </label>
              <label className="flex items-center gap-2 text-xs text-muted-foreground">
                <Switch
                  checked={persistent}
                  onCheckedChange={(v) => {
                    setPersistent(v);
                    if (v) setUseWorktree(false);
                  }}
                />
                {t("scheduled.persistentLabel", { defaultValue: "常驻 agent（跨次运行累积上下文）" })}
              </label>
              <div>
                <Button size="sm" className="h-8 text-xs" onClick={create} disabled={creating || !name.trim() || !prompt.trim()}>
                  {creating && <Loader2 className="mr-1 h-3 w-3 animate-spin" />}
                  {t("scheduled.createButton")}
                </Button>
              </div>
            </div>
          </section>

          {/* ── Task list ───────────────────────────────────────── */}
          <section className="space-y-2">
            <h2 className="text-xs font-semibold text-muted-foreground">{t("scheduled.taskTitle")}</h2>
            {store.loading && (
              <p className="flex items-center gap-1 text-xs text-muted-foreground">
                <Loader2 className="h-3 w-3 animate-spin" /> {t("scheduled.loading")}
              </p>
            )}
            {store.error && <p className="text-xs text-destructive">{t("scheduled.loadFailed", { error: store.error })}</p>}
            {!store.loading && store.tasks.length === 0 && (
              <p className="text-xs text-muted-foreground">{t("scheduled.emptyTasks")}</p>
            )}
            {store.tasks.map((task) => (
              <div key={task.id} className="rounded-lg border p-3">
                <div className="flex items-center gap-2">
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">{task.name}</span>
                  <Badge variant="outline" className="shrink-0 text-[10px]">
                    {describeSchedule(task.schedule, t)}
                  </Badge>
                  <Badge variant="secondary" className="shrink-0 text-[10px]">
                    {task.work_mode === "depwork" ? t("scheduled.depworkOption") : t("scheduled.codeOption")}
                  </Badge>
                  {task.persistent && (
                    <Badge className="shrink-0 bg-primary/15 text-[10px] text-primary">
                      {t("scheduled.persistentBadge")}
                    </Badge>
                  )}
                  <Switch
                    checked={task.active}
                    onCheckedChange={(active) =>
                      store.updateTask(task.id, { active }).catch(notifyError)
                    }
                  />
                  <Button size="sm" variant="outline" className="h-7 px-2 text-[11px]" onClick={() => runNow(task.id)}>
                    <Play className="mr-1 h-3 w-3" />
                    {t("scheduled.runNow")}
                  </Button>
                  <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" onClick={() => removeTask(task.id)} aria-label={t("scheduled.delete")}>
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
                <p className="mt-1 line-clamp-2 whitespace-pre-wrap text-xs text-muted-foreground">{task.prompt}</p>
                <p className="mt-1 text-[11px] text-muted-foreground/70">
                  {t("scheduled.runCount", { count: task.run_count })} · {t("scheduled.lastRun")}: {formatTime(task.last_run_at_ms)}
                  {task.use_worktree && ` · ${t("scheduled.worktree")}`}
                  {task.project_path ? ` · ${task.project_path}` : ""}
                </p>
                <button
                  className="mt-1 text-[11px] text-primary/80 hover:text-primary"
                  onClick={() => store.loadRuns(store.runsTaskId === task.id ? null : task.id)}
                >
                  {store.runsTaskId === task.id ? t("scheduled.allRuns") : t("scheduled.runsTitle")}
                </button>
              </div>
            ))}
          </section>

          {/* ── Run inbox ──────────────────────────────────────── */}
          <section className="space-y-2">
            <h2 className="text-xs font-semibold text-muted-foreground">{t("scheduled.runsTitle")}</h2>
            {store.runs.length === 0 && (
              <p className="text-xs text-muted-foreground">{t("scheduled.emptyRuns")}</p>
            )}
            {store.runs.map((run) => (
              <div key={run.id} className="rounded-lg border p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge className={`text-[10px] ${STATUS_COLORS[run.status]}`}>{t(`scheduled.status${cap(run.status)}`)}</Badge>
                  <span className="text-[11px] text-muted-foreground">
                    {formatDateTime(run.started_at)}
                    {run.finished_at ? ` → ${formatDateTime(run.finished_at)}` : ""}
                  </span>
                  {run.session_id && (
                    <span className="ml-auto max-w-[180px] truncate text-[11px] text-muted-foreground/70" title={run.session_id}>
                      {t("scheduled.openSession")}: {run.session_id}
                    </span>
                  )}
                  <div className="ml-auto flex items-center gap-1">
                    {run.status === "running" && (
                      <Button size="sm" variant="outline" className="h-6 px-2 text-[11px]" onClick={() => cancelRun(run.id)}>
                        {t("scheduled.cancelRun")}
                      </Button>
                    )}
                    {run.worktree_path && (
                      <Button size="sm" variant="outline" className="h-6 px-2 text-[11px]" onClick={() => cleanupWorktree(run.id)}>
                        {t("scheduled.cleanupWorktree")}
                      </Button>
                    )}
                    <Button size="icon" variant="ghost" className="h-6 w-6 text-muted-foreground hover:text-destructive" onClick={() => deleteRun(run.id)} aria-label={t("scheduled.deleteRun")}>
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </div>
                </div>
                {run.summary && <p className="mt-1 line-clamp-3 whitespace-pre-wrap text-xs">{run.summary}</p>}
                {run.error && <p className="mt-1 line-clamp-3 whitespace-pre-wrap text-xs text-destructive">{t("scheduled.error")}: {run.error}</p>}
                {run.worktree_path && (
                  <p className="mt-1 truncate text-[11px] text-muted-foreground/70" title={run.worktree_path}>
                    {t("scheduled.worktree")}: {run.worktree_path}
                  </p>
                )}
              </div>
            ))}
          </section>
        </div>
      </ScrollArea>
    </div>
  );
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}
