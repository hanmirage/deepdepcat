# Depwork 任务集（14）

> 通过 ACP 自动跑（`bench/run_depwork.mjs`，`work_mode=depwork`）。结果摘要写 `bench/results/<run>/depwork-<id>.md`。

## Task 1: literature-review
- **任务**：用 `research_search` 检索「agent harness verification」相关文献（≥3 条），保存到资料夹（tag: agents），导出引用 Markdown。
- **Acceptance**：(a) 检索结果含标题/作者/年份/URL；(b) 资料夹 ≥3 条；(c) 导出文件含来源与访问日期；(d) 每条结论可溯源。

## Task 2: topic-research
- **任务**：调研「AI agent 商业化 2026」：web 检索 + 保存 ≥3 个来源，输出 500 字调研摘要（每段带来源标注）。
- **Acceptance**：(a) 来源 ≥3 且已保存；(b) 摘要每段可溯源；(c) 无未标注的事实断言。

## Task 3: wechat-article
- **任务**：基于资料夹内容写一篇公众号风格文章（1200 字左右）：钩子开头、小标题、结尾行动号召。
- **Acceptance**：(a) 符合 depwork-content skill 结构；(b) 关键事实带引用；(c) 导出为 Word（docx_generate）或 Markdown。

## Task 4: xiaohongshu-note
- **任务**：把同一调研写成小红书笔记：≤20 字标题、emoji 适量、段落 ≤3 行、结尾一个互动问题。
- **Acceptance**：(a) 标题 ≤20 字；(b) 每段 ≤3 行；(c) 有行动号召/互动问题。

## Task 5: research-export
- **任务**：整理资料夹（至少 5 条、2 个标签），导出带引用的 Markdown 到指定路径。
- **Acceptance**：(a) 导出文件存在且包含全部条目；(b) 含来源/URL/访问日期；(c) 标签正确分组（如有）。

## Task 6: fact-check
- **任务**：给一段 5 句的行业论断逐条做事实核查：能查证的给来源，查不到的标「未证实」。
- **Acceptance**：(a) 每条论断有 verdict（证实/部分/未证实）；(b) 证实的带来源 URL；(c) 不编造来源。

## Task 7: ppt-outline
- **任务**：基于资料夹内容生成一份 8 页 PPT 大纲（标题 + 每页要点）。
- **Acceptance**：(a) 8 页结构完整；(b) 每页要点可对应来源；(c) 大纲适合演示节奏。

## Task 8: data-table
- **任务**：把 10 行调研数据整理成 Excel 表格（标题/来源/年份/要点/标签），并加筛选说明。
- **Acceptance**：(a) xlsx 生成成功；(b) 表头与数据一致；(c) 打开无损坏。

## Task 9: word-report
- **任务**：把调研摘要生成 Word 报告（标题页 + 正文 + 引用列表）。
- **Acceptance**：(a) docx 生成成功；(b) 结构完整（标题/正文/引用）；(c) 引用列表与实际来源一致。

## Task 10: research-folder
- **任务**：只管理资料夹：给 3 条资料打标签、删除 1 条、列出剩余并核对。
- **Acceptance**：(a) 标签生效（research_list 可按标签过滤）；(b) 删除后列表正确；(c) 全程只读研究（不生成文档）。

## Task 11: content-pack
- **任务**：把给定源文分别改写为公众号/小红书/知乎三版，用 `content_pack` 导出文本包并自检合规，修正违规后重跑直至报告通过。
- **Acceptance**：(a) `content_pack` 返回含三平台 pass/fail 与违规明细的报告；(b) 小红书版标题 ≤20 字、每段 ≤3 行（校验通过）；公众号版含小标题且标题 8-20 字；(c) 文本包文件 + manifest.json 生成在工作区。

## Task 12: content-pipeline
- **任务**：选一个主题走完整内容产线：调研（资料夹引用）→ 成稿（公众号/小红书/知乎三版，同一事实基线）→ `content_pack` 分发验收，修正违规直到全部平台 fail 级规则通过，交付文本包。
- **Acceptance**：(a) 使用 `content_pack` 且三平台全部通过 fail 级规则；(b) 小红书版标题 ≤20 字、每段 ≤3 行，公众号版含小标题且标题 8-20 字；(c) 文本包 + manifest.json 在工作区；(d) 每平台关键事实可溯源（来源 URL 或「未证实」标注）。

## Task 13: citation-link
- **任务**：写一段带 `[#id]` 引用的调研文章（引用已保存到资料夹的来源），用 `citation_link` 校验引用并渲染参考列表，修正断链后交付。
- **Acceptance**：(a) `citation_link` 报告 0 断链；(b) 编号映射正确（`[#id]`→`[n]`）；(c) 参考列表文件生成且含全部被引用条目；(d) 替换后正文的引用编号与参考列表一致。

## Task 14: doc-consistency
- **任务**：写一份 ≥3 章的长文档（分章写），用 `doc_consistency` 做跨章一致性校验，按报告修正重复段落/缺章/编号跳号后交付。
- **Acceptance**：(a) `doc_consistency` 报告 0 跨章重复段落；(b) 章节编号连续（或全部无编号）；(c) 指定 required_sections 全部存在；(d) 报告标注「✓ 通过」。
