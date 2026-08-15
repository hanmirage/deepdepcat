---
name: long-document
description: 长文档分段写作模板 — 先大纲、再分章写、最后汇总校验，避免单次生成爆上下文或结构失控。Use when the user asks to write a long document/report/book with many sections (≥3 chapters or ≥1500 字).
when-to-use: 长文档, 报告, 白皮书, 章节, 分段, 长文, long document, report, chapter
work_mode: depwork
allowed-tools: todo_write, research_search, research_save, research_list, research_folder_search, web_search, web_fetch, docx_generate, docx_edit, docx_search, pdf_generate, doc_consistency, citation_link
---

# 长文档分段写作模板

## 流程（三段式，缺一不可）

1. **先大纲**：用 `todo_write` 建立章节清单（标题 + 每章要点 + 预期来源）；先给用户/自己确认结构，再动笔。
2. **分章写**：一次只写一章（≤800 字/章），每章结束用 `docx_edit` 或累计 `docx_generate` 追加；不要在一条回复里写完整个文档。
3. **汇总校验**：全部章节完成后，用 `doc_consistency` 跑跨章一致性校验（跨章重复段落/结构完整性/章节编号连续性），按报告修问题；引用用 `citation_link` 校验（0 断链）；全部通过后再导出成品（docx/pdf）。

## 写作规则

- 每章开头一句「本章目的」，结尾一句「小结」；
- 数据与引用标注来源（资料夹条目/URL/访问日期）；无法查证的写「未证实」；
- 章节间逻辑递进，禁止复制粘贴前文；
- 标题层级统一（一级/二级/三级），编号不跳号。

## 导出

- 长文 → `docx_generate`（可用 `template=` 复用样式）；
- 样张/排版预览 → `pdf_generate`；
- 内容修正 → `docx_edit`（保留修订模式可用）。
