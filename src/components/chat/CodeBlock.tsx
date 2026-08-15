/**
 * CodeBlock — enhanced code block with line numbers, file path, and language icon.
 *
 * Features:
 * - File path breadcrumb in header
 * - Language icon detection
 * - Line numbers (toggleable)
 * - Copy button
 * - Syntax highlighting integration
 */

import { useState, useMemo, type ReactNode } from "react";
import { Check, Copy, FileCode, Braces, Terminal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

// Language icon mapping
const LANGUAGE_ICONS: Record<string, typeof FileCode> = {
  typescript: Braces,
  ts: Braces,
  tsx: Braces,
  javascript: Braces,
  js: Braces,
  jsx: Braces,
  python: Terminal,
  py: Terminal,
  rust: Terminal,
  rs: Terminal,
  go: Terminal,
  java: Terminal,
  bash: Terminal,
  sh: Terminal,
  shell: Terminal,
};

interface CodeBlockProps {
  code: string;
  language?: string;
  filePath?: string;
  showLineNumbers?: boolean;
  children?: ReactNode;
}

/**
 * Extract language from className (e.g., "language-typescript" → "typescript")
 */
export function extractLanguage(className?: string): string | null {
  if (!className) return null;
  const match = /language-(\w[\w+-]*)/.exec(className);
  return match?.[1]?.toLowerCase() ?? null;
}

/**
 * Get display name for language. Exported so the streaming code block (which
 * renders its own lighter header) shows the SAME label the completed block
 * will — the streaming → completed swap must not change the header text.
 */
export function getLanguageDisplayName(lang: string): string {
  const names: Record<string, string> = {
    ts: "TypeScript",
    tsx: "TSX",
    js: "JavaScript",
    jsx: "JSX",
    py: "Python",
    rs: "Rust",
    go: "Go",
    java: "Java",
    bash: "Bash",
    sh: "Shell",
  };
  return names[lang] ?? lang.charAt(0).toUpperCase() + lang.slice(1);
}

/**
 * Get icon for language — exported so the streaming code block (which renders
 * its own lighter header) shares the same icon source, keeping the visual
 * language consistent across streaming → completed.
 */
export function getLanguageIcon(lang: string) {
  const normalized = lang.toLowerCase();
  return LANGUAGE_ICONS[normalized] ?? FileCode;
}

/**
 * Format file path for display (shorten long paths).
 */
function formatFilePath(path: string): string {
  const parts = path.split(/[/\\]/);
  if (parts.length <= 3) return path;
  return "…/" + parts.slice(-2).join("/");
}

/**
 * Get line count for a code string. Exported so the streaming block's live
 * header (line-count label) matches the completed block's.
 */
export function getLineCount(code: string): number {
  return code.split("\n").length;
}

export function CodeBlock({
  code,
  language,
  filePath,
  showLineNumbers = true,
  children,
}: CodeBlockProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  
  const lines = useMemo(() => {
    if (!showLineNumbers) return [];
    return code.split("\n").map((_, i) => i + 1);
  }, [code, showLineNumbers]);

  const displayLang = language ? getLanguageDisplayName(language) : "Code";
  const Icon = language ? getLanguageIcon(language) : FileCode;
  const lineCount = getLineCount(code);

  const handleCopy = () => {
    navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }).catch(() => {
      // Clipboard unavailable — the code stays selectable.
    });
  };

  return (
    <div className="paper-settle group/my-3 overflow-hidden rounded-lg border border-border bg-card">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-border bg-muted/60 px-3 py-2">
        <div className="flex items-center gap-2">
          <Icon className="h-4 w-4 text-muted-foreground" />
          
          {filePath ? (
            <div className="flex items-center gap-1 text-xs">
              <span className="text-muted-foreground/60 font-mono">
                {formatFilePath(filePath)}
              </span>
              <span className="text-border">|</span>
              <span className="text-muted-foreground uppercase text-[10px] tracking-wider">
                {displayLang}
              </span>
            </div>
          ) : (
            <span className="text-xs text-muted-foreground uppercase tracking-wider">
              {displayLang}
            </span>
          )}
          
          {lineCount > 1 && (
            <span className="text-[10px] text-muted-foreground/50 ml-1">
              {lineCount} lines
            </span>
          )}
        </div>

        <button
          onClick={handleCopy}
          aria-label={copied ? t("chat.copied", { defaultValue: "已复制" }) : t("chat.copyCode", { defaultValue: "复制代码" })}
          className={cn(
            "flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] transition-colors",
            copied
              ? "text-green-600"
              : "text-muted-foreground hover:bg-muted hover:text-foreground"
          )}
        >
          {copied ? (
            <Check className="h-3.5 w-3.5" />
          ) : (
            <Copy className="h-3.5 w-3.5" />
          )}
          {copied ? t("chat.copied", { defaultValue: "已复制" }) : t("chat.copyCode", { defaultValue: "复制" })}
        </button>
      </div>

      {/* Code content */}
      <div className="relative flex">
        {/* Line numbers */}
        {showLineNumbers && lines.length > 0 && (
          <div className="select-none border-r border-border/50 bg-muted/30 py-3 px-2 text-right">
            {lines.map((line) => (
              <div
                key={line}
                className="text-[11px] leading-5 text-muted-foreground/40 font-mono"
              >
                {line}
              </div>
            ))}
          </div>
        )}

        {/* Code */}
        <div className="flex-1 overflow-x-auto">
          <pre className="p-3 text-xs leading-5">
            {children ?? (
              <code className="font-mono text-foreground/90">{code}</code>
            )}
          </pre>
        </div>
      </div>
    </div>
  );
}
