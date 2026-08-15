/**
 * mcpError tests — the friendly-error classifier.
 */

import { describe, it, expect } from "vitest";
import { analyzeMcpError } from "@/lib/mcpError";

describe("analyzeMcpError", () => {
  it("recognizes a missing Python module", () => {
    const hint = analyzeMcpError(
      "MCP server closed the connection before answering 'initialize' — " +
        "Server stderr tail: python.exe: Error while finding module " +
        "specification for 'wps_controller.mcp_server' " +
        "(ModuleNotFoundError: No module named 'wps_controller')",
    );
    expect(hint).not.toBeNull();
    expect(hint!.titleKey).toBe("settings.mcp.errorMissingModule");
  });

  it("recognizes a missing python/command binary", () => {
    const hint = analyzeMcpError(
      "python: command not found — the server process likely exited",
    );
    expect(hint).not.toBeNull();
    expect(hint!.titleKey).toBe("settings.mcp.errorCommandMissing");
  });

  it("recognizes WPS COM being absent", () => {
    const hint = analyzeMcpError(
      "COM Error: <unknown> — Cannot connect to WPS Application",
    );
    expect(hint).not.toBeNull();
    expect(hint!.titleKey).toBe("settings.mcp.errorWpsNotInstalled");
  });

  it("returns null for unrecognized failures", () => {
    expect(analyzeMcpError("connection reset by peer")).toBeNull();
  });
});
