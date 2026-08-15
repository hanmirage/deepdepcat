---
name: zhihu-answer
description: 知乎回答模板 — 前置结论、论证分层、引用与数据充足、结尾可延伸。Use when the user asks to write a 知乎/知乎回答 or a structured long answer.
when-to-use: 知乎, 回答, zhihu, answer
work_mode: depwork
allowed-tools: research_search, research_save, research_list, research_folder_search, web_search, web_fetch, docx_generate, content_pack
---

# 知乎回答模板

## 结构

- **前置结论**：第一段直接给答案（3 句内），不铺垫；
- **论证分层**：按「是什么 → 为什么 → 怎么做」或「证据 → 解释 → 边界」组织；
- **引用与数据**：每个关键论断带来源；无法查证的标注「未证实」；
- **边界说明**：说明结论适用的前提与不适用的情况；
- **结尾**：一句总结 + 一个延伸问题（可选）。

## 要求

- 段落 ≤ 6 行，小标题按需使用；
- 不写「谢邀」式套话；语气专业但不端着；
- 长回答可导出 `docx_generate`。
