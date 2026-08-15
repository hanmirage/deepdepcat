//! Bundled skills — built-in skill definitions.

use crate::skills::types::{Skill, SkillSource};

/// Get all bundled skills.
pub fn get_bundled_skills() -> Vec<Skill> {
    vec![
        Skill {
            id: "code-review".to_string(),
            name: "Code Review".to_string(),
            description: "Review code for bugs, style issues, and improvements".to_string(),
            content: r#"You are a code review expert. Review the provided code for:
1. Bugs and potential issues
2. Code style and conventions
3. Performance concerns
4. Security vulnerabilities
5. Suggestions for improvement

Provide specific, actionable feedback with line references."#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "read_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
            ],
            permission_mode: Some("plan".to_string()),
            paths: vec![],
            work_modes: vec!["code".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "debug".to_string(),
            name: "Debug Mode".to_string(),
            description: "Systematic debugging workflow".to_string(),
            content: r#"You are a debugging expert. Follow this systematic approach:
1. Reproduce the issue
2. Identify the root cause
3. Propose a fix
4. Verify the fix resolves the issue
5. Check for similar issues elsewhere

Use logs, stack traces, and code inspection to diagnose problems."#
                .to_string(),
            model: None,
            allowed_tools: vec![],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["code".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "refactor".to_string(),
            name: "Refactor".to_string(),
            description: "Code refactoring with best practices".to_string(),
            content: r#"You are a refactoring expert. When refactoring:
1. Preserve behavior (test before and after)
2. Make small, incremental changes
3. Follow SOLID principles
4. Improve readability and maintainability
5. Remove dead code and duplication

Explain each refactoring step and its rationale."#
                .to_string(),
            model: None,
            allowed_tools: vec![],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["code".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "explain".to_string(),
            name: "Explain Code".to_string(),
            description: "Explain code in detail for learning".to_string(),
            content: r#"You are a code educator. Explain code clearly:
1. High-level purpose
2. Key components and their roles
3. Control flow and data flow
4. Design patterns used
5. Trade-offs and alternatives

Adapt your explanation to the user's expertise level."#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "read_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
                "list_dir".to_string(),
            ],
            permission_mode: Some("plan".to_string()),
            paths: vec![],
            work_modes: vec!["code".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "deep-research".to_string(),
            name: "深度调研".to_string(),
            description: "Deep research pipeline: question decomposition, evidence scoring, conflict resolution, graded conclusions".to_string(),
            content: r#"You are a deep research lead. For a serious research question, run this pipeline instead of ad-hoc searching:

1. DECOMPOSE — Split the question into 3-6 answerable sub-questions. Record them in the todo list. One sub-question at a time.
2. GATHER — For each sub-question: research_search / web_fetch_depwork / browser_control as needed. Clip every source you keep into the 资料夹 with research_clip (tags = sub-question id). If a source disagrees with another, keep BOTH and tag them conflict:yes.
3. SCORE — For each source, score evidence strength 1-5: primary data > peer-reviewed/authoritative > reputable secondary > blog/forum > unverifiable. Never invent a score; when uncertain, use the lower score.
4. RESOLVE — When sources conflict: prefer higher-scoring evidence; if scores are close, note the disagreement explicitly instead of picking silently. Do not average contradictory claims.
5. CONCLUDE — Answer each sub-question with a graded confidence (高/中/低) and cite the exact source ids (#id) from the 资料夹. Distinguish facts from inferences.
6. DELIVER — Generate the report with research_report (docx), then use docx_edit to add the narrative sections (结论/证据/冲突) if the user wants a finished document.

Rules:
- Every factual claim must trace to a clipped source (#id). No source = mark as 待证实, never fabricate.
- Use research_folder_search before re-searching a topic you already clipped.
- If the user's question is simple (answerable in one pass), skip straight to gather; do not ritualize the pipeline."#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "research_search".to_string(),
                "research_clip".to_string(),
                "research_folder_search".to_string(),
                "research_list".to_string(),
                "research_export".to_string(),
                "research_report".to_string(),
                "web_fetch_depwork".to_string(),
                "web_open".to_string(),
                "browser_control".to_string(),
                "docx_edit".to_string(),
                "todo_write".to_string(),
            ],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["depwork".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "content-pipeline".to_string(),
            name: "内容产线".to_string(),
            description: "内容产线：调研 → 成稿 → 多平台分发 一站式管线，交付公众号/小红书/知乎三版文本包。Use when the user wants to research then publish across platforms, 调研后发文, 多平台分发, 内容产线, 写一篇发公众号小红书知乎".to_string(),
            content: r#"You are a content production lead. When the user wants to research a topic and publish it across platforms (公众号/小红书/知乎), run this pipeline end to end instead of doing it ad-hoc:

1. CLARIFY — If the requirement is genuinely ambiguous, ask ONE focused question (topic / audience / target platforms / length). Then update_goal with the deliverable: 《主题》公众号/小红书/知乎 多平台文本包. Lay out the pipeline as todos with todo_write (one parent per stage below) so the user can follow progress.

2. RESEARCH — research_search / research_clip sources into the 资料夹 (tags = topic); for industry/trend questions use research_search source=web, for academic topics keep the default sources. research_folder_search before re-searching a topic you already clipped. Keep every source you use and cite it inline with [#id] markers (the research_list id, e.g. [#12]). Aim for ≥2 independent sources; mark what you cannot verify as 未证实, never fabricate.

3. DRAFT — Write ONE base article with solid structure and traceable facts. Then adapt it per target platform (公众号 / 小红书 / 知乎), one version each, following the platform template skill when available (wechat-article / xiaohongshu-note / zhihu-answer). Keep ONE fact baseline across all versions — only the expression differs, never the data.

4. VERIFY + PACK — Run citation_link on the base article to resolve the [#id] markers against the 资料夹, render the numbered reference list and catch broken citations — fix every broken citation before packing. Then call content_pack with all platform versions (items: [{platform, title, content}], output_dir in the workspace). Read the compliance report. Fix every fail-level violation (title length, subheadings, paragraph lines, emoji, closing call), then re-run content_pack until every platform passes its fail-level rules. Do not hand over a package with a fail-level violation.

5. DELIVER — Report the package path + manifest.json + per-platform pass status. For the 公众号 long form, also offer a finished docx via docx_generate.

Rules:
- Every factual claim must trace to a clipped source ([#id]); no source = mark as 未证实, never fabricate.
- Same fact baseline across platforms; never alter numbers or data for adaptation.
- citation_link must report zero broken citations, and content_pack must pass every platform's fail-level rules, before delivery.
- If the user wants only one platform, skip the others; if the request is simple (no research needed), skip stage 2; do not ritualize the pipeline."#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "research_search".to_string(),
                "research_clip".to_string(),
                "research_folder_search".to_string(),
                "research_list".to_string(),
                "citation_link".to_string(),
                "content_pack".to_string(),
                "docx_generate".to_string(),
                "docx_edit".to_string(),
                "todo_write".to_string(),
                "update_goal".to_string(),
            ],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["depwork".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "video-script".to_string(),
            name: "视频脚本与分镜".to_string(),
            description: "自媒体视频脚本工单：钩子→结构→分镜→字幕→交付".to_string(),
            content: r#"You are a video script writer. Produce a structured script:

1. 定档：平台（抖音/小红书/B站/视频号）、时长（秒数 → 字数 ≈ 每秒 4 字）、目标（涨粉/带货/科普/口播）。
2. 钩子（前 3 秒）：一句反常识/痛点/悬念，写 2 个备选。
3. 主体结构：钩子 → 痛点/场景 → 方案/信息 → 证据（数字/案例）→ 收尾 CTA。每段标时长。
4. 分镜表：镜头序号 / 景别 / 画面 / 字幕 / 旁白，表格输出。
5. 字幕稿：逐句口语化，长句拆短；标注重音词。
6. 交付：脚本正文用 docx_generate 生成文件；PPT 版用 ppt_generate（每页=一个镜头段落）。

Rules: 不编造数据；引用数字必须可溯源；时长与字数严格对应。"#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "docx_generate".to_string(),
                "ppt_generate".to_string(),
                "todo_write".to_string(),
            ],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["depwork".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "content-adapt".to_string(),
            name: "多平台文案适配".to_string(),
            description: "把一份内容改写成多平台版本：标题/正文/话题标签/字数档位".to_string(),
            content: r#"You are a cross-platform content adapter. Given one piece of content, produce per-platform variants:

1. 先确认目标平台列表（默认：公众号 / 小红书 / 抖音 / 知乎 / B站）。
2. 每个平台输出：标题（2 个备选）+ 正文（平台字数档位内）+ 话题标签 + 发布建议（时间/配图方向）。
   - 公众号：800-2000 字，小标题分层，可带表格；
   - 小红书：600-1000 字，emoji 适度，5-10 个话题标签，首图建议；
   - 抖音：口播稿 60-200 字 + 画面提示；
   - 知乎：回答体，先结论后论证，可引用资料夹来源；
   - B站：简介 100-300 字 + 3-6 个分区标签。
3. 统一事实与数字：所有平台共用同一套数据，禁止为适配改数据。
4. 交付：每平台一节 Markdown，用 docx_generate 生成文件（或对话输出）。

Rules: 同一事实基线，只有表达方式差异；引用来源标注 #id；不确定的信息标「待证实」。"#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "docx_generate".to_string(),
                "research_folder_search".to_string(),
                "research_list".to_string(),
                "todo_write".to_string(),
                "content_pack".to_string(),
            ],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["depwork".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "literature-review".to_string(),
            name: "文献综述管线".to_string(),
            description: "科研综述：检索→筛选→逐篇笔记→大纲→综述成稿".to_string(),
            content: r#"You are a literature review lead. Pipeline:

1. 检索：research_open_access（arXiv）+ research_search（Semantic Scholar/Crossref）按主题多关键词检索；每篇候选剪藏进资料夹（research_clip / research_save，标签=主题）。
2. 筛选：按相关性与年份排序，剔除不相关；给每篇打 高/中/低 相关度（research_clip 标签里写 relevance:high 等）。
3. 逐篇笔记：doc_read / pdf_tools 读 PDF，每篇写 2-4 行：问题/方法/结论/与综述主题的关系；用 docx_generate 或对话维护笔记。
4. 大纲：按 主题聚类（不是逐篇流水账）：背景 → 方法流派 → 共识 → 争议 → 空白 → 结论。
5. 成稿：research_report 生成来源附录，docx_edit 补综述正文；引用统一用 #id 并在文末 research_export format=gb7714 或 bibtex 出参考文献。

Rules: 观点必须挂来源；争议不能只写一方；「空白」只能来自已读文献的明确缺失，不能臆测。"#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "research_open_access".to_string(),
                "research_search".to_string(),
                "research_clip".to_string(),
                "research_save".to_string(),
                "research_folder_search".to_string(),
                "research_list".to_string(),
                "research_export".to_string(),
                "research_report".to_string(),
                "doc_read".to_string(),
                "pdf_tools".to_string(),
                "docx_generate".to_string(),
                "docx_edit".to_string(),
                "todo_write".to_string(),
            ],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["depwork".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
        Skill {
            id: "paper-table".to_string(),
            name: "论文表格提取".to_string(),
            description: "从论文/PDF 提取表格与数据并结构化到 xlsx".to_string(),
            content: r#"You are a data extraction specialist for academic PDFs.

1. 读论文：doc_read / pdf_tools 打开 PDF，定位表格所在页。
2. 提取：把表格按 行列 转录成 CSV 结构；列名照抄原文，数值原样保留（不要四舍五入），单位单独成列。
3. 标注：表头注明来源（论文标题 + 页码 + 表格编号）；无法识别的单元格标 "?"，绝不猜测。
4. 结构化：table_process 清洗（去重、类型判断），xlsx_generate 输出工作簿；公式类汇总（如均值）用 "=AVERAGE(...)" 让 Excel 计算。
5. 交付：返回文件路径 + 表格概要（行/列/来源）。

Rules: 数值必须逐字转录；OCR/文本提取的歧义标注而不是脑补；来源信息必须落在交付文件里。"#
                .to_string(),
            model: None,
            allowed_tools: vec![
                "doc_read".to_string(),
                "pdf_tools".to_string(),
                "table_process".to_string(),
                "xlsx_generate".to_string(),
                "ocr_image".to_string(),
                "todo_write".to_string(),
            ],
            permission_mode: Some("default".to_string()),
            paths: vec![],
            work_modes: vec!["depwork".to_string()],
            when_to_use: vec![],
            source: SkillSource::Bundled,
            file_path: None,
            enabled: true,
        },
    ]
}
