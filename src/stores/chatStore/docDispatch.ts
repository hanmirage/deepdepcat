/**
 * Chat store — split by concern (types / prefs / mode detection / stream state).
 */

/** Document extensions the preview panel can open natively. */
const DOCUMENT_EXT_RE = /\.(?:docx|pptx|xlsx|html|pdf)(?![A-Za-z])/;

/** Extract a document path from a tool result (e.g. "Created Word
 *  document: D:\out\report.docx\n(12 KB, Word-compatible)"). Returns the
 *  last matched document path, or null. Exported for tests. */
export function extractDocumentPath(result: string): string | null {
  if (!DOCUMENT_EXT_RE.test(result)) return null;
  const matches = result.match(/[^\s\n"'()]+\.(?:docx|pptx|xlsx|html|pdf)/gi);
  if (!matches || matches.length === 0) return null;
  return matches[matches.length - 1].replace(/[.,;:]+$/, "");
}

/** Last path segment (both slash styles). Exported for tests. */
export function basenameOf(path: string): string {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return idx >= 0 ? path.slice(idx + 1) : path;
}

// ── Persisted chat preferences ────────────────────────────
