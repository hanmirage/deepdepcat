/**
 * markdownHeal — streaming markdown healing.
 *
 * While text streams in, inline markers arrive BEFORE their closing pair
 * (`**bold` is three tokens in, the closing `**` is still coming). The
 * old renderer kept the unclosed tail as plain text — so `**` was visible
 * as literal asterisks until the pair closed, and only then "popped" into
 * bold. Claude-style streaming renders the emphasis AS it types.
 *
 * `healInline` appends the missing closing markers before parsing, so a
 * half-typed `**加粗` renders as bold immediately and stays bold when the
 * real closing marker arrives (the healed string and the natural string
 * parse to the same tokens — no visual pop).
 *
 * Rules:
 *  - strong `**`, em `*`, inline code `` ` ``, del `~~` are healed only
 *    when the open marker has CONTENT after it (a bare trailing `**` that
 *    is still being typed stays literal — no phantom empty bold).
 *  - links `[t](u)` are NOT healed — an unclosed `[` stays plain text
 *    (URLs are error-prone to guess; opencode's remend does the same
 *    "text-only" downgrade).
 *  - nesting is single-level (matches inlineMarkdown's scope). An unclosed
 *    marker opened INSIDE another marker is left literal — its real closer
 *    is ambiguous and injecting one would pop away at turn_end. Only
 *    top-level unclosed markers are healed.
 */

/** Marker kinds tracked by the healer, in parse precedence order. */
type MarkerKind = "strong" | "del" | "code" | "em";

interface OpenMarker {
  kind: MarkerKind;
  /** Whether any plain content appeared after this marker opened. */
  hadContent: boolean;
  /** Opened while another marker was already open (nested emphasis). A
   *  nested unclosed marker is left literal — its real closer is ambiguous,
   *  and appending one injects a phantom delimiter that pops away at
   *  turn_end. Only top-level unclosed markers are healed. */
  nested: boolean;
}

const MARKER_LENGTH: Record<MarkerKind, number> = {
  strong: 2,
  del: 2,
  code: 1,
  em: 1,
};

const CLOSING: Record<MarkerKind, string> = {
  strong: "**",
  del: "~~",
  code: "`",
  em: "*",
};

/** Append missing closing markers to a partially-streamed inline string. */
export function healInline(text: string): string {
  const open: OpenMarker[] = [];
  let out = "";
  let i = 0;

  while (i < text.length) {
    const rest = text.slice(i);
    let kind: MarkerKind | null = null;

    if (rest.startsWith("**")) kind = "strong";
    else if (rest.startsWith("~~")) kind = "del";
    else if (rest.startsWith("`")) kind = "code";
    else if (rest.startsWith("*")) kind = "em";

    if (kind !== null) {
      const len = MARKER_LENGTH[kind];
      out += rest.slice(0, len);
      let existing: OpenMarker | undefined;
      for (let k = open.length - 1; k >= 0; k--) {
        if (open[k].kind === kind) {
          existing = open[k];
          break;
        }
      }
      if (existing) {
        // Closing marker — remove the matching opener.
        open.splice(open.indexOf(existing), 1);
      } else {
        open.push({ kind, hadContent: false, nested: open.length > 0 });
      }
      i += len;
      continue;
    }

    out += text[i];
    if (open.length > 0) {
      for (const m of open) m.hadContent = true;
    }
    i += 1;
  }

  // Close remaining open markers that have content after them, inner-most
  // (most recently opened) first. Nested markers are skipped — their closer
  // is ambiguous (see `nested`), so healing them would inject a phantom.
  for (let j = open.length - 1; j >= 0; j--) {
    const m = open[j];
    if (m.hadContent && !m.nested) {
      out += CLOSING[m.kind];
    }
  }

  return out;
}
