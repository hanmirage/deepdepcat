# DeepDepCat 自家任务基准（bench）

> 目标文档第三条铁律：**自家 pipeline 通过率**。本目录是 DeepDepCat 的 20 任务基准（编码 10 + Depwork 10），每轮主线收尾用它验证「不倒退」，定期用它跑分。

## 任务集

- `tasks/code.md` — 10 个编码任务（基于 `work/` 沙箱项目）
- `tasks/depwork.md` — 10 个 Depwork 任务（文献/调研/文案/文档）

## 夹具布局（2026-08-09 起）

- `work-src/` = **全绿基线**（所有共享缺陷已修复，`npm test` 通过）；
- `bench/fixtures/<task-id>/` = 该任务的 delta 覆盖（只还原自己的缺陷，例如 `fix-format-bug/src/format.js`、`fix-test-flake/test/async.test.js`、`verify-only/test/baseline-fail.test.js`）；
- 每个任务：`work-src` 复制到 `work/` → 覆盖 `fixtures/<task-id>/` → 运行。起始态除目标任务外必须全绿。

## 怎么跑

### 编码任务（自动，走 ACP）

前置条件：
1. 应用已启动，配置好 DeepSeek key，设置 → 常规 → ACP 服务开启（默认端口 31524）；
2. Node 18+；
3. 工作区是 `bench/work`（git 仓库，脚本每个任务前 `git reset --hard` 恢复干净状态）。

```powershell
node bench/run_bench.mjs --base http://127.0.0.1:31524 --out bench/results/run-2026-08-08
```

脚本对每个 code 任务：恢复工作区 → ACP `session/new` → `prompt/stream` → SSE 收集 → 存 transcript（`<out>/<task-id>.json`）→ 关会话。完成后打印 `completed / failed` 计数。

每个任务在 `session/close` 前会调用 ACP `session/evidence` 归档 `<task-id>.evidence.json`（messages + agent_events，独立于 agent 自述），打分以此为准。

### Depwork 任务（自动，走 ACP — 2026-08-09 起）

ACP 已支持 `work_mode=depwork` + `permission_mode=bypass`，Depwork 基准可全自动跑：

```powershell
node bench/run_depwork.mjs --base http://127.0.0.1:31524 --out bench/results/depwork-run-YYYY-MM-DD
```

与 code 任务一致：`session/evidence` 归档 `<id>.evidence.json`（工具调用、文件改动、消息全文），打分不依赖 agent 自述。工作区为 `bench/depwork-work/`（已 gitignore）。前提：应用运行、ACP 开启、DeepSeek key 有余额、权限为 bypass（脚本内传参）。

## 评分（score.md）

每任务按 acceptance 检查表人工/评审打分：`pass` / `fail` / `blocked`。通过率 = pass / (pass + fail)。**失败归类必须记录**：未完成 / 超时 / 权限卡死 / 输出错误 / 提前宣布完成（laziness）/ 环境问题——这六类是后续主线的数据源。

## 冒烟清单（真实环境）

- [ ] kill -9 恢复：跑一个多轮任务 → 任务管理器杀掉进程 → 重启 → 该会话 rewind 点/用量/事件日志仍在（M3 验收）
- [x] research_search 真实 API（2026-08-08：Semantic Scholar + Crossref 均返回 2026 年论文、DOI/URL/摘要正常）
- [ ] workflow fan_out 真实并行（3+ 步，看活动面板并行执行与进度事件）
- [ ] 权限 grant 撤销即时生效（设置页撤销后下一次同类调用恢复弹窗）

## 真实跑分记录

### 2026-08-08 首次真实跑分（DeepSeek V4-Flash，ACP）

- 5 个编码任务全部 completed：`fix-format-bug` / `add-summary` / `fix-race` / `split-module` 被 M2 独立 evaluator 判 **PASS**（逐条验收标准 + 真实命令证据）；`verify-only` 如实报告基线失败且零改动（行为正确）。
- evaluator 实际抓到的质量点：生成器报告路径不一致、任务范围外改动的必要性论证、并发拒绝路径/未处理异常审查、31 个导出完整保留核验。
- 过程发现并解决的真实问题：
  1. DeepSeek Responses API 当前只接受 `deepseek-v4-flash`（pro 的 Codex 集成 8 月初才开放）→ 跑分默认模型用 flash；
  2. `permission_mode.json` 持久化的全局模式会覆盖 `config.toml` 的 `[permissions] mode` → 跑分需两处都设 `bypass`；
  3. ACP 参数为 snake_case（`session_id`），脚本已修正。
- 配置已恢复原状（`config.toml` / `permission_mode.json` 均回滚，ACP 关闭、权限回 default）。

### 2026-08-08 全量编码跑分（10/10 完成）

| 任务 | 判定 | 说明 |
|---|---|---|
| fix-format-bug | ✅ PASS | evaluator 逐条核验 + 真实验证 |
| add-summary | ✅ PASS | 4/4 验收点，含空数组边界 |
| fix-race | ✅ PASS | 并发拒绝路径/未处理异常审查 |
| split-module | ✅ PASS | 312 行拆分，31 导出完整保留 |
| verify-only | ✅ PASS | 如实报告基线失败、零改动（人工判定） |
| fix-test-flake | ✅ PASS | 连续 3 次全绿 + hash 对比 |
| refactor-callback | ❌ FAIL | **DSML 纯 XML 工具调用泄漏**：模型输出 `<tool_calls><invoke>` 无 `｜DSML｜` 分隔符，dsml.rs 不识别 → 工具未执行、标记泄漏为文本 |
| write-docs | ✅ PASS | 519 行 README、39 导出全覆盖、未改 src（人工判定；evaluator 未产出 verdict，待查） |
| add-validation | ✅ PASS | 6/6 测试绿 + LSP 干净 |
| error-messages | ✅ PASS | 三要素错误 + 成功路径锁定 |

**通过率 9/10**。唯一 FAIL 是真实发现的 harness 兼容性 bug（纯 XML tool_calls 变体）。

### 2026-08-08 DSML 纯 XML 变体修复 + 补跑

- 修复：`dsml.rs` 的 `normalized()` 增加纯 ASCII `<tool_calls><invoke><parameter>` 变体归一化（DeepSeek V4-Flash Responses 真实输出形态），现有解析/剥离正则全复用；新增 3 个测试（解析/自闭合/普通文本不误伤）。
- 补跑 `refactor-callback`：**工具调用正常执行**，`legacy.js` 成功改写为 Promise/async，工作区实测 `npm test` 3/3 全绿，行为语义保留 → **PASS（人工判定）**。
- 无 evaluator verdict 的原因：生成器跑了真实测试且通过（Tier2 证据）→ 按 M2 设计免独立评审，属正确行为。
- **编码任务集达成 10/10 PASS**。
