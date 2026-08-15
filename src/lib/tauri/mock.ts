/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

import type { Session } from "@/types";
import type { DepworkTask, BrowserStatus, SystemInfo } from "./types";

// ── Mock data ─────────────────────────────────────────────────

// MOCK_MODELS removed — models now come from settingsStore providers
// or the Tauri backend, never hardcoded.

export const MOCK_SESSION: Session = {
  id: "mock-session-1",
  title: "Mock Session",
  model: "deepseek-chat",
  provider: "deepseek",
  status: "active",
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
  total_usage: { prompt_tokens: 0, completion_tokens: 0 },
  turn_count: 0,
  system_prompt: "",
  work_mode: "code",
  pinned: false,
};

export const MOCK_SYSTEM_INFO: SystemInfo = {
  os: "browser",
  arch: "mock",
  cpu_count: navigator.hardwareConcurrency ?? 4,
  total_memory_mb: 8192,
  app_version: "0.0.0-mock",
};

export const MOCK_REPLY = `I'm a mock assistant running in browser dev mode.\n\nYou said: `;

export const MOCK_TASKS: DepworkTask[] = [
  {
    id: "mock-task-1",
    description: "分析项目依赖结构并生成报告",
    status: "completed",
    context_paths: [],
    created_at: new Date(Date.now() - 1000 * 60 * 30).toISOString(),
  },
  {
    id: "mock-task-2",
    description: "重构认证模块到 trait 抽象",
    status: "running",
    context_paths: ["src/auth/"],
    created_at: new Date(Date.now() - 1000 * 60 * 10).toISOString(),
  },
  {
    id: "mock-task-3",
    description: "为 API 端点添加 OpenAPI 文档",
    status: "pending",
    context_paths: ["src-tauri/src/commands/"],
    created_at: new Date(Date.now() - 1000 * 60 * 5).toISOString(),
  },
];

export const MOCK_BROWSER_STATUS: BrowserStatus = {
    running: false,
    url: null,
    title: null,
    awaiting_takeover: false,
    takeover_reason: null,
    profile: null,
    headless: false,
    download_dir: null,
  };
