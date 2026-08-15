import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "node_modules/**",
      "src-tauri/target/**",
      "src-tauri/gen/**",
      "server/**",
      "*.config.*",
    ],
  },
  {
    // Root build/deploy helper — plain Node script, not part of the app.
    files: ["script.js"],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.browser, ...globals.es2022 },
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "no-warning-comments": [
        "error",
        { terms: ["TODO", "FIXME", "HACK", "XXX"], location: "start" },
      ],
      "max-lines": ["error", { max: 500, skipBlankLines: true, skipComments: true }],
      // Known debt: ~87 functions still exceed 80 lines (stream listener,
      // large JSX components). Kept as a WARNING so it stays visible without
      // blocking merges; refactor in follow-up passes.
      "max-lines-per-function": [
        "warn",
        { max: 80, skipBlankLines: true, skipComments: true, IIFEs: false },
      ],
      "no-console": "warn",
    },
  },
  {
    // Bench harness runs under Node (fetch/process/setTimeout etc. are host
    // globals, not app browser globals).
    files: ["bench/**/*.{js,mjs}"],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
  },
  {
    // Workload fixtures are a git-reset sandbox used for scoring — their
    // contents are reference material and must not be "fixed" for lint.
    files: ["bench/work/**/*.js", "bench/work-src/**/*.js"],
    rules: {
      "@typescript-eslint/no-unused-vars": "off",
    },
  },
  {
    // Test files enumerate cases linearly — the 500-line source budget would
    // force artificial consolidation that hides coverage intent. Source
    // files keep the limit.
    files: ["**/__tests__/**/*.{ts,tsx}"],
    rules: {
      "max-lines": "off",
    },
  },
  {
    files: ["vite.config.ts", "vitest.config.ts", "postcss.config.js", "tailwind.config.ts"],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
);
