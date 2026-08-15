//! MarkdownRenderer — shared markdown rendering with full plugin stack.
//!
//! Bundles: GFM (tables, strikethrough, task lists), line-break preservation,
//! raw HTML passthrough, and syntax highlighting. Custom component overrides
//! add copy buttons, language labels, external links, and scroll wrappers.

import { memo, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeHighlight from "rehype-highlight";
import { cn } from "@/lib/utils";
import { looksLikeFilePath } from "@/lib/inlineMarkdown";
import { CodeBlock, extractLanguage } from "@/components/chat/CodeBlock";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";

/** Safe class names (alphanumeric/dash/space) — enough for hljs token
 *  classes and Tailwind utilities without opening up arbitrary markup. */
const SAFE_CLASS_RE = /^[\w-]+( [\w-]+)*$/;

/**
 * rehype-sanitize's default schema strips every className except a few
 * GitHub-specific ones, which would kill rehype-highlight's `hljs`
 * classes on code/pre/span. Classes cannot execute code, so allow them on
 * those three elements while keeping the rest of the default schema
 * (no scripts, no event handlers, no iframes, no javascript: URLs).
 */
const markdownSchema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    code: [
      ...(defaultSchema.attributes?.code ?? []),
      ["className", SAFE_CLASS_RE],
    ],
    pre: [
      ...(defaultSchema.attributes?.pre ?? []),
      ["className", SAFE_CLASS_RE],
    ],
    span: [
      ...(defaultSchema.attributes?.span ?? []),
      ["className", SAFE_CLASS_RE],
    ],
  },
};

// ── Utility to extract text from ReactNode ───────────────────

/** Extract raw text from a ReactNode (handles string, array, element). */
function nodeToText(node: ReactNode): string {
  if (node == null || node === false) return "";
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeToText).join("");
  if (typeof node === "object" && "props" in node) {
    const el = node as { props?: { children?: ReactNode } };
    return nodeToText(el.props?.children);
  }
  return "";
}

// ── Custom element renderers ──────────────────────────────────

function makeComponents(interactiveFiles: boolean): Components {
  return {
    // Links: open in new tab, add safety attrs
    a({ node: _node, ...props }) {
      return (
        <a
          {...props}
          target="_blank"
          rel="noopener noreferrer"
        />
      );
    },

    // Code blocks: enhanced with line numbers, language icons, file path
    pre({ children, node: _node }) {
      // Extract code text and language from the pre > code structure
      const codeEl = children as { props?: { className?: string; children?: ReactNode } } | undefined;
      const className = codeEl?.props?.className;
      const lang = extractLanguage(className);
      const code = nodeToText(codeEl?.props?.children);

      return (
        <CodeBlock
          code={code}
          language={lang ?? undefined}
        >
          {children}
        </CodeBlock>
      );
    },

    // Inline code: subtle background — unless it reads as a file path, in
    // which case it becomes a clickable file reference (teal + opens in the
    // workspace). Only in chat contexts (`interactiveFiles`); document
    // previews keep plain code.
    code({ className, children, ...props }) {
      // Block code (inside <pre>) — let CodeBlock handle styling,
      // just pass through with the hljs classes from rehype-highlight.
      if (className && /language-/.test(className)) {
        return (
          <code className={className} {...props}>
            {children}
          </code>
        );
      }
      const text = nodeToText(children);
      if (interactiveFiles && looksLikeFilePath(text)) {
        return (
          <button
            type="button"
            title={text}
            onClick={(e) => {
              e.stopPropagation();
              const mode = useAppStore.getState().mode;
              useRightPanelStore.getState().revealFile(mode, text);
            }}
            className="code-tok-file cursor-pointer rounded bg-muted/60 px-1 py-0.5 font-mono text-xs align-baseline transition-colors hover:underline hover:underline-offset-2"
          >
            {children}
          </button>
        );
      }
      // Inline code
      return (
        <code
          className="rounded bg-muted px-1 py-0.5 text-xs"
          {...props}
        >
          {children}
        </code>
      );
    },

    // Tables: wrap in horizontal scroll
    table({ children }) {
      return (
        <div className="my-3 overflow-x-auto">
          <table>{children}</table>
        </div>
      );
    },

    // Images: constrain width + height so a model-emitted giant image can
    // never blow up the message column; lazy-load below the fold.
    img({ node: _node, ...props }) {
      const { alt, ...rest } = props;
      return (
        <img
          {...rest}
          alt={alt ?? ""}
          loading="lazy"
          className="max-h-[60vh] max-w-full rounded-lg object-contain"
        />
      );
    },
  };
}

// ── Main component ────────────────────────────────────────────

export interface MarkdownRendererProps {
  content: string;
  className?: string;
  /** When true, inline code that reads as a file path renders as a clickable
   *  file reference (opens in the workspace). Chat contexts pass true;
   *  document previews leave it off so paths stay plain text. */
  interactiveFiles?: boolean;
}

function MarkdownRendererImpl({
  content,
  className,
  interactiveFiles = false,
}: MarkdownRendererProps) {
  return (
    <div
      className={cn(
        "prose prose-sm dark:prose-invert max-w-none",
        className,
      )}
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        // rehype-raw parses model/tool-emitted HTML, then rehype-sanitize
        // strips scripts/event handlers/iframes before anything is rendered.
        // Order matters: sanitize must run AFTER raw so injected HTML never
        // reaches the DOM unsanitized.
        rehypePlugins={[rehypeRaw, [rehypeSanitize, markdownSchema], rehypeHighlight]}
        components={makeComponents(interactiveFiles)}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

export const MarkdownRenderer = memo(MarkdownRendererImpl);
