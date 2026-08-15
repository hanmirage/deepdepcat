/**
 * User-message text → renderable parts.
 *
 * A user paste often contains fenced code blocks (```lang ... ```); the
 * bubble keeps them as real code blocks (copy + highlight) instead of a
 * monochrome pre-wrap blob. Everything else stays plain text so the user's
 * own wording is never reinterpreted as markdown.
 */

export interface UserTextPart {
  kind: "text" | "code";
  text: string;
  lang?: string;
}

const FENCE_RE = /^```([\w+-]*)\s*$/;

/** Split text into alternating plain-text and fenced-code parts. */
export function splitFencedCode(text: string): UserTextPart[] {
  const parts: UserTextPart[] = [];
  const lines = text.split("\n");
  let textBuf: string[] = [];
  let codeBuf: string[] = [];
  let lang = "";
  let opener = "";
  let inFence = false;

  const flushText = () => {
    if (textBuf.length > 0) {
      parts.push({ kind: "text", text: textBuf.join("\n") });
      textBuf = [];
    }
  };

  for (const line of lines) {
    const match = FENCE_RE.exec(line);
    if (match) {
      if (!inFence) {
        flushText();
        inFence = true;
        lang = match[1] ?? "";
        opener = line;
        codeBuf = [];
      } else {
        parts.push({ kind: "code", text: codeBuf.join("\n"), lang: lang || undefined });
        inFence = false;
      }
      continue;
    }
    if (inFence) codeBuf.push(line);
    else textBuf.push(line);
  }

  if (inFence) {
    // Unclosed fence — degrade to plain text (with the opener restored)
    // rather than silently dropping the user's content.
    textBuf.push(opener, ...codeBuf);
  }
  flushText();
  return parts;
}
