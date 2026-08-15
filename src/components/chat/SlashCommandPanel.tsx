/**
 * SlashCommandPanel — command palette triggered by "/" in input.
 *
 * Features:
 * - Fuzzy search across commands
 * - Keyboard navigation (up/down, enter, escape)
 * - Command categories
 * - Description + example for each command
 *
 * Reference: Notion slash commands, GitHub Copilot slash.
 */

import { useState, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Search, Terminal, FileSearch, GitBranch, Zap, HelpCircle } from "lucide-react";
import { cn } from "@/lib/utils";

export interface SlashCommand {
  id: string;
  label: string;
  description: string;
  /** i18n key for the description (falls back to `description`). */
  descriptionKey?: string;
  example?: string;
  icon: typeof Terminal;
  category: "code" | "search" | "git" | "agent" | "other";
  action: () => void;
  /** When false, selecting the command runs `action` WITHOUT filling the
   *  input with the command label — used for mode/persona switches. */
  fillInput?: boolean;
}

interface SlashCommandPanelProps {
  commands: SlashCommand[];
  query: string;
  onSelect: (command: SlashCommand) => void;
  onClose: () => void;
  className?: string;
}

/**
 * Category config with colors.
 */
const CATEGORY_CONFIG: Record<
  SlashCommand["category"],
  { label: string; color: string }
> = {
  code: { label: "代码", color: "text-blue-600 dark:text-blue-400" },
  search: { label: "搜索", color: "text-amber-600 dark:text-amber-400" },
  git: { label: "Git", color: "text-purple-600 dark:text-purple-400" },
  agent: { label: "Agent", color: "text-emerald-600 dark:text-emerald-400" },
  other: { label: "其他", color: "text-muted-foreground" },
};

/**
 * Default commands if none provided.
 */
export const DEFAULT_COMMANDS: SlashCommand[] = [
  {
    id: "explain",
    label: "/explain",
    descriptionKey: "chat.slashExplainDesc",
    description: "解释当前代码或选中的内容",
    example: "/explain",
    icon: HelpCircle,
    category: "code",
    action: () => {},
  },
  {
    id: "refactor",
    label: "/refactor",
    descriptionKey: "chat.slashRefactorDesc",
    description: "重构当前代码，提高可读性和性能",
    example: "/refactor",
    icon: Zap,
    category: "code",
    action: () => {},
  },
  {
    id: "find",
    label: "/find",
    descriptionKey: "chat.slashFindDesc",
    description: "在项目中搜索特定内容",
    example: "/find function_name",
    icon: FileSearch,
    category: "search",
    action: () => {},
  },
  {
    id: "git",
    label: "/git",
    descriptionKey: "chat.slashGitDesc",
    description: "执行 Git 相关操作",
    example: "/git status",
    icon: GitBranch,
    category: "git",
    action: () => {},
  },
];

export function SlashCommandPanel({
  commands = DEFAULT_COMMANDS,
  query,
  onSelect,
  onClose,
  className,
}: SlashCommandPanelProps) {
  const { t } = useTranslation();
  const [selectedIndex, setSelectedIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  // Filter commands based on query
  const filteredCommands = useMemo(() => {
    if (!query) return commands;
    const lowerQuery = query.toLowerCase();
    return commands.filter(
      (cmd) =>
        cmd.label.toLowerCase().includes(lowerQuery) ||
        cmd.description.toLowerCase().includes(lowerQuery)
    );
  }, [commands, query]);

  // Group by category
  const groupedCommands = useMemo(() => {
    const groups: Record<string, SlashCommand[]> = {};
    for (const cmd of filteredCommands) {
      if (!groups[cmd.category]) groups[cmd.category] = [];
      groups[cmd.category].push(cmd);
    }
    return groups;
  }, [filteredCommands]);

  // Flat list for keyboard navigation
  const flatCommands = useMemo(() => {
    return Object.values(groupedCommands).flat();
  }, [groupedCommands]);

  // Reset selection when filtered list changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  // Keyboard navigation — guarded: once focus leaves the textarea (e.g. the
  // user opened the model dropdown while the panel was up), arrows/Enter/Esc
  // must not be hijacked from whatever now has focus.
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const editable =
        el instanceof HTMLTextAreaElement ||
        el instanceof HTMLInputElement ||
        (el instanceof HTMLElement && el.isContentEditable);
      if (!editable) return;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, flatCommands.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const cmd = flatCommands[selectedIndex];
        if (cmd) {
          onSelect(cmd);
        }
      } else if (e.key === "Escape") {
        onClose();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [flatCommands, selectedIndex, onSelect, onClose]);

  // Scroll selected into view
  useEffect(() => {
    const selected = listRef.current?.querySelector(`[data-index="${selectedIndex}"]`);
    selected?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  if (flatCommands.length === 0) {
    return (
      <div
        className={cn(
          "rounded-lg border border-border bg-popover shadow-lg",
          "w-full max-w-md p-4 text-center text-sm text-muted-foreground",
          className
        )}
      >
        <Search className="mx-auto mb-2 h-5 w-5 text-muted-foreground/50" />
        {t("chat.noCommandFound", { defaultValue: "未找到命令" })}
      </div>
    );
  }

  let currentIndex = 0;

  return (
    <div
      ref={listRef}
      className={cn(
        "overflow-y-auto rounded-lg border border-border bg-popover shadow-lg",
        "w-full max-w-md max-h-[300px]",
        className
      )}
    >
      {Object.entries(groupedCommands).map(([category, cmds]) => {
        const config = CATEGORY_CONFIG[category as SlashCommand["category"]];
        return (
          <div key={category}>
            <div className="bg-muted/50 px-3 py-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
              {t(`chat.commandCategory.${category}`, { defaultValue: config.label })}
            </div>
            {cmds.map((cmd) => {
              const index = currentIndex++;
              const isSelected = index === selectedIndex;
              const Icon = cmd.icon;

              return (
                <button
                  key={cmd.id}
                  data-index={index}
                  onClick={() => onSelect(cmd)}
                  onMouseEnter={() => setSelectedIndex(index)}
                  className={cn(
                    "flex w-full items-start gap-3 px-3 py-2.5 text-left transition-colors",
                    isSelected ? "bg-primary/5" : "hover:bg-muted/50"
                  )}
                >
                  <Icon className={cn("mt-0.5 h-4 w-4 shrink-0", config.color)} />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium text-foreground">
                        {cmd.label}
                      </span>
                      {cmd.example && (
                        <span className="text-[10px] text-muted-foreground/50">
                          {cmd.example}
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-muted-foreground truncate">
                      {cmd.descriptionKey ? t(cmd.descriptionKey) : cmd.description}
                    </p>
                  </div>
                </button>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}
