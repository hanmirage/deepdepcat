/**
 * highlight.worker — Shiki syntax highlighting off the main thread.
 *
 * Receives streaming code (growing prefix) per block key, highlights the
 * full text with Shiki, and returns the token list. The React main thread
 * renders tokens with stable index keys — React reconciliation keeps the
 * unchanged prefix DOM spans untouched and only the tail re-renders.
 *
 * Built on createHighlighterCore with EXPLICIT language/theme registration
 * (not the full bundled registry) so the worker bundle only carries the
 * languages we actually stream.
 *
 * Protocol:
 *   → { type: "highlight", key, generation, text, language, theme }
 *   ← { type: "result", key, generation, language, text, tokens, error? }
 *   → { type: "dispose", key }
 *
 * Supersede: the main thread bumps `generation` per request for a key and
 * drops results whose generation is stale (a newer request was already
 * sent — its result will arrive later).
 */

import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import javascript from "shiki/langs/javascript.mjs";
import typescript from "shiki/langs/typescript.mjs";
import tsx from "shiki/langs/tsx.mjs";
import jsx from "shiki/langs/jsx.mjs";
import python from "shiki/langs/python.mjs";
import json from "shiki/langs/json.mjs";
import html from "shiki/langs/html.mjs";
import xml from "shiki/langs/xml.mjs";
import css from "shiki/langs/css.mjs";
import sql from "shiki/langs/sql.mjs";
import shellscript from "shiki/langs/shellscript.mjs";
import bash from "shiki/langs/bash.mjs";
import markdown from "shiki/langs/markdown.mjs";
import rust from "shiki/langs/rust.mjs";
import go from "shiki/langs/go.mjs";
import java from "shiki/langs/java.mjs";
import cpp from "shiki/langs/cpp.mjs";
import csharp from "shiki/langs/csharp.mjs";
import ruby from "shiki/langs/ruby.mjs";
import php from "shiki/langs/php.mjs";
import kotlin from "shiki/langs/kotlin.mjs";
import swift from "shiki/langs/swift.mjs";
import { flattenShikiTokens, type HighlightPayload } from "./codeTokens";

/**
 * DeepDepCat syntax themes — mirrors the CSS --code-* palette (index.css)
 * so streaming (Shiki worker), the completed hljs pass, and the lightweight
 * tokenizer all show ONE color scheme. Kept as hex here because the worker
 * renders inline colors (CSS variables don't cross the worker boundary).
 */
type ShikiTheme = {
  name: string;
  type: "light" | "dark";
  colors: Record<string, string>;
  tokenColors: { scope?: string[]; settings: { foreground?: string; fontStyle?: string } }[];
};

function deepdepcatTheme(type: "light" | "dark"): ShikiTheme {
  const d = type === "dark";
  const keyword = d ? "#b08eeb" : "#6f35d4";
  const string = d ? "#64d894" : "#298e53";
  const number = d ? "#f6a951" : "#c16515";
  const comment = d ? "#9ca3af" : "#737373";
  const tag = d ? "#68a9f3" : "#1b66bb";
  const attr = d ? "#b08eeb" : "#6f35d4";
  const atrule = d ? "#b08eeb" : "#6f35d4";
  const prop = d ? "#63a4ee" : "#1f61ad";
  const fn = d ? "#75b3f0" : "#2966a3";
  const fg = d ? "#e5e7eb" : "#111827";
  const bg = d ? "#1e1e1e" : "#ffffff";
  return {
    name: type === "dark" ? "deepdepcat-dark" : "deepdepcat-light",
    type,
    colors: { background: bg, foreground: fg },
    tokenColors: [
      // Comments
      { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: comment, fontStyle: "italic" } },
      // Keywords / control flow
      { scope: ["keyword", "keyword.control", "keyword.operator", "storage", "storage.type", "storage.modifier", "keyword.operator.new"], settings: { foreground: keyword } },
      // Strings / templates / regex
      { scope: ["string", "string.quoted", "string.template", "string.regexp", "punctuation.definition.string"], settings: { foreground: string } },
      // Numbers / constants
      { scope: ["constant.numeric", "constant.language", "constant.character", "constant.other"], settings: { foreground: number } },
      // Function names / support functions
      { scope: ["entity.name.function", "support.function", "meta.function-call", "variable.function"], settings: { foreground: fn } },
      // Tags / markup (html/xml)
      { scope: ["entity.name.tag", "punctuation.definition.tag"], settings: { foreground: tag } },
      // Attribute names
      { scope: ["entity.other.attribute-name"], settings: { foreground: attr } },
      // CSS at-rules / properties
      { scope: ["keyword.control.at-rule"], settings: { foreground: atrule } },
      { scope: ["support.type.property-name", "meta.property-name"], settings: { foreground: prop } },
      // Types / classes
      { scope: ["entity.name.type", "support.type", "entity.name.class"], settings: { foreground: fn, fontStyle: "bold" } },
    ],
  };
}

const deepdepcatLight = deepdepcatTheme("light");
const deepdepcatDark = deepdepcatTheme("dark");

let highlighterPromise: Promise<HighlighterCore> | null = null;

function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      langs: [
        javascript,
        typescript,
        tsx,
        jsx,
        python,
        json,
        html,
        xml,
        css,
        sql,
        shellscript,
        bash,
        markdown,
        rust,
        go,
        java,
        cpp,
        csharp,
        ruby,
        php,
        kotlin,
        swift,
      ],
      themes: [deepdepcatLight, deepdepcatDark],
      engine: createJavaScriptRegexEngine(),
    });
  }
  return highlighterPromise;
}

/** Normalize our language labels to Shiki's registered names. */
function normalizeLanguage(lang: string): string {
  switch (lang.toLowerCase()) {
    case "js":
    case "mjs":
    case "cjs":
      return "javascript";
    case "ts":
      return "typescript";
    case "py":
      return "python";
    case "sh":
    case "shell":
    case "zsh":
      return "shellscript";
    case "rs":
      return "rust";
    case "c++":
    case "cc":
    case "h":
    case "hpp":
      return "cpp";
    case "cs":
    case "c#":
      return "csharp";
    case "rb":
      return "ruby";
    case "kt":
      return "kotlin";
    case "golang":
      return "go";
    default:
      return lang.toLowerCase();
  }
}

const ctx = self as unknown as {
  onmessage: ((e: MessageEvent<WorkerMsg>) => void) | null;
  postMessage: (msg: unknown) => void;
};

type WorkerMsg =
  | {
      type: "highlight";
      key: string;
      generation: number;
      text: string;
      language: string;
      theme: "light" | "dark";
    }
  | { type: "dispose"; key: string };

ctx.onmessage = (e: MessageEvent<WorkerMsg>) => {
  const msg = e.data;
  if (msg.type === "dispose") return;
  void (async () => {
    const { key, generation, text, language, theme } = msg;
    const payload: HighlightPayload = { tokens: [], text, language };
    try {
      const highlighter = await getHighlighter();
      const lang = normalizeLanguage(language);
      if (highlighter.getLoadedLanguages().includes(lang)) {
        const lines = highlighter.codeToTokensBase(text, {
          lang: lang as never,
          theme: theme === "dark" ? "deepdepcat-dark" : "deepdepcat-light",
        });
        payload.tokens = flattenShikiTokens(lines);
      }
    } catch {
      payload.error = true;
    }
    ctx.postMessage({
      type: "result",
      key,
      generation,
      language,
      text,
      tokens: payload.tokens,
      error: payload.error,
    });
  })();
};
