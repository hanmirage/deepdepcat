/**
 * ModelSelector — dropdown for switching AI models.
 *
 * Pure component: receives models + selection + callback as props.
 * No store import — the parent decides which store to wire up.
 *
 * Models are grouped by provider and show context window size as a hint.
 */

import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, Check, Search } from "lucide-react";
// Direct imports skip the package index.js (which pulls in @lobehub/ui's
// IconAvatar — a dependency we don't install). Mono/Color only need react.
import DeepSeekMono from "@lobehub/icons/es/DeepSeek/components/Mono";
import DeepSeekColor from "@lobehub/icons/es/DeepSeek/components/Color";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ModelWithPricing } from "@/config/models";
import { resolveContextWindow } from "@/stores/settingsStore";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";

export interface ModelSelectorProps {
  models: ModelWithPricing[];
  selectedModel: ModelWithPricing | null;
  onSelectModel: (model: ModelWithPricing) => void;
  className?: string;
}

/** localStorage key for the "recently used" model list (UI convenience). */
const RECENT_MODELS_KEY = "deepdepcat.recentModels";
const MAX_RECENT_MODELS = 5;

function loadRecentModels(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_MODELS_KEY);
    const parsed = JSON.parse(raw ?? "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((x): x is string => typeof x === "string")
      .slice(0, MAX_RECENT_MODELS);
  } catch {
    return [];
  }
}

function saveRecentModels(ids: string[]): void {
  try {
    localStorage.setItem(RECENT_MODELS_KEY, JSON.stringify(ids));
  } catch {
    /* storage unavailable — recents just won't persist */
  }
}

/**
 * Short model label for the trigger button — keeps the toolbar ultra-compact.
 * DeepSeek models collapse to "Pro" / "Flash"; other models drop the provider
 * prefix (e.g. "GPT-4o" stays, "Zhipu GLM-4" → "GLM-4"). Falls back to the
 * full name when nothing matches.
 */
function shortModelName(model: ModelWithPricing): string {
  const name = model.name;
  const id = model.id.toLowerCase();
  const provider = model.provider?.toLowerCase() ?? "";

  // DeepSeek: match by id or name → "Pro" / "Flash".
  if (provider.includes("deepseek") || id.includes("deepseek")) {
    if (id.includes("flash")) return "Flash";
    if (id.includes("pro")) return "Pro";
  }

  // Generic: drop the provider prefix (e.g. "DeepSeek V4 Pro" → "V4 Pro").
  if (provider && name.toLowerCase().startsWith(provider + " ")) {
    return name.slice(provider.length + 1);
  }
  const idx = name.toLowerCase().indexOf(provider);
  if (idx === 0) return name.slice(provider.length);

  return name;
}

export function ModelSelector({
  models,
  selectedModel,
  onSelectModel,
  className,
}: ModelSelectorProps) {
  const { t } = useTranslation();
  const theme = useAppStore((s) => s.theme);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [recentIds, setRecentIds] = useState<string[]>(loadRecentModels);
  const searchRef = useRef<HTMLInputElement>(null);

  // Closing the dropdown resets the search so the next open starts fresh
  // (a stale filter would otherwise leave the list filtered with no way to
  // see it). Focus the search box on open so typing filters immediately.
  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) setQuery("");
    else setTimeout(() => searchRef.current?.focus(), 0);
  };

  // Query filter — matches name, id and provider prefix so a multi-provider
  // model list stays navigable without endless scrolling.
  const filteredModels = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return models;
    return models.filter(
      (m) =>
        m.name.toLowerCase().includes(q) ||
        m.id.toLowerCase().includes(q) ||
        (m.provider ?? "").toLowerCase().includes(q),
    );
  }, [models, query]);

  // Group models by provider
  const grouped = useMemo(() => {
    const map = new Map<string, ModelWithPricing[]>();
    for (const model of filteredModels) {
      const existing = map.get(model.provider) ?? [];
      existing.push(model);
      map.set(model.provider, existing);
    }
    return Array.from(map.entries());
  }, [filteredModels]);

  // Recently selected models (only shown when not searching).
  const recentModels = useMemo(() => {
    if (query.trim()) return [];
    const byId = new Map(models.map((m) => [m.id, m]));
    return recentIds
      .map((id) => byId.get(id))
      .filter((m): m is ModelWithPricing => Boolean(m));
  }, [models, recentIds, query]);

  const handleSelect = (model: ModelWithPricing) => {
    onSelectModel(model);
    setRecentIds((prev) => {
      const next = [model.id, ...prev.filter((id) => id !== model.id)].slice(
        0,
        MAX_RECENT_MODELS,
      );
      saveRecentModels(next);
      return next;
    });
  };

  const renderModelOption = (model: ModelWithPricing) => (
    <DropdownMenuItem
      key={model.id}
      onClick={() => handleSelect(model)}
      className="flex items-start gap-2 py-1.5"
    >
      <div className="flex-1">
        <span className="text-sm font-medium">{model.name}</span>
        <p className="mt-0.5 text-[10px] text-muted-foreground">
          {t("chat.contextWindow", { count: (resolveContextWindow(model.id, model.context_window ?? 32000) / 1000).toFixed(0) })}
        </p>
      </div>
      {selectedModel?.id === model.id && (
        <Check className="mt-0.5 h-3.5 w-3.5 text-primary" />
      )}
    </DropdownMenuItem>
  );

  // Show the DeepSeek brand icon next to the label when a DeepSeek model is
  // selected — monochrome in light mode, colored in dark mode.
  const isDeepSeek =
    selectedModel?.provider?.toLowerCase().includes("deepseek") ||
    selectedModel?.id.toLowerCase().includes("deepseek");

  return (
    <DropdownMenu open={open} onOpenChange={handleOpenChange}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className={cn(
            "h-7 shrink-0 gap-1.5 rounded-full border border-border/60 bg-muted/20 px-2 text-xs hover:bg-muted/40 hover:text-foreground",
            className,
          )}
        >
          {isDeepSeek && (
            <span className="flex items-center">
              {theme === "dark" ? <DeepSeekColor size={16} /> : <DeepSeekMono size={16} />}
            </span>
          )}
          {selectedModel ? shortModelName(selectedModel) : t("chat.selectModel")}
          <ChevronDown className="h-3 w-3 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="w-72"
      >
        {/* Search — stays fixed while the model list scrolls. */}
        <div className="border-b p-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("chat.searchModel")}
              aria-label={t("chat.searchModel")}
              className="h-7 pl-7 pr-2 text-xs shadow-none focus-visible:bg-background"
            />
          </div>
        </div>

        <div className="max-h-[min(22rem,50vh)] overflow-y-auto overscroll-contain p-1">
          {recentModels.length > 0 && (
            <>
              <DropdownMenuGroup>
                <DropdownMenuLabel className="text-[10px] uppercase tracking-wide text-muted-foreground">
                  {t("chat.recentModels")}
                </DropdownMenuLabel>
                {recentModels.map(renderModelOption)}
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
            </>
          )}

          {grouped.map(([provider, providerModels], groupIdx) => (
            <div key={provider}>
              {groupIdx > 0 && <DropdownMenuSeparator />}
              <DropdownMenuGroup>
                <DropdownMenuLabel className="text-[10px] uppercase tracking-wide text-muted-foreground">
                  {provider}
                </DropdownMenuLabel>
                {providerModels.map(renderModelOption)}
              </DropdownMenuGroup>
            </div>
          ))}

          {filteredModels.length === 0 && (
            <p className="px-3 py-6 text-center text-xs text-muted-foreground">
              {t("chat.noModelMatch")}
            </p>
          )}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
