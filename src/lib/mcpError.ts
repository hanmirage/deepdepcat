/**
 * MCP error analysis — turn raw backend error strings into actionable
 * guidance. A stdio server that fails to start (missing Python module,
 * command not found, WPS absent) surfaces as a wall of stderr; the settings
 * card should say WHAT is wrong and HOW to fix it, not dump the trace.
 */

/** A friendly, actionable breakdown of a raw MCP connection error. */
export interface McpErrorHint {
  /** Short title: what went wrong (localized by the caller's key). */
  titleKey: string;
  /** Fix suggestion (localized by the caller's key). */
  actionKey?: string;
  /** The raw error, shown collapsed so the friendly part leads. */
  raw: string;
}

/**
 * Classify a raw backend error message. Returns a hint when the failure is a
 * known, fixable pattern (missing module / bad command / absent WPS), else
 * null — the caller falls back to showing the raw message alone.
 */
export function analyzeMcpError(raw: string): McpErrorHint | null {
  const lower = raw.toLowerCase();

  // Missing Python module: `ModuleNotFoundError: No module named 'xxx'`.
  const modMatch = raw.match(/No module named '([^']+)'/);
  if (modMatch) {
    const mod = modMatch[1];
    return {
      titleKey: "settings.mcp.errorMissingModule",
      actionKey: "settings.mcp.errorMissingModuleFix",
      raw,
    };
  }

  // Python/CLI not found or not a recognized command.
  if (
    lower.includes("not found") &&
    (lower.includes("python") || lower.includes("npx") || lower.includes("command"))
  ) {
    return { titleKey: "settings.mcp.errorCommandMissing", raw };
  }

  // WPS Office COM not available (COM dispatch fails at export/init).
  if (lower.includes("wps") && (lower.includes("com") || lower.includes("application"))) {
    return { titleKey: "settings.mcp.errorWpsNotInstalled", raw };
  }

  return null;
}
