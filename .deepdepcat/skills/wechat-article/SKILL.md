---
name: wechat-article
description: 公众号文章写作模板 — 钩子开头、小标题分段、论证完整、结尾行动号召。Use when the user asks to write or polish a 公众号/微信文章 or a long-form article.
when-to-use: 公众号, 微信, 文章, 长文, wechat, article
work_mode: depwork
allowed-tools: research_search, research_save, research_list, research_folder_search, web_search, web_fetch, docx_generate, docx_edit, pdf_generate, content_pack
---

# 公众号文章模板

## 结构（1200 字量级）

- **钩子开头（前 3 行）**：具体冲突、反常识结论或数字，禁止「在当今社会」式套话；30 秒内给出「读完能得到什么」的承诺。
- **小标题分段（3–5 段）**：每段一个小标题 + 一个核心论点 + 一个例子/数据；段与段之间逻辑递进，不并列堆砌。
- **论证完整**：关键事实必须可溯源（来源/链接/访问日期）；无法查证的写「未证实」，不编造。
- **结尾行动号召**：一句可被截图引用的总结 + 一个具体行动建议（不要只写「欢迎点赞」）。

## 写作要求

- 每段不超过 6 行；段落短，多用句号断句。
- 数据、引用单独成行并标注来源；不用「据统计」这类无出处说法。
- 标题 8–20 字，包含关键词与利益点。
- 语气：直接、有观点，不写官话套话。

## 导出

- 长文 → `docx_generate`（标题、小标题、正文、引用列表）。
- 需要样张 → `pdf_generate`。
