/**
 * ModelProviderAddForm — the "add provider" form + fetch-status helpers,
 * extracted from ModelProviderSettings.
 */

import { useTranslation } from "react-i18next";
import { Loader2, Download, Plus, Trash2, Eye, EyeOff } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { SettingSelect } from "@/components/settings/SettingSelect";
import { AlertCircle, CheckCircle2 } from "lucide-react";
import { contextWindowOptions, type ApiFormat, type ModelConfig } from "@/stores/settingsStore";
import { useState } from "react";
import { cn } from "@/lib/utils";

/** Password input with a show/hide toggle — used for every API key field. */
export function SecretInput({
  value,
  onChange,
  placeholder,
  className,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <Input
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className={cn("pr-7", className)}
      />
      <button
        type="button"
        onClick={() => setShow(!show)}
        className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
        aria-label={show ? t("settings.modelProviders.hideKey") : t("settings.modelProviders.showKey")}
      >
        {show ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
      </button>
    </div>
  );
}

export interface FetchState {
  loading: boolean;
  error: string | null;
  success: string | null;
}

export function renderFetchStatus(state: FetchState) {
  if (state.loading) return null;
  if (state.error) {
    return (
      <div className="flex items-center gap-1.5 rounded-md border border-destructive/30 bg-destructive/5 px-2 py-1 text-[11px] text-destructive">
        <AlertCircle className="h-3 w-3 shrink-0" />
        <span className="truncate">{state.error}</span>
      </div>
    );
  }
  if (state.success) {
    return (
      <div className="flex items-center gap-1.5 rounded-md border border-emerald-500/30 bg-emerald-500/5 px-2 py-1 text-[11px] text-emerald-600 dark:text-emerald-400">
        <CheckCircle2 className="h-3 w-3 shrink-0" />
        <span className="truncate">{state.success}</span>
      </div>
    );
  }
  return null;
}

export interface ModelProviderAddFormProps {
  show: boolean;
  onShowChange: (show: boolean) => void;
  newName: string;
  setNewName: (v: string) => void;
  newUrl: string;
  setNewUrl: (v: string) => void;
  newKey: string;
  setNewKey: (v: string) => void;
  newFormat: ApiFormat;
  setNewFormat: (v: ApiFormat) => void;
  newModels: ModelConfig[];
  setNewModels: (v: ModelConfig[]) => void;
  newFetchState: FetchState;
  handleNewFetch: () => void;
  handleAddProvider: () => void;
  onCancel: () => void;
  formatOptions: { value: string; label: string }[];
}

export function ModelProviderAddForm(props: ModelProviderAddFormProps) {
  const { t } = useTranslation();
  const {
    show, onShowChange, newName, setNewName, newUrl, setNewUrl, newKey, setNewKey,
    newFormat, setNewFormat, newModels, setNewModels, newFetchState,
    handleNewFetch, handleAddProvider, onCancel, formatOptions,
  } = props;
  return (
    <div className="space-y-3 rounded-lg border border-dashed border-border p-4">
      <p className="text-xs font-semibold">{t("settings.modelProviders.addProvider")}</p>
      <p className="text-[11px] text-muted-foreground">
        {t("settings.modelProviders.addProviderDesc")}
      </p>

      {show ? (
        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.name")}</label>
            <Input
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder={t("settings.modelProviders.namePlaceholder")}
              className="h-8 text-xs"
            />
          </div>

          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.baseUrl")}</label>
              <Input
                value={newUrl}
                onChange={(e) => setNewUrl(e.target.value)}
                placeholder="https://api.example.com/v1"
                className="h-8 text-xs"
              />
            </div>
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.apiKey")}</label>
              <SecretInput
                value={newKey}
                onChange={setNewKey}
                placeholder={t("settings.modelProviders.apiKeyPlaceholder")}
                className="h-8 text-xs"
              />
            </div>
          </div>

          <div>
            <label className="mb-1 block text-[10px] text-muted-foreground">{t("settings.modelProviders.apiFormat")}</label>
            <SettingSelect
              value={newFormat}
              onChange={(v) => setNewFormat(v as ApiFormat)}
                options={formatOptions}
              className="w-full"
            />
          </div>

          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5 text-xs"
              onClick={handleNewFetch}
              disabled={newFetchState.loading || !newUrl}
            >
              {newFetchState.loading ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Download className="h-3.5 w-3.5" />
              )}
              {newFetchState.loading ? t("settings.modelProviders.fetching") : t("settings.modelProviders.fetchModels")}
            </Button>
              {renderFetchStatus(newFetchState)}
          </div>

          {newModels.length > 0 && (
            <div>
              <label className="mb-1 block text-[10px] text-muted-foreground">
                {t("settings.modelProviders.fetchedModels", { count: newModels.length })}
              </label>
              <div className="max-h-40 space-y-1 overflow-y-auto">
                {newModels.map((model, idx) => (
                  <div key={idx} className="flex items-center gap-2">
                    <Input
                      value={model.name}
                      onChange={(e) => {
                        const updated = [...newModels];
                        updated[idx] = { ...updated[idx], name: e.target.value };
                        setNewModels(updated);
                      }}
                      className="h-7 flex-1 text-xs"
                    />
                    <SettingSelect
                      value={String(model.contextWindow)}
                      onChange={(v) => {
                        const updated = [...newModels];
                        updated[idx] = { ...updated[idx], contextWindow: Number(v) };
                        setNewModels(updated);
                      }}
                      options={contextWindowOptions(model.contextWindow)}
                      className="h-7 w-24 text-xs"
                    />
                    <button
                      onClick={() => setNewModels(newModels.filter((_, i) => i !== idx))}
                      className="text-muted-foreground transition-colors hover:text-destructive"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}

          <div className="flex gap-2">
            <Button
              size="sm"
              onClick={handleAddProvider}
              disabled={!newName || !newUrl}
              className="flex-1"
            >
              <Plus className="h-3.5 w-3.5" />
              {t("settings.modelProviders.addProviderBtn")}
            </Button>
            <Button
              size="sm"
                variant="ghost"
                onClick={onCancel}
            >
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      ) : (
        <Button
          variant="outline"
          size="sm"
          className="w-full"
            onClick={() => onShowChange(true)}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("settings.modelProviders.addProviderBtn")}
        </Button>
      )}
    </div>
  );
}
