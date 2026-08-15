/**
 * SkillsSettings — skill management settings page.
 *
 * Lists bundled and user-level skills. Allows adding, editing, and
 * deleting user skills. Skills can be invoked via $skill-name in chat.
 * Also exposes the ecosystem-compat gates (Claude / Cursor skills).
 */

import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Zap, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { skillsApi } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settingsStore";
import type { Skill } from "@/types";
import { cn } from "@/lib/utils";

export interface SkillsSettingsProps {
  className?: string;
}

export function SkillsSettings({ className }: SkillsSettingsProps) {
  const { t } = useTranslation();
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [showAdd, setShowAdd] = useState(false);
  const [newName, setNewName] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [armedDelete, setArmedDelete] = useState<string | null>(null);
  const skillsCompat = useSettingsStore((s) => s.skillsCompat);
  const setSkillsCompat = useSettingsStore((s) => s.setSkillsCompat);

  const loadSkills = useCallback(async () => {
    setLoading(true);
    try {
      const list = await skillsApi.list();
      setSkills(list);
    } catch {
      setSkills([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void loadSkills(); }, [loadSkills]);

  const handleAdd = useCallback(() => {
    if (!newName.trim()) return;
    // Backend `Skill` requires id/content and `source` is the bundled|file
    // enum — send the full payload or deserialization fails silently.
    const name = newName.trim();
    // Sanitize the derived id: spaces/special characters produce invalid ids
    // that the backend cannot route back to this skill.
    const safeId = `user-${name.toLowerCase().replace(/[^a-z0-9._-]+/g, "-")}`;
    const payload = {
      id: safeId,
      name,
      description: newDesc.trim(),
      content: newDesc.trim(),
      source: "file",
      enabled: true,
    };
    void skillsApi.save(payload)
      .then(() => {
        setNewName("");
        setNewDesc("");
        setSaveError(null);
        setShowAdd(false);
        void loadSkills();
      })
      .catch((e: unknown) => {
        setSaveError(
          e instanceof Error ? e.message : String(e),
        );
      });
  }, [newName, newDesc, loadSkills]);

  const handleDelete = useCallback((id: string) => {
    // Two-step confirm: first click arms, second click (within 3s) deletes.
    if (armedDelete !== id) {
      setArmedDelete(id);
      setTimeout(() => setArmedDelete((cur) => (cur === id ? null : cur)), 3000);
      return;
    }
    setArmedDelete(null);
    void skillsApi.delete(id)
      .then(() => void loadSkills())
      .catch((e: unknown) => {
        setSaveError(
          e instanceof Error ? e.message : String(e),
        );
      });
  }, [armedDelete, loadSkills]);

  return (
    <div className={cn("space-y-4", className)}>
      <div className="flex items-center justify-between">
        <p className="text-xs text-muted-foreground">
          {t("settings.skillsDesc")}
        </p>
        <Button variant="outline" size="sm" className="h-8 gap-1 text-xs" onClick={() => setShowAdd(!showAdd)}>
          <Plus className="h-3.5 w-3.5" />
          {t("common.add")}
        </Button>
      </div>

      {/* ── Ecosystem compat ─────────────────────────────────── */}
      <div className="rounded-lg border border-border bg-card p-3">
        <p className="text-xs font-medium">
          {t("settings.skillsEcoCompat")}
        </p>
        <p className="mt-0.5 text-[10px] text-muted-foreground">
          {t("settings.skillsEcoCompatDesc")}
        </p>
        <div className="mt-2 space-y-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[11px] text-foreground">
              {t("settings.skillsEcoClaude")}
            </span>
            <Switch
              checked={skillsCompat.claudeEnabled}
              onCheckedChange={(v) => void setSkillsCompat("claudeEnabled", v)}
              aria-label="Claude skills"
            />
          </div>
          <div className="flex items-center justify-between">
            <span className="text-[11px] text-foreground">
              {t("settings.skillsEcoCursor")}
            </span>
            <Switch
              checked={skillsCompat.cursorEnabled}
              onCheckedChange={(v) => void setSkillsCompat("cursorEnabled", v)}
              aria-label="Cursor skills"
            />
          </div>
        </div>
      </div>

      {showAdd && (
        <div className="rounded-lg border border-border bg-card p-3 space-y-3">
          <Input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="skill-name" className="h-8 text-xs" />
          <Input value={newDesc} onChange={(e) => setNewDesc(e.target.value)} placeholder={t("settings.skillsDescPlaceholder")} className="h-8 text-xs" />
          {saveError && (
            <p className="text-[10px] text-destructive">{t("settings.skillsSaveFailed", { error: saveError })}</p>
          )}
          <div className="flex gap-2">
            <Button size="sm" className="h-8 text-xs" disabled={!newName.trim()} onClick={handleAdd}>
              {t("common.save")}
            </Button>
            <Button size="sm" variant="ghost" className="h-8 text-xs" onClick={() => setShowAdd(false)}>
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      )}

      {loading ? (
        <div className="flex items-center justify-center py-12">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : skills.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-12 text-center">
          <Zap className="h-8 w-8 text-muted-foreground/40 mb-2" />
          <p className="text-xs text-muted-foreground">{t("settings.skillsEmpty")}</p>
        </div>
      ) : (
        <div className="space-y-2">
          {skills.map((skill) => (
            <div key={skill.id} className="flex items-center gap-2 rounded-lg border border-border bg-card px-3 py-2.5">
              <Zap className="h-4 w-4 shrink-0 text-primary" />
              <div className="flex-1 min-w-0">
                <p className="text-xs font-medium">{skill.name}</p>
                <p className="text-[10px] text-muted-foreground truncate">{skill.description}</p>
              </div>
              <span className="shrink-0 rounded bg-secondary px-1.5 py-0.5 text-[9px] uppercase tracking-wide text-muted-foreground">
                {skill.source}
              </span>
              {skill.source !== "bundled" && (
                <Button
                  variant={armedDelete === skill.id ? "destructive" : "ghost"}
                  size="sm"
                  className="h-7 shrink-0 text-muted-foreground hover:text-destructive"
                  onClick={() => handleDelete(skill.id)}
                  title={armedDelete === skill.id ? t("settings.skillsConfirmDelete", { defaultValue: "再次点击确认删除" }) : t("common.delete")}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
