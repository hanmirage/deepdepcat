/**
 * Depwork state (Zustand).
 *
 * Manages the file-tree + preview-panel state for the
 * DepworkView layout:
 * - Root folder path (opened via Tauri dialog)
 * - File tree (lazy-loaded, expandable directories)
 * - Selected file (drives the right-hand preview panel)
 *
 * Chat messages, streaming, and input flow through depworkChatStore —
 * DepworkView uses DepworkInput/DepworkMessageList, fully isolated from chatStore.
 */

import { create } from "zustand";
import { logError } from "@/lib/logger";
import { pickFolder, listWorkspaceFiles, type WorkspaceFileEntry } from "@/lib/tauri";

/** A node in the file tree. Directories are expandable (lazy-loaded). */
export interface FileTreeNode {
  name: string;
  path: string;
  isDir: boolean;
  size: number | null;
  children?: FileTreeNode[];
  expanded?: boolean;
  loaded?: boolean;
  /** True while this directory's children are being fetched (guards rapid
   *  double-clicks from starting duplicate reads). */
  loading?: boolean;
}

interface DepworkState {
  // ── File tree ───────────────────────────────────────────────
  rootPath: string | null;
  tree: FileTreeNode[];
  treeLoading: boolean;

  // ── Preview ─────────────────────────────────────────────────
  selectedFile: FileTreeNode | null;

  // ── Actions ────────────────────────────────────────────────
  /** Open a document directory via the native picker. Returns the chosen
   *  path (null when the user cancelled). */
  openFolder: () => Promise<string | null>;
  /** Close the current document directory and clear the tree/preview. */
  clearFolder: () => void;
  toggleDirectory: (node: FileTreeNode) => Promise<void>;
  selectFile: (node: FileTreeNode) => void;
}
/** Convert a WorkspaceFileEntry to a FileTreeNode. */
function toNode(entry: WorkspaceFileEntry): FileTreeNode {
  return {
    name: entry.name,
    path: entry.path,
    isDir: entry.isDir,
    size: entry.size,
    loaded: entry.isDir ? false : true,
  };
}

/** Recursively load children for a directory node (one level deep). */
async function loadChildren(node: FileTreeNode): Promise<FileTreeNode[]> {
  const entries = await listWorkspaceFiles(node.path);
  return entries.map(toNode);
}

/** Recursively find and update a node in the tree by path. */
function updateNodeInTree(
  tree: FileTreeNode[],
  targetPath: string,
  updater: (node: FileTreeNode) => FileTreeNode,
): FileTreeNode[] {
  return tree.map((node) => {
    if (node.path === targetPath) return updater(node);
    if (node.children) {
      return { ...node, children: updateNodeInTree(node.children, targetPath, updater) };
    }
    return node;
  });
}

/** Recursively find a node in the tree by path. */
function findNodeInTree(
  tree: FileTreeNode[],
  targetPath: string,
): FileTreeNode | null {
  for (const node of tree) {
    if (node.path === targetPath) return node;
    if (node.children) {
      const found = findNodeInTree(node.children, targetPath);
      if (found) return found;
    }
  }
  return null;
}

export const useDepworkStore = create<DepworkState>((set, get) => ({
  rootPath: null,
  tree: [],
  treeLoading: false,
  selectedFile: null,

  // ── Actions ────────────────────────────────────────────────
  openFolder: async () => {
    const path = await pickFolder();
    if (!path) return null;

    set({ rootPath: path, treeLoading: true, selectedFile: null });
    try {
      const entries = await listWorkspaceFiles(path);
      set({ tree: entries.map(toNode), treeLoading: false });
    } catch (e) {
      logError("depworkStore", "Failed to list files:", e);
      set({ tree: [], treeLoading: false });
    }
    return path;
  },

  clearFolder: () =>
    set({ rootPath: null, tree: [], treeLoading: false, selectedFile: null }),

  toggleDirectory: async (node) => {
    if (!node.isDir) return;

    // Read the CURRENT store state, not the render-closure node: a rapid
    // double-click fires two calls before React re-renders, and both would
    // see the stale `loaded: false` — duplicating the directory read and
    // racing to set children.
    const fresh = findNodeInTree(get().tree, node.path);
    if (!fresh) return;

    // If already loaded, just toggle expanded
    if (fresh.loaded) {
      set((s) => ({
        tree: updateNodeInTree(s.tree, fresh.path, (n) => ({
          ...n,
          expanded: !n.expanded,
        })),
      }));
      return;
    }

    // Lazy-load children then expand. Mark the node as loading so a second
    // click in the same tick collapses/expands idempotently instead of
    // starting another read.
    if (fresh.loading) return;
    set((s) => ({
      tree: updateNodeInTree(s.tree, fresh.path, (n) => ({ ...n, loading: true })),
    }));

    try {
      const children = await loadChildren(fresh);
      set((s) => ({
        tree: updateNodeInTree(s.tree, fresh.path, (n) => ({
          ...n,
          children,
          loaded: true,
          expanded: true,
          loading: false,
        })),
      }));
    } catch (e) {
      logError("depworkStore", "Failed to expand directory:", e);
      set((s) => ({
        tree: updateNodeInTree(s.tree, fresh.path, (n) => ({
          ...n,
          loading: false,
        })),
      }));
    }
  },

  selectFile: (node) => {
    set({ selectedFile: node });
  },
}));
