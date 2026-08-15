---
name: depwork-content
description: Depwork 自媒体内容创作模板 — 标题、结构、平台适配、配图与导出。Use when the user asks to write, draft, or polish 文案, 文章, 笔记, 脚本, 帖子, 标题, or content for 公众号/小红书/抖音/知乎.
when-to-use: 文案, 文章, 笔记, 脚本, 帖子, 标题, 公众号, 小红书, 抖音, 知乎, content
work_mode: depwork
allowed-tools: research_search, research_save, research_list, web_search, web_fetch, docx_generate, pdf_generate, content_pack
---

# 自媒体内容创作模板

## 开头定生死（前 3 行）

- 钩子：具体冲突/反常识/数字，不用「在当今社会」；
- 承诺：读者 30 秒内知道「读完能得到什么」；
- 示例：「我花 3 天跑了 20 个资料源，发现 90% 的选题都死在第一步。」

## 结构（推荐任一）

- 清单式：3 个要点 + 每个配一个例子（适合小红书/知乎）；
- 问题解决式：痛点 → 原因 → 方法 → 验证（适合公众号）；
- 故事式：冲突 → 转折 → 结论（适合视频脚本）；
- 对比式：A vs B，逐维度（适合测评/调研类）。

## 平台差异

- 公众号：完整论证 + 段落短 + 小标题；
- 小红书：标题 ≤ 20 字、emoji 适量、段落 ≤ 3 行、结尾行动号召；
- 抖音脚本：前 3 秒钩子 + 每 15 秒一个信息点；
- 知乎：前置结论，引用与数据充足。

## 收尾

- 一句总结（可被截图引用）；
- 一个行动建议；
- 互动问题（只问一个）。

## 导出

- 短内容 → 纯文本/Markdown；
- 长文/图文 → `docx_generate`；
- 需排版样张 → `pdf_generate`。
