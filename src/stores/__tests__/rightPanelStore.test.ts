/**
 * Right-panel store tests — transient-stack memory, panel open/close
 * semantics, chat jumps (reveal file) and the event-driven auto-show contract.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { useRightPanelStore, MAX_PANES } from "@/stores/rightPanelStore";

const MODE_WIDTH = { code: 640, depwork: 640 };
const MODE_SIGNAL = { code: false, depwork: false };
const MODE_SUPPRESS = { code: false, depwork: false };

describe("right panel store", () => {
  beforeEach(() => {
    useRightPanelStore.setState({
      open: false,
      width: MODE_WIDTH,
      panes: { code: ["activity"], depwork: ["activity"] },
      pendingFile: { code: null, depwork: null },
      pendingPreview: { code: null, depwork: null },
      activitySignal: MODE_SIGNAL,
      autoOpenSuppressed: MODE_SUPPRESS,
    });
  });

  it("remembers the active panes per mode", () => {
    useRightPanelStore.getState().openPane("code", "files");
    useRightPanelStore.getState().openPane("depwork", "browser");

    expect(useRightPanelStore.getState().panes).toEqual({
      code: ["activity", "files"],
      depwork: ["activity", "browser"],
    });
  });

  it("openPane opens the panel and clears auto-open suppression for that mode", () => {
    useRightPanelStore.setState({
      autoOpenSuppressed: { code: true, depwork: false },
    });
    useRightPanelStore.getState().openPane("code", "files");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(true);
    expect(s.panes.code).toEqual(["activity", "files"]);
    expect(s.autoOpenSuppressed.code).toBe(false);
    // The other mode's suppression is untouched.
    expect(s.autoOpenSuppressed.depwork).toBe(false);
  });

  it("openPane moves an already-open pane to the focused position", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["files", "activity"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().openPane("code", "files");

    expect(useRightPanelStore.getState().panes.code).toEqual(["activity", "files"]);
  });

  it("openPane replaces the oldest pane when the transient stack is full", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["activity", "files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().openPane("code", "plan");

    expect(useRightPanelStore.getState().panes.code).toHaveLength(MAX_PANES);
    expect(useRightPanelStore.getState().panes.code).toEqual(["files", "plan"]);
  });

  it("closePane removes one pane and keeps the others", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["activity", "files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().closePane("code", "activity");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual(["files"]);
    expect(s.open).toBe(true);
  });

  it("closePane never hides the panel when the last pane closes", () => {
    useRightPanelStore.setState({ open: true });
    useRightPanelStore.getState().closePane("code", "activity");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual([]);
    expect(s.open).toBe(true);
  });

  it("dismiss hides the panel and suppresses auto-show for that mode", () => {
    useRightPanelStore.setState({ open: true });
    useRightPanelStore.getState().dismiss("code");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(false);
    expect(s.autoOpenSuppressed.code).toBe(true);
  });

  it("notifyActivity auto-opens the activity pane once when quiet", () => {
    useRightPanelStore.setState({
      panes: { code: ["files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().notifyActivity("code");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(true);
    expect(s.panes.code).toEqual(["files", "activity"]);
    expect(s.activitySignal.code).toBe(true);
  });

  it("notifyActivity only pulses when the user dismissed the panel", () => {
    useRightPanelStore.setState({
      autoOpenSuppressed: { code: false, depwork: true },
    });
    useRightPanelStore.getState().notifyActivity("depwork");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(false);
    expect(s.activitySignal.depwork).toBe(true);
  });

  it("notifyActivity adds the activity pane when the panel is already open", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().notifyActivity("code");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(true);
    expect(s.panes.code).toEqual(["files", "activity"]);
    expect(s.activitySignal.code).toBe(true);
  });

  it("notifySubagents auto-opens the subagents pane when quiet", () => {
    useRightPanelStore.setState({
      open: false,
      panes: { code: [], depwork: ["activity"] },
    });
    useRightPanelStore.getState().notifySubagents("code");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(true);
    expect(s.panes.code).toEqual(["subagents"]);
    expect(s.activitySignal.code).toBe(true);
  });

  it("notifySubagents adds the subagents pane when the panel is already open", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().notifySubagents("code");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual(["files", "subagents"]);
    expect(s.activitySignal.code).toBe(true);
  });

  it("clearSubagents removes the subagents pane", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["subagents"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().clearSubagents("code");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual([]);
    expect(s.panes.depwork).toEqual(["activity"]);
  });

  it("notifyTask auto-opens the task pane when quiet", () => {
    useRightPanelStore.setState({
      open: false,
      panes: { code: [], depwork: ["activity"] },
    });
    useRightPanelStore.getState().notifyTask("code");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(true);
    expect(s.panes.code).toEqual(["task"]);
    expect(s.activitySignal.code).toBe(true);
  });

  it("notifyTask adds the task pane when the panel is already open", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().notifyTask("code");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual(["files", "task"]);
    expect(s.activitySignal.code).toBe(true);
  });

  it("clearTask removes the task pane", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["task"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().clearTask("code");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual([]);
    expect(s.panes.depwork).toEqual(["activity"]);
  });

  it("clearActivity drops the activity pane and shrinks toward the default width", () => {
    useRightPanelStore.setState({
      open: true,
      width: { code: 720, depwork: 720 },
      panes: { code: ["activity"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().clearActivity("code");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual([]);
    expect(s.panes.depwork).toEqual(["activity"]);
    expect(s.open).toBe(true);
    expect(s.width.code).toBe(300);
    expect(s.width.depwork).toBe(720);
  });

  it("clearActivity is a no-op when the activity pane is absent", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().clearActivity("code");

    expect(useRightPanelStore.getState().panes.code).toEqual(["files"]);
  });

  it("removePane drops the pane without re-anchoring anything", () => {
    useRightPanelStore.setState({
      open: true,
      panes: { code: ["files"], depwork: [] },
    });
    useRightPanelStore.getState().removePane("code", "files");

    const s = useRightPanelStore.getState();
    expect(s.panes.code).toEqual([]);
    expect(s.open).toBe(true);
  });

  it("removePane no-ops when the pane is absent", () => {
    useRightPanelStore.setState({
      panes: { code: ["files"], depwork: [] },
    });
    useRightPanelStore.getState().removePane("code", "plan");

    expect(useRightPanelStore.getState().panes.code).toEqual(["files"]);
  });

  it("revealFile opens the files pane and records the pending file per mode", () => {
    useRightPanelStore.getState().revealFile("code", "D:\\proj\\src\\main.ts");

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(true);
    expect(s.panes.code).toEqual(["activity", "files"]);
    expect(s.pendingFile.code).toBe("D:\\proj\\src\\main.ts");
    expect(s.pendingFile.depwork).toBeNull();
  });

  it("clearPendingFile consumes the pending request per mode", () => {
    useRightPanelStore.setState({
      pendingFile: { code: "a.ts", depwork: "b.docx" },
    });
    useRightPanelStore.getState().clearPendingFile("code");

    const s = useRightPanelStore.getState();
    expect(s.pendingFile.code).toBeNull();
    expect(s.pendingFile.depwork).toBe("b.docx");
  });

  it("pendingPreview is stashed and consumed per mode", () => {
    useRightPanelStore
      .getState()
      .setPendingPreview("depwork", { url: "https://example.com", path: null });
    useRightPanelStore.getState().setPendingPreview("code", { url: null, path: "D:\\r.html" });

    let s = useRightPanelStore.getState();
    expect(s.pendingPreview.depwork?.url).toBe("https://example.com");
    expect(s.pendingPreview.code?.path).toBe("D:\\r.html");

    useRightPanelStore.getState().clearPendingPreview("code");
    s = useRightPanelStore.getState();
    expect(s.pendingPreview.code).toBeNull();
    expect(s.pendingPreview.depwork).not.toBeNull();
  });

  it("toggle toggles the panel and flips suppression for that mode", () => {
    useRightPanelStore.getState().toggle("code");
    expect(useRightPanelStore.getState().open).toBe(true);
    expect(useRightPanelStore.getState().autoOpenSuppressed.code).toBe(false);

    useRightPanelStore.getState().toggle("code");
    expect(useRightPanelStore.getState().open).toBe(false);
    expect(useRightPanelStore.getState().autoOpenSuppressed.code).toBe(true);
  });

  it("clamps the width to the drag range", () => {
    useRightPanelStore.getState().setWidth("code", 9999);
    expect(useRightPanelStore.getState().width.code).toBe(1280);
    useRightPanelStore.getState().setWidth("code", 10);
    expect(useRightPanelStore.getState().width.code).toBe(280);
  });

  it("openPane auto-expands to the browser width when the browser opens", () => {
    useRightPanelStore.setState({
      open: true,
      width: { code: 640, depwork: 640 },
      panes: { code: ["activity"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().openPane("depwork", "browser");

    expect(useRightPanelStore.getState().width.depwork).toBe(1080);
  });

  it("closePane shrinks the browser width back to the pane default", () => {
    useRightPanelStore.setState({
      open: true,
      width: { code: 1080, depwork: 1080 },
      panes: { code: ["activity"], depwork: ["activity", "browser"] },
    });
    useRightPanelStore.getState().closePane("depwork", "browser");

    expect(useRightPanelStore.getState().width.depwork).toBe(300);
  });

  it("openPane auto-expands the panel when a second pane opens", () => {
    useRightPanelStore.setState({
      open: true,
      width: { code: 500, depwork: 500 },
      panes: { code: ["activity"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().openPane("code", "files");

    expect(useRightPanelStore.getState().width.code).toBe(720);
  });

  it("closePane shrinks back when only one pane remains at the auto width", () => {
    useRightPanelStore.setState({
      open: true,
      width: { code: 720, depwork: 720 },
      panes: { code: ["activity", "files"], depwork: ["activity"] },
    });
    useRightPanelStore.getState().closePane("code", "files");

    expect(useRightPanelStore.getState().width.code).toBe(300);
  });
});
