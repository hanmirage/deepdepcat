# 编码任务集（10）

> 工作区：`bench/work`（预置沙箱，每个任务前 `git reset --hard`）。评分按 acceptance 检查表逐条核验；transcript 存 `bench/results/<run>/<id>.json`。

## Task 1: fix-format-bug
- **任务**：修复 `work/src/format.js` 中 `formatDate` 在月份 < 10 时不补零的 bug，并补一个测试。
- **Acceptance**：(a) `formatDate(new Date(2026, 0, 5))` 返回 `2026-01-05`；(b) 新增测试覆盖该用例；(c) `npm test` 全绿。

## Task 2: add-summary
- **任务**：给 `work/src/stats.js` 加 `summarize(numbers)` 函数：返回 `{count, sum, avg}`，空数组时 `avg` 为 0。
- **Acceptance**：(a) 函数存在且导出；(b) 空数组不抛错；(c) 有测试；(d) `npm test` 全绿。

## Task 3: fix-test-flake
- **任务**：`work/test/async.test.js` 有一个偶发失败的测试（定时器竞态）。修到稳定通过，不改断言语义。
- **Acceptance**：(a) `npm test` 连续跑 3 次全绿；(b) 改动最小（只动相关测试文件）。

## Task 4: refactor-callback
- **任务**：把 `work/src/legacy.js` 的回调风格改成 Promise/async，保持行为不变。
- **Acceptance**：(a) 无回调嵌套；(b) 原测试（改写后）全绿；(c) 无行为变化（对照 git diff 检查）。

## Task 5: write-docs
- **任务**：给 `work/src/*.js` 的公开函数写 README 风格文档（`work/README.md`），说明用法与边界。
- **Acceptance**：(a) README 覆盖全部公开函数；(b) 每个有签名 + 示例 + 边界说明；(c) 不修改 src 代码。

## Task 6: add-validation
- **任务**：`work/src/parse.js` 的 `parseConfig` 对非法 JSON 返回错误而不是抛异常；调用方已按 `{ok, error}` 处理。
- **Acceptance**：(a) 非法输入返回 `{ok: false, error}`；(b) 合法输入行为不变；(c) 有测试；(d) `npm test` 全绿。

## Task 7: fix-race
- **任务**：`work/src/cache.js` 有并发重复请求问题（两个相同 key 的并发 fetch 会重复调用）。加去重。
- **Acceptance**：(a) 并发相同 key 只调用一次底层 fetch；(b) 有并发测试；(c) `npm test` 全绿。

## Task 8: split-module
- **任务**：`work/src/big.js`（>200 行）按职责拆成 2-3 个文件，导出保持兼容。
- **Acceptance**：(a) 每个新文件 ≤80 行；(b) 导出 API 不变；(c) `npm test` 全绿。

## Task 9: error-messages
- **任务**：`work/src/cli.js` 的错误提示太笼统。让每个失败路径给出「做了什么 + 为什么失败 + 下一步」。
- **Acceptance**：(a) 每条错误含三个要素；(b) 不改变成功路径输出；(c) 有对应测试。

## Task 10: verify-only
- **任务**：只做验证：运行 `npm test` 和 `npx tsc --noEmit`（如有），报告结果，**不修改任何文件**。
- **Acceptance**：(a) 不产生文件改动（git status 干净）；(b) 报告含每个验证命令的真实输出与结论。
