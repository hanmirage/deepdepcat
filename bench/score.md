# 评分标准

## 判定

| 结果 | 定义 |
|---|---|
| `pass` | acceptance 检查表全部满足，且无失败归类 |
| `fail` | 至少一条 acceptance 不满足，或命中任一失败归类 |
| `blocked` | 环境/依赖问题（API key 缺失、API 故障、ACP 未开），不算通过率分母 |

## 失败归类（记录到结果 JSON 的 `failure` 字段）

| 归类 | 含义 |
|---|---|
| `incomplete` | 回合结束但任务未完成（验收门漏检） |
| `timeout` | 超时/预算耗尽仍未完成 |
| `permission_stuck` | 权限路径卡死、反复弹窗、冷却异常 |
| `wrong_output` | 完成了但结果错误/不符合 acceptance |
| `laziness` | 提前宣布完成（M1 应根治） |
| `env` | 环境问题（工具缺失、网络、仓库状态） |

## 通过率

```
pass_rate = pass / (pass + fail)
blocked 不计入分母；depwork 手工任务与 code 自动任务分开统计。
```

## 跑分节奏

- 每条主线收尾：跑 code 10（约 20-40 分钟），对比上一次结果，**通过率不得下降**；
- 每 3 条主线或每周：跑全量 20（含 depwork 手工）；
- 结果归档在 `bench/results/`，命名 `run-<date>`，禁止覆盖历史。
