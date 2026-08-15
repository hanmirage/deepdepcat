import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

// @tauri-apps/cli uses Vite for the frontend build.
// This config ensures:
// 1. React plugin is active
// 2. "@" alias points to src/
// 3. Fixed port 5173 for Tauri dev server
// 4. src-tauri/ is excluded from file watching
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  clearScreen: false,
  // Shiki worker needs code-splitting — ESM worker format (iife can't).
  worker: {
    format: "es",
  },
  build: {
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        manualChunks: {
          react: ["react", "react-dom", "react-i18next"],
          tauri: ["@tauri-apps/api", "@tauri-apps/plugin-dialog", "@tauri-apps/plugin-fs", "@tauri-apps/plugin-store"],
          markdown: ["react-markdown", "rehype-highlight", "rehype-raw", "rehype-sanitize", "remark-gfm", "remark-breaks"],
          radix: [
            "@radix-ui/react-avatar",
            "@radix-ui/react-collapsible",
            "@radix-ui/react-dialog",
            "@radix-ui/react-dropdown-menu",
            "@radix-ui/react-scroll-area",
            "@radix-ui/react-separator",
            "@radix-ui/react-slot",
            "@radix-ui/react-switch",
            "@radix-ui/react-tabs",
            "@radix-ui/react-tooltip",
          ],
          utils: ["class-variance-authority", "clsx", "tailwind-merge", "zustand", "lucide-react"],
        },
      },
    },
  },
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
