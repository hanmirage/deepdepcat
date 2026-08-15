# WPS Office MCP Server

通过 MCP 协议让 AI 直接操控 WPS Office（Writer / Calc / Impress）：文档创建、编辑、导出、实时打字。

采用 **JSON 项目模型 + 延迟渲染** 架构：所有编辑操作只修改内存中的 JSON 字典（纯 Python，不需要 WPS 运行），只有导出时才真正调用 WPS COM 生成文件。

```
AI 客户端 ──MCP 协议──▶ 本 Server ──▶ wps_controller 核心库 ──▶ WPS COM
```

## 功能

- **Writer / Calc / Impress 三端**：创建、编辑、查找替换、表格、样式、合并单元格、幻灯片等 31 个工具
- **实时打字（live）**：导出时在可见 WPS 窗口中逐字输入（打字机效果），三档速度 + 自定义间隔
- **导出**：docx / xlsx / pptx / pdf / txt / html / csv / rtf 等 13 种格式预设
- **撤销 / 重做 / 持久化**：会话内 50 步撤销栈，项目可存 JSON 文件跨重启恢复
- **双入口**：MCP server（AI 调用）+ 命令行 CLI（脚本调用，`--json` 结构化输出）

## 系统要求

- Windows 10/11
- WPS Office 2019+（COM 自动化基于 KWPS / KET / KWPP.Application）
- Python 3.10+

## 安装

```bash
pip install -r requirements.txt
# 或
pip install .
```

## 接入 MCP 客户端

在任意 MCP 客户端的 mcpServers 配置中声明：

```json
{
  "mcpServers": {
    "wps-office": {
      "command": "python",
      "args": ["-m", "wps_controller.mcp_server"]
    }
  }
}
```

启动后 AI 即可调用 `create_document` / `writer_add_*` / `calc_set_*` / `impress_add_*` / `export_document` 等工具。

## CLI 使用

```bash
# 创建文档（--json 输出结构化结果）
python -m wps_controller.wps_cli --json document new --type writer --name 报告 -o report.json

# 添加内容
python -m wps_controller.wps_cli --json --project report.json writer add-heading -t 前言 -l 1
python -m wps_controller.wps_cli --json --project report.json writer add-paragraph -t 正文 --bold

# 导出（调用 WPS COM 生成真实文件）
python -m wps_controller.wps_cli --json --project report.json export render output.docx -p docx --overwrite

# 实时打字导出
python -m wps_controller.wps_cli --json --project report.json export render output.docx -p docx --live --speed slow
```

### 命令速查

```
wps_cli
├── document new|open|save|info|profiles|json
├── writer  add-paragraph|add-heading|add-list|add-table|add-image|add-page-break|remove|list|set-text|find-replace
├── calc    add-sheet|remove-sheet|rename-sheet|set-cell|get-cell|set-range|merge-cells|list-sheets
├── impress add-slide|remove-slide|set-content|list-slides|add-element|move-slide|duplicate-slide
├── style   create|modify|list|apply|remove
├── export  presets|preset-info|render
└── session status|undo|redo|history
```

## 架构

```
CLI / MCP Server
    │
    ▼
Session 层 —— 撤销/重做/持久化（纯 Python，不需要 WPS）
    │
    ▼
Core 模块 —— writer.py / calc.py / impress.py / styles.py（操作 JSON 项目字典）
    │
    ▼
export.py —— 只在导出时调用 WPS COM
    │
    ▼
wps_backend.py —— COM 接口封装（KWPS / KET / KWPP.Application）
    │
    ▼
WPS Office
```

## JSON 项目格式

```json
{
  "version": "1.0",
  "name": "报告",
  "type": "writer",
  "settings": {},
  "styles": {},
  "content": [
    { "type": "heading", "level": 1, "text": "前言", "style": {} },
    { "type": "paragraph", "text": "正文", "style": { "bold": true } },
    { "type": "table", "rows": 3, "cols": 3, "data": [["A1","B1","C1"]] }
  ]
}
```

Calc 项目用 `sheets` 数组（name + cells + merged_cells），Impress 项目用 `slides` 数组（title + content + elements）。

## 导出预设

| 预设 | 格式 | 类型 |
|------|------|------|
| docx / doc / pdf / txt / html / rtf | 对应扩展名 | writer |
| xlsx / xls / csv / pdf-calc | 对应扩展名 | calc |
| pptx / ppt / pdf-impress | 对应扩展名 | impress |

## 测试

```bash
python e2e_test.py    # CLI 端到端（44 用例）
python test_mcp.py    # MCP 工具层（45 用例，含 live 模式）
python full_test.py   # 全量回归（35 用例，live 三档速度）
```

测试会生成 docx/xlsx/pptx/pdf 产物，需要本机安装 WPS Office。

## License

MIT
