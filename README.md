# DeepDepCat

**Dual-workflow AI desktop workbench** — one native application for both coding and document/office automation, natively optimized for DeepSeek V4.

<p align="center">
  <img src="https://img.shields.io/github/v/release/hanmirage/deepdepcat?label=version&color=4a9eff" alt="version" />
  <img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="license" />
  <img src="https://img.shields.io/badge/platform-Windows%20(x64)-lightgrey" alt="platform" />
  <a href="https://deepdepcat.hsmiai.xyz"><img src="https://img.shields.io/badge/website-deepdepcat.hsmiai.xyz-blue" alt="website" /></a>
  <a href="https://github.com/hanmirage/deepdepcat/releases"><img src="https://img.shields.io/github/downloads/hanmirage/deepdepcat/total" alt="downloads" /></a>
  <a href="https://github.com/hanmirage/deepdepcat/stargazers"><img src="https://img.shields.io/github/stars/hanmirage/deepdepcat" alt="stars" /></a>
</p>

<p align="center">
  <img src="./docs/assets/deepdepcat-icon.png" width="96" alt="DeepDepCat icon" />
</p>

## Why DeepDepCat

There are terminal coding agents (they only write code) and chat clients (they only talk). DeepDepCat is neither — it is both, in a single native desktop app:

- **Code workspace** — LSP-driven coding agent: subagent orchestration, multi-session parallel conversations, execution strategies (plan-execute / reflexion / coordinator / generate-review), checkpoint rollback, sandboxed tool execution, and permission modes from "confirm every change" to "fully automatic".
- **Depwork workspace** — document & office automation agent: Word/PPT/Excel generation, OCR, table processing, media conversion, desktop UI automation, batch file operations — with a live task panel showing step-by-step progress, pausable and resumable at any time.

DeepSeek V4 is not bolted on — it is the core: 1M context window, max reasoning effort, streaming thinking-mode display, and context-cache hit metrics you can actually see.

> This repository is the open-source home of the DeepDepCat desktop app, released under Apache-2.0. It is also the release channel and community hub. Issues, feature requests and feedback are very welcome.

## Screenshots

<p align="center">
  <img src="./docs/assets/screenshot-onboarding.png" alt="Onboarding" width="300" />
  <img src="./docs/assets/screenshot-code.png" alt="Code workspace" width="300" />
  <img src="./docs/assets/screenshot-depwork.jpg" alt="Depwork workspace" width="300" />
</p>

## Quick start

1. Download the latest Windows installer from [Releases](https://github.com/hanmirage/deepdepcat/releases), or visit the [official site](https://deepdepcat.hsmiai.xyz).
2. Open the app — **Settings → Model Providers**, select the built-in **DeepSeek** provider, and paste your [DeepSeek API Key](https://platform.deepseek.com/api_keys).
3. Add models `deepseek-v4-pro` / `deepseek-v4-flash` (1M context window by default).
4. Start working — Code workspace for code, Depwork workspace for documents.

## Key features

| Feature | Description |
|---------|-------------|
| Dual workflows | Coding + document/office automation in ONE app, each with its own tool chain, system prompt and agent definitions |
| Multi-session parallelism | Sessions run concurrently without blocking each other; live per-session indicators and one-click stop in the sidebar |
| Per-session terminal | A persistent interactive shell per chat session — switch sessions to switch terminals; supports python/vim/htop |
| Subagent orchestration | Complex tasks decomposed into parallel subagents with real-time activity tracking |
| Execution strategies | Standard / plan-execute / reflexion / coordinator / generate-review (independent evaluator) |
| Pause & resume | Tasks pause at checkpoints (context fully preserved) and resume anytime — not a kill |
| DeepSeek V4 native | 1M context, max reasoning effort, streaming thinking display, cache-hit metrics |
| Long-term memory | Cross-session project memory with automatic injection and Chinese-semantic retrieval (FTS5 CJK) |
| Skills & ecosystem | Own directory-based skills plus Claude/Cursor skill & plugin compatibility — zero format conversion |
| MCP support | First-class MCP (stdio / SSE / HTTP); external server tools merge into one registry |
| Checkpoint rollback | Undo any agent file change with one click — back to the pre-edit state |
| Trust & safety | 3-level permission modes (read-only / accept edits / allow all), sandboxed execution, bash AST security analysis, inline user confirmations |
| Silent updates | Small releases install automatically on exit, zero UI; major releases via the update button |

## Building from source

DeepDepCat is fully open source (Apache-2.0). To build the desktop app yourself:

1. Install [Rust](https://rustup.rs), [Node.js 18+](https://nodejs.org), and the [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/).
2. Run `npm install`.
3. Run `npm run tauri build` (or `npm run tauri dev` for development).

Source layout: [src](./src) — React frontend · [src-tauri](./src-tauri) — Rust backend · [server](./server) — FastAPI update/auth/telemetry backend · [depwork-mcp](./depwork-mcp) — WPS Office MCP server.

## WPS Office MCP server

**WPS Office MCP server** (`depwork-mcp/`) — control WPS Office (Writer / Calc / Impress) from any MCP-compatible AI client: document creation & editing, export to docx/xlsx/pptx/pdf, and live typewriter-style writing into a visible window. JSON project model + deferred COM rendering — editing never requires WPS to run.

Add it to your MCP client in one block:

```json
{
  "mcpServers": {
    "wps-office": {
      "command": "python",
      "args": ["-m", "wps_controller.mcp_server"]
    }
  }
}
```

See [depwork-mcp/README.md](./depwork-mcp/README.md) for the full tool list, CLI usage and JSON project format.

## Platform support

- **Windows (x64)** — available now (NSIS installer, silent updates)
- **macOS (Apple Silicon / Intel)** — cross-platform Tauri app; builds in progress
- **Linux** — planned

## Tech stack

- **Desktop shell**: [Tauri 2](https://tauri.app) (Rust backend + WebView2 / WebKit)
- **Frontend**: React 18 + TypeScript + Vite + Zustand
- **Backend services**: Python FastAPI + SQLite (updates, auth, telemetry, sync)
- **Models**: DeepSeek V4 (default), OpenAI, Anthropic, xAI Grok, local Ollama — OpenAI / Anthropic / Responses protocols

## Integration guide

- [DeepSeek official awesome list guide](./docs/deepdepcat.md) (English · [简体中文](./docs/deepdepcat.zh-CN.md))

## Co-creators

<a href="https://github.com/hanmirage"><img src="https://github.com/hanmirage.png" width="48" height="48" alt="hanmirage" /></a>
<a href="https://github.com/FengZi1221"><img src="https://github.com/FengZi1221.png" width="48" height="48" alt="FengZi1221" /></a>
<a href="https://github.com/liucy0727"><img src="https://github.com/liucy0727.png" width="48" height="48" alt="liucy0727" /></a>
<a href="https://github.com/lm35260"><img src="https://github.com/lm35260.png" width="48" height="48" alt="lm35260" /></a>
<a href="https://github.com/Noob8878"><img src="https://github.com/Noob8878.png" width="48" height="48" alt="Noob8878" /></a>

## License

[Apache-2.0](./LICENSE) © 2026 hanmirage
