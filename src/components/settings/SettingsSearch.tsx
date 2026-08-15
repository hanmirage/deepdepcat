import type { KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Search, SearchX, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SETTINGS_GROUPS, type SettingsCategory } from "@/config/settings";
import type { SettingsSearchResult } from "@/config/settingsSearch";
import { cn } from "@/lib/utils";

export interface SettingsSearchNavProps {
  query: string;
  onQueryChange: (query: string) => void;
  results: SettingsSearchResult[];
  activeCategory: SettingsCategory;
  hideVision: boolean;
  onSelect: (category: SettingsCategory, entryKey?: string) => void;
}

function Highlight({ text, query }: { text: string; query: string }) {
  const q = query.trim().toLowerCase();
  if (!q) return <>{text}</>;
  const index = text.toLowerCase().indexOf(q);
  if (index < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, index)}
      <span className="rounded-sm bg-primary/15 text-primary">
        {text.slice(index, index + q.length)}
      </span>
      {text.slice(index + q.length)}
    </>
  );
}

function SearchInput({
  query,
  onQueryChange,
  onEnter,
  placeholder,
}: {
  query: string;
  onQueryChange: (query: string) => void;
  onEnter: () => void;
  placeholder: string;
}) {
  const { t } = useTranslation();
  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") onQueryChange("");
    else if (event.key === "Enter") onEnter();
  };
  return (
    <div className="relative mb-2">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/60" />
      <Input
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        aria-label={placeholder}
        className="h-8 pl-8 pr-7 text-xs"
      />
      {query.trim().length > 0 && (
        <button
          onClick={() => onQueryChange("")}
          aria-label={t("common.clear")}
          className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground/60 hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

function GroupedNav({
  activeCategory,
  hideVision,
  onSelect,
}: {
  activeCategory: SettingsCategory;
  hideVision: boolean;
  onSelect: (category: SettingsCategory) => void;
}) {
  const { t } = useTranslation();
  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="space-y-2">
        {SETTINGS_GROUPS.map((group) => (
          <div key={group.label}>
            <p className="mb-1 px-2.5 text-[9px] font-semibold uppercase tracking-widest text-muted-foreground/50">
              {t(group.label)}
            </p>
            <div className="space-y-0.5">
              {group.items
                .filter((cat) => !hideVision || cat.id !== "vision")
                .map((cat) => {
                  const Icon = cat.icon;
                  const active = activeCategory === cat.id;
                  return (
                    <button
                      key={cat.id}
                      onClick={() => onSelect(cat.id)}
                      className={cn(
                        "flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-xs transition-colors",
                        active
                          ? "bg-secondary font-medium text-secondary-foreground"
                          : "text-muted-foreground hover:bg-secondary/50 hover:text-foreground",
                      )}
                    >
                      <Icon className="h-3.5 w-3.5 shrink-0" />
                      {t(cat.label)}
                    </button>
                  );
                })}
            </div>
          </div>
        ))}
      </div>
    </ScrollArea>
  );
}

function ResultGroup({
  result,
  query,
  active,
  onSelect,
}: {
  result: SettingsSearchResult;
  query: string;
  active: boolean;
  onSelect: (category: SettingsCategory, entryKey?: string) => void;
}) {
  const Icon = result.category.icon;
  return (
    <div className="rounded-md p-1">
      <button
        onClick={() => onSelect(result.category.id)}
        className={cn(
          "flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-xs transition-colors",
          active
            ? "bg-secondary font-medium text-secondary-foreground"
            : "text-muted-foreground hover:bg-secondary/50 hover:text-foreground",
        )}
      >
        <Icon className="h-3.5 w-3.5 shrink-0" />
        <span className="truncate">
          <Highlight text={result.category.label} query={query} />
        </span>
        {result.entryMatches.length === 0 && (
          <span className="ml-auto shrink-0 text-[9px] uppercase tracking-wide text-muted-foreground/50">
            {result.groupLabel}
          </span>
        )}
      </button>
      {result.entryMatches.length > 0 && (
        <div className="ml-5 mt-0.5 space-y-0.5 border-l border-border/60 pl-3">
          {result.entryMatches.map((match) => (
            <button
              key={match.entry.key}
              onClick={() => onSelect(result.category.id, match.entry.key)}
              className="block w-full rounded-md px-2 py-1 text-left hover:bg-secondary/50"
            >
              <span className="block truncate text-[11px] font-medium text-foreground">
                <Highlight text={match.label} query={query} />
              </span>
              {match.desc && (
                <span className="mt-0.5 block line-clamp-2 text-[10px] leading-snug text-muted-foreground">
                  <Highlight text={match.desc} query={query} />
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function ResultsList({
  results,
  query,
  activeCategory,
  onSelect,
}: {
  results: SettingsSearchResult[];
  query: string;
  activeCategory: SettingsCategory;
  onSelect: (category: SettingsCategory, entryKey?: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <ScrollArea className="min-h-0 flex-1">
      <p className="px-2.5 pb-1 text-[9px] font-semibold uppercase tracking-widest text-muted-foreground/50">
        {t("settings.search.results", { count: results.length })}
      </p>
      <div className="space-y-1">
        {results.map((result) => (
          <ResultGroup
            key={result.category.id}
            result={result}
            query={query}
            active={activeCategory === result.category.id}
            onSelect={onSelect}
          />
        ))}
      </div>
    </ScrollArea>
  );
}

function EmptyResults() {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col items-center justify-center px-4 py-10 text-center">
      <SearchX className="mb-2 h-7 w-7 text-muted-foreground/40" />
      <p className="text-xs text-muted-foreground">{t("settings.search.empty")}</p>
      <p className="mt-1 text-[10px] text-muted-foreground/60">
        {t("settings.search.emptyHint")}
      </p>
    </div>
  );
}

export function SettingsSearchNav({
  query,
  onQueryChange,
  results,
  activeCategory,
  hideVision,
  onSelect,
}: SettingsSearchNavProps) {
  const { t } = useTranslation();
  const searching = query.trim().length > 0;
  const visibleResults = results.filter(
    (result) => !(hideVision && result.category.id === "vision"),
  );
  const firstResult = visibleResults[0];
  const selectFirst = () => {
    if (firstResult) onSelect(firstResult.category.id, firstResult.entryMatches[0]?.entry.key);
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <SearchInput
        query={query}
        onQueryChange={onQueryChange}
        onEnter={selectFirst}
        placeholder={t("settings.search.placeholder")}
      />
      {!searching ? (
        <GroupedNav
          activeCategory={activeCategory}
          hideVision={hideVision}
          onSelect={(cat) => onSelect(cat)}
        />
      ) : visibleResults.length === 0 ? (
        <EmptyResults />
      ) : (
        <ResultsList
          results={visibleResults}
          query={query}
          activeCategory={activeCategory}
          onSelect={onSelect}
        />
      )}
    </div>
  );
}
