/**
 * Sidebar collapse tests — verify appStore's sidebar toggle semantics:
 *  - toggleSidebar flips collapsed and marks the choice as user-managed
 *  - setSidebarCollapsed does NOT mark user-managed (used by auto-collapse)
 */

import { describe, it, expect, beforeEach } from "vitest";
import { useAppStore } from "@/stores/appStore";

describe("sidebar collapse (appStore)", () => {
  beforeEach(() => {
    // Reset the store to a clean state for each test.
    useAppStore.setState({
      sidebarCollapsed: false,
      sidebarUserManaged: false,
    });
  });

  it("starts expanded and not user-managed", () => {
    const s = useAppStore.getState();
    expect(s.sidebarCollapsed).toBe(false);
    expect(s.sidebarUserManaged).toBe(false);
  });

  it("toggleSidebar collapses and marks user-managed", () => {
    useAppStore.getState().toggleSidebar();
    const s = useAppStore.getState();
    expect(s.sidebarCollapsed).toBe(true);
    expect(s.sidebarUserManaged).toBe(true);
  });

  it("toggleSidebar twice expands again (still user-managed)", () => {
    useAppStore.getState().toggleSidebar();
    useAppStore.getState().toggleSidebar();
    const s = useAppStore.getState();
    expect(s.sidebarCollapsed).toBe(false);
    expect(s.sidebarUserManaged).toBe(true);
  });

  it("setSidebarCollapsed does NOT mark user-managed (auto-collapse path)", () => {
    useAppStore.getState().setSidebarCollapsed(true);
    const s = useAppStore.getState();
    expect(s.sidebarCollapsed).toBe(true);
    expect(s.sidebarUserManaged).toBe(false);
  });

  it("auto-collapse can expand again while still auto-managed", () => {
    useAppStore.getState().setSidebarCollapsed(true);
    useAppStore.getState().setSidebarCollapsed(false);
    const s = useAppStore.getState();
    expect(s.sidebarCollapsed).toBe(false);
    expect(s.sidebarUserManaged).toBe(false);
  });
});
