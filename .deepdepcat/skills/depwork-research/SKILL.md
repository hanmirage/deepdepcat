---
name: depwork-research
description: Depwork 调研工作流 — 选题调研、文献检索、资料夹管理、带引用成稿。Use when the user asks for research, literature review, 调研, 查资料, 文献, or a fact-checked draft.
when-to-use: 调研, 查资料, 文献, research, literature, 选题调研, 资料整理
work_mode: depwork
allowed-tools: research_search, research_save, research_list, research_remove, research_export, web_search, web_fetch
---

# Depwork 调研工作流

用于自媒体选题、行业调研、文献综述与事实核查。核心纪律：**每条结论必须有来源，来源必须进资料夹，成稿必须带引用。**

## 流程

1. **明确需求**：选题/问题、产出格式（短文/报告/文档）、引用风格（链接式即可，或 DOI）。
2. **检索**：
   - 学术问题 → `research_search`（Semantic Scholar + Crossref）；
   - 行业/时效问题 → `web_search` + `web_fetch` 读原文。
3. **筛选与保存**：只保留可核验的一手/权威来源；每确认一条就用 `research_save` 存入资料夹（带 title/url/source/snippet/tags，快照可选）。
4. **整理**：`research_list` 回顾资料夹，按主题分组，找出支撑点与反例。
5. **成稿**：每个关键结论旁标注来源；不要写没有来源支撑的断言。
6. **导出**：`research_export` 生成带访问日期的引用列表，随稿交付。

## 规则

- 至少 2 个独立来源才能支撑一个事实性断言；
- 无法核验的信息明确标注「未证实」，不要编造来源；
- 保存时 snippet 写「这条资料支撑什么」，而不是复制标题；
- 成稿后做一次对抗检查：逐条结论问「来源是谁？权威吗？时间合适吗？」
