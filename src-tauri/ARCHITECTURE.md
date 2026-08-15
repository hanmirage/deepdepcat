# DeepDepCat 后端架构图

> 本文是 Rust 后端（`src-tauri/src/`）的**分层地图**：每个顶层模块属于哪层、依赖规则是什么、当前与目标的差距。
> 目标：**单向依赖**——entry → harness → capability → model → infra，一切从 bootstrap 装配。
> 依据：2026-08-13 三路诊断（分层扫描 / 循环映射 / 死代码扫描）。

## 依赖规则（一句话）

**低层永远不 import 高层**；跨层引用只能从上往下；`Tool` trait 住中立层；组合根独立成模块。

## 目标分层

```
┌─────────────────────────────────────────────────────────────────┐
│ bootstrap/   组合根：AppState + initialize + 启动装配            │
├─────────────────────────────────────────────────────────────────┤
│ entry        传输层：commands/ acp/ a2a/ automation/             │
│              （只转发，不写业务逻辑；chat.rs 是例外，待抽薄）     │
├─────────────────────────────────────────────────────────────────┤
│ harness      运行时核心：agent/（主循环 run/ + gates + recovery  │
│              + compaction + verification + multi_agent + workflow）│
├─────────────────────────────────────────────────────────────────┤
│ capability   能力层：tools/ permissions/ hooks/ memory/ skills/  │
│              （互不依赖，只依赖 model/ + infra/）                 │
├─────────────────────────────────────────────────────────────────┤
│ model        llm/（客户端 / 流式 / 协议适配 / 熔断）              │
├─────────────────────────────────────────────────────────────────┤
│ infra        纯基建：core/ storage/ observability/ workspace/    │
│              browser/ codebase/                                  │
└─────────────────────────────────────────────────────────────────┘
```

## 模块 → 层映射（当前）

| 模块 | 目标层 | 现状 | 差距 |
|---|---|---|---|
| `bootstrap/`（原 `core/state`） | 组合根 | ✅ **已升为顶层**（AppState + initialize + init/lifecycle/mode/plan/session） | AppState 60 字段待瘦身（S4） |
| `toolkit/`（新） | capability 底座 | ✅ **已建**（Tool trait + ToolContext + ToolResult + PermissionDecision + WorkMode/ToolScope） | — |
| `commands/` | entry | 30/33 文件纯转发；chat.rs(436) 厚编排、update.rs(508) 超行 | Phase 3 统一编排后抽薄 chat |
| `acp/` `a2a/` `automation/` | entry | 转发为主，但各复制一份编排脚手架 | **Phase 3a 收敛** |
| `agent/` | harness | 依赖方向正确；不再持有 Tool trait | — |
| `tools/` | capability | 仅 8 个文件仍反向依赖 agent（元工具 + 待下移工具函数） | 见"护栏例外清单" |
| `permissions/` `hooks/` `memory/` `skills/` | capability | 基本健康 | — |
| `llm/` | model | 只依赖 core，干净 | — |
| `core/` | infra | state 迁走后只剩纯基建；str_util.rs(801) 偏杂物堆 | 后续拆分 |
| `storage/` `observability/` `workspace/` `browser/` `codebase/` | infra | ✅ workspace→agent 反向边已修复（ProjectType → core/types） | — |

### 护栏例外清单（tests/layering.rs）

以下 8 个文件仍允许 capability→agent（已登记在护栏，需随清理缩减）：
- 真元工具：`tools/builtin/agent_tool.rs`（子代理）、`workflow_tool.rs`（workflow harness）
- 待下移工具函数：`bash.rs`(stream_chunk)、`plan_mode.rs`/`ask_user.rs`(sanitize)、`read_file.rs`/`visual_describe.rs`(image_transcribe)、`memory/procedure.rs`(sanitize)

## 关键约束（红线）

1. **`deepseek-native:` 标记（18 处）不动**——位置移动允许，语义与调用路径不变。
2. **主循环时序不动**——Phase 1/3 是"位置移动 + 结构收敛"，不是行为重写。
3. **不批量改名**（core→infra、commands→entry 等）：用本图标注归属层，除非明确要求 rename。
4. 每个顶层模块的 `mod.rs` 头注释应写"本模块属于 X 层，依赖 Y，被 Z 依赖"。

## 与文档的关系

- 能力清单（带代码证据）：`docs/CURRENT_STATE.md`
- 路线图与剩余主线：`docs/ROADMAP.md`
