/**
 * codeHighlight — lightweight streaming syntax highlighting.
 *
 * The final pass (turn_end) highlights code with rehype-highlight/Shiki.
 * But WHILE streaming, the fenced block is plain text — the single biggest
 * visual gap vs Claude (code is colored as it types). This module is a
 * dependency-free tokenizer that colors keywords / strings / comments /
 * numbers on the streaming path.
 *
 * Streaming-safe by construction: only CLOSED constructs match
 * (`"..."`, `//...`, block comments need their closing token), so an
 * unterminated string or block comment stays uncolored instead of
 * flashing wrong colors mid-stream.
 *
 * Languages: js (js/ts/jsx/tsx/mjs/cjs), py, sql, json, html (xml/svg),
 * css, sh, rust, go, java, c/c++/h, csharp/cs, ruby, php, kotlin/kt,
 * swift. Anything else renders plain (the turn_end pass still gets the
 * full Shiki treatment).
 */

export interface CodeToken {
  text: string;
  /** Token class (e.g. "code-tok-keyword") or null for plain text. */
  className: string | null;
}

interface TokenRule {
  className: string;
  re: RegExp;
}

const JS_KEYWORDS =
  "const let var function return if else for while do switch case break continue new class extends import from export default async await try catch finally throw typeof instanceof in of this null undefined true false void delete static get set yield".split(
    " ",
  );

const PY_KEYWORDS =
  "def return if elif else for while import from as class try except finally with lambda pass break continue and or not in is None True False global nonlocal yield raise assert del".split(
    " ",
  );

const SQL_KEYWORDS =
  "select from where insert into values update set delete create table drop alter join left right inner outer on group by order having limit offset distinct count sum avg min max and or not null primary key foreign references default index union all as exists case when then else end".split(
    " ",
  );

const JSON_KEYWORDS = ["true", "false", "null"];

const RUST_KEYWORDS =
  "fn let mut const struct enum impl trait pub use mod crate super self async await move match if else for while loop return break continue where type unsafe extern static ref dyn in as true false".split(
    " ",
  );

const GO_KEYWORDS =
  "func var const package import type struct interface map chan select go defer return if else for range switch case default break continue fallthrough goto true false nil".split(
    " ",
  );

const JAVA_KEYWORDS =
  "public private protected static final void class interface extends implements import package new return if else for while do switch case default break continue try catch finally throw throws true false null this super int long float double boolean char byte short".split(
    " ",
  );

const CPP_KEYWORDS =
  "include define ifdef ifndef endif pragma return if else for while do switch case default break continue struct class union typedef enum const static extern volatile register new delete public private protected virtual override template typename namespace using true false nullptr this int float double char long short unsigned signed bool void auto".split(
    " ",
  );

const CSHARP_KEYWORDS =
  "using namespace class interface struct enum public private protected internal static readonly const virtual override abstract sealed partial async await return if else for foreach while do switch case default break continue try catch finally throw new true false null this base".split(
    " ",
  );

const RUBY_KEYWORDS =
  "def end if elsif else unless while until for do return break next redo retry begin rescue ensure module class include extend require attr_reader attr_writer attr_accessor yield self nil true false".split(
    " ",
  );

const PHP_KEYWORDS =
  "function return if else elseif for foreach while do switch case default break continue try catch finally throw class interface extends implements public private protected static const new true false null echo print isset empty include require namespace use".split(
    " ",
  );

const KOTLIN_KEYWORDS =
  "fun val var class interface object data sealed enum companion init constructor override open abstract final internal private protected public inline suspend coroutine return if else when for while do break continue try catch finally throw true false null this super".split(
    " ",
  );

const SWIFT_KEYWORDS =
  "func var let class struct enum protocol extension import return if else guard for while repeat switch case default break continue fallthrough try catch throw defer true false nil self super in is as".split(
    " ",
  );

function keywordRule(keywords: string[]): TokenRule {
  return {
    className: "code-tok-keyword",
    re: new RegExp(`\\b(?:${keywords.join("|")})\\b`, "g"),
  };
}

function commentRule(pattern: string): TokenRule {
  // `m` so line comments (`//…`, `#…`, `--…`) match to end-of-line.
  return { className: "code-tok-comment", re: new RegExp(pattern, "gm") };
}

/** C-family line + block comments (`//…` / `/* … *​/`). */
function cCommentRule(): TokenRule {
  return commentRule(`\\/\\/.*$|\\/\\*[\\s\\S]*?\\*\\/`);
}

/** Hash line comments (`#…` — Python/Ruby/Shell). */
function hashCommentRule(): TokenRule {
  return commentRule(`#.*$`);
}

function stringRule(): TokenRule {
  // Closed double/single quotes and backticks (template literal).
  return {
    className: "code-tok-string",
    re: /"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`/g,
  };
}

function numberRule(): TokenRule {
  return { className: "code-tok-number", re: /\b\d+(?:\.\d+)?(?:e[+-]?\d+)?\b/gi };
}

function rulesFor(lang: string): TokenRule[] {
  switch (lang) {
    case "js":
    case "ts":
    case "jsx":
    case "tsx":
    case "mjs":
    case "cjs":
      return [
        cCommentRule(),
        stringRule(),
        keywordRule(JS_KEYWORDS),
        numberRule(),
      ];
    case "py":
    case "python":
      return [hashCommentRule(), stringRule(), keywordRule(PY_KEYWORDS), numberRule()];
    case "sql":
      return [commentRule(`--.*$|\\/\\*[\\s\\S]*?\\*\\/`), stringRule(), keywordRule(SQL_KEYWORDS), numberRule()];
    case "json":
      return [stringRule(), keywordRule(JSON_KEYWORDS), numberRule()];
    case "html":
    case "xml":
    case "svg":
      return [
        commentRule(`<!--[\\s\\S]*?-->`),
        { className: "code-tok-tag", re: /<\/?[a-zA-Z][\w-]*|\/?>/g },
        { className: "code-tok-attr", re: /[\w-]+(?==)/g },
        stringRule(),
      ];
    case "css":
    case "scss":
    case "less":
      return [
        commentRule(`\\/\\*[\\s\\S]*?\\*\\/`),
        { className: "code-tok-atrule", re: /@[\w-]+/g },
        { className: "code-tok-prop", re: /[\w-]+(?=\s*:)/g },
        stringRule(),
        numberRule(),
      ];
    case "sh":
    case "bash":
    case "shell":
    case "zsh":
      return [
        hashCommentRule(),
        stringRule(),
        keywordRule(["if", "then", "else", "fi", "for", "while", "do", "done", "case", "esac", "function", "return", "export", "local", "echo", "cd", "exit"]),
        numberRule(),
      ];
    case "rust":
    case "rs":
      return [cCommentRule(), stringRule(), keywordRule(RUST_KEYWORDS), numberRule()];
    case "go":
    case "golang":
      return [cCommentRule(), stringRule(), keywordRule(GO_KEYWORDS), numberRule()];
    case "java":
      return [cCommentRule(), stringRule(), keywordRule(JAVA_KEYWORDS), numberRule()];
    case "c":
    case "cpp":
    case "c++":
    case "h":
    case "hpp":
    case "cc":
      return [cCommentRule(), stringRule(), keywordRule(CPP_KEYWORDS), numberRule()];
    case "csharp":
    case "cs":
    case "c#":
      return [cCommentRule(), stringRule(), keywordRule(CSHARP_KEYWORDS), numberRule()];
    case "ruby":
    case "rb":
      return [hashCommentRule(), stringRule(), keywordRule(RUBY_KEYWORDS), numberRule()];
    case "php":
      return [commentRule(`\\/\\/.*$|\\/\\*[\\s\\S]*?\\*\\/|#.*$`), stringRule(), keywordRule(PHP_KEYWORDS), numberRule()];
    case "kotlin":
    case "kt":
      return [cCommentRule(), stringRule(), keywordRule(KOTLIN_KEYWORDS), numberRule()];
    case "swift":
      return [cCommentRule(), stringRule(), keywordRule(SWIFT_KEYWORDS), numberRule()];
    default:
      return [];
  }
}

/**
 * Tokenize a code string into highlighted tokens.
 * The stream reveals a growing prefix; tokenizing the prefix is
 * deterministic and cheap (linear scan, no parser).
 */
export function highlightTokens(code: string, lang: string | undefined): CodeToken[] {
  const rules = rulesFor(lang ?? "");
  if (rules.length === 0) {
    return code ? [{ text: code, className: null }] : [];
  }

  const matches: { index: number; text: string; className: string }[] = [];
  for (const rule of rules) {
    for (const m of code.matchAll(rule.re)) {
      matches.push({ index: m.index ?? 0, text: m[0], className: rule.className });
    }
  }
  // Order by position; rules are listed comments-first so overlaps at the
  // same position prefer the earlier rule (stable sort keeps rule order).
  matches.sort((a, b) => a.index - b.index || 0);

  const tokens: CodeToken[] = [];
  let pos = 0;
  for (const m of matches) {
    if (m.index < pos) continue;
    if (m.index > pos) {
      tokens.push({ text: code.slice(pos, m.index), className: null });
    }
    tokens.push({ text: m.text, className: m.className });
    pos = m.index + m.text.length;
  }
  if (pos < code.length) {
    tokens.push({ text: code.slice(pos), className: null });
  }
  return tokens;
}
