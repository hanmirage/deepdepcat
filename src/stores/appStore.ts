/**
 * Global application state (Zustand) — assembled from focused slices.
 *
 * Manages: view (mode/theme/sidebar/settings/debug/system), workspace,
 * update + feature flags, and one-time startup init. Each slice lives in
 * `appStoreSlices/` so the store stays under the file-size budget while
 * the public API (`useAppStore`) remains unchanged for all selectors.
 */

import { create } from "zustand";
import { initSlice } from "@/stores/appStoreSlices/initSlice";
import { updateSlice } from "@/stores/appStoreSlices/updateSlice";
import { viewSlice } from "@/stores/appStoreSlices/viewSlice";
import { workspaceSlice } from "@/stores/appStoreSlices/workspaceSlice";
import type { AppState, StoreSet } from "@/stores/appStoreSlices/types";

export const useAppStore = create<AppState>((set, get) => ({
  ...viewSlice(set as StoreSet, get),
  ...workspaceSlice(set as StoreSet, get),
  ...updateSlice(set as StoreSet, get),
  ...initSlice(set as StoreSet, get),
  // Slices are individually typed as Partial; together they cover the full
  // contract (every AppState field is owned by exactly one slice).
} as AppState));
