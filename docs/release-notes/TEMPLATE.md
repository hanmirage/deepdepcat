# 发布说明模板（复制为 docs/release-notes/<版本>.md 后填写）

<!--
规则：
1. title / summary 必须以 `title:` / `summary:` 开头（半角冒号）；
2. 条目以 `- ` 开头，至少一条，写用户能感知的改动（不要写内部代号）；
3. summary 会进官网 manifest 和云端更新 notes，长度 ≤120 字符最佳；
4. 文件名必须与版本号一致：docs/release-notes/1.1.9.md
5. 写好后运行：
   pwsh .\scripts\release.ps1 -Version 1.1.9 -NotesFile docs\release-notes\1.1.9.md -DryRun
   确认七步无误后再正式跑（正式跑前先提交发布说明文件）。
-->

title: 一句话概括本版（会显示在官网更新日志顶部）
summary: v1.1.9 - 简短描述（manifest / 云端更新说明）
- 用户可感知的改动条目 1
- 用户可感知的改动条目 2
- 冒烟/回归结果：Rust xxxx / clippy 0 / vitest xxx
