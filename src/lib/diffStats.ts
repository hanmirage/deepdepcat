/**
 * diffStats — shared diff statistics for streaming write tools.
 *
 * Extracted from ToolCallCard so both the tool card and the exec header can
 * show Claude/Codex-style LIVE counters: while a write tool is streaming its
 * arguments, the +N/-M counts are recomputed on every delta (write_file:
 * added grows with the streamed content; edit_file: computed as soon as
 * both texts are present, even while new_text is partial).
 */

export interface DiffStats {
  added: number;
  removed: number;
}

function parseArgs(args: string): Record<string, unknown> {
  try {
    return args ? JSON.parse(args) : {};
  } catch {
    return {};
  }
}

/** Compute diff line counts for a write tool's arguments.
 *  Returns null when the tool isn't a write tool or the texts aren't
 *  available yet (edit_file mid-stream with no new_text so far). */
export function computeDiffStats(toolName: string, args: string): DiffStats | null {
  const parsed = parseArgs(args);

  if (toolName === "edit_file" || toolName === "search_replace") {
    const oldText = typeof parsed.old_text === "string" ? parsed.old_text : null;
    const newText = typeof parsed.new_text === "string" ? parsed.new_text : null;
    if (!oldText || !newText) return null;

    const oldLines = oldText.split("\n");
    const newLines = newText.split("\n");
    const maxLen = Math.max(oldLines.length, newLines.length);

    let added = 0;
    let removed = 0;
    for (let i = 0; i < maxLen; i++) {
      const oldLine = i < oldLines.length ? oldLines[i] : null;
      const newLine = i < newLines.length ? newLines[i] : null;
      if (oldLine === null && newLine !== null) added++;
      else if (newLine === null && oldLine !== null) removed++;
      else if (oldLine !== newLine) {
        added++;
        removed++;
      }
    }
    return { added, removed };
  }

  if (toolName === "write_file") {
    const content = typeof parsed.content === "string" ? parsed.content : null;
    if (!content) return null;
    return { added: content.split("\n").length, removed: 0 };
  }

  return null;
}
