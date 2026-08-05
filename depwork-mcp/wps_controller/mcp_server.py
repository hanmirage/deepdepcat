#!/usr/bin/env python3
"""WPS Office MCP Server —— 让 AI 直接通过 MCP 协议控制 WPS Office。

复用 wps_controller 核心库，暴露 MCP 工具供 AI 调用。

架构：
    AI 客户端  ──MCP 协议──▶  本 Server  ──▶  wps_controller 核心库  ──▶  WPS COM

启动：
    python -m wps_controller.mcp_server

MCP 客户端配置（在 mcpServers 中声明本服务）：
    {
      "mcpServers": {
        "wps-office": {
          "command": "python",
          "args": ["-m", "wps_controller.mcp_server"]
        }
      }
    }
"""

import sys
import os
import json
import copy
from typing import Any, Optional, Dict, List

# 确保包可导入
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from mcp.server import MCPServer
from mcp.types import ToolAnnotations

from wps_controller.core import document as doc_mod
from wps_controller.core import writer as writer_mod
from wps_controller.core import calc as calc_mod
from wps_controller.core import impress as impress_mod
from wps_controller.core import export as export_mod
from wps_controller.core.session import Session

# ══════════════════════════════════════════════════════════════
# 会话管理 —— 内存中保持状态，AI 不需要传文件路径
# ══════════════════════════════════════════════════════════════

_session = Session()


def _get_project() -> Dict[str, Any]:
    """获取当前项目，如果没有则报错。"""
    if not _session.has_project():
        raise RuntimeError(
            "没有打开的文档。请先调用 create_document 或 open_document。"
        )
    return _session.get_project()


def _snapshot(desc: str = "") -> None:
    """保存快照（用于撤销）。"""
    _session.snapshot(desc)


def _ok(data: Any, message: str = "") -> str:
    """返回成功结果 JSON。"""
    result = {"success": True, "data": data}
    if message:
        result["message"] = message
    return json.dumps(result, ensure_ascii=False, default=str)


def _err(msg: str) -> str:
    """返回错误结果 JSON。"""
    return json.dumps({"success": False, "error": msg}, ensure_ascii=False, default=str)


# ══════════════════════════════════════════════════════════════
# 创建 MCP Server
# ══════════════════════════════════════════════════════════════

server = MCPServer(
    name="wps-office",
    title="WPS Office Controller",
    description="通过 MCP 协议控制 WPS Office，支持 Writer/Calc/Impress 文档的创建、编辑和导出，支持实时打字效果。",
    version="2.0.0",
    instructions=(
        "这是一个 WPS Office 控制器。工作流程：\n"
        "1. 先调用 create_document 创建文档（或 open_document 打开已有项目）\n"
        "2. 调用 writer_/calc_/impress_ 系列工具添加内容\n"
        "3. 调用 export_document 导出为 docx/xlsx/pptx/pdf 文件\n"
        "4. 导出时可选 live=true 开启实时打字效果（speed: fast/normal/slow）\n"
        "会话状态在内存中保持，不需要传文件路径。"
    ),
)


# ══════════════════════════════════════════════════════════════
# 文档管理工具
# ══════════════════════════════════════════════════════════════

@server.tool(
    description="创建一个新的 WPS 文档项目。doc_type: writer(文字)/calc(表格)/impress(演示)。name: 文档名称。",
    annotations=ToolAnnotations(title="创建文档"),
)
def create_document(
    doc_type: str = "writer",
    name: str = "untitled",
    profile: Optional[str] = None,
) -> str:
    """创建新文档。"""
    try:
        project = doc_mod.create_document(doc_type, name, profile)
        _session.set_project(project)
        return _ok(doc_mod.get_document_info(project), f"已创建 {doc_type} 文档: {name}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="打开已有的项目 JSON 文件。path: .json 项目文件路径。",
    annotations=ToolAnnotations(title="打开文档"),
)
def open_document(path: str) -> str:
    """打开已有项目。"""
    try:
        project = doc_mod.open_document(path)
        _session.set_project(project, path)
        return _ok(doc_mod.get_document_info(project), f"已打开: {path}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="保存当前文档项目到 JSON 文件。path: 保存路径（可选，默认原路径）。",
    annotations=ToolAnnotations(title="保存文档"),
)
def save_document(path: Optional[str] = None) -> str:
    """保存项目。"""
    try:
        project = _get_project()
        saved_path = _session.save_session(path)
        return _ok({"path": saved_path}, f"已保存到: {saved_path}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="获取当前文档的信息摘要（类型、内容数量、工作表/幻灯片数等）。",
    annotations=ToolAnnotations(title="文档信息"),
)
def get_document_info() -> str:
    """获取文档信息。"""
    try:
        project = _get_project()
        return _ok(doc_mod.get_document_info(project))
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="获取会话状态（是否有文档、修改状态、撤销/重做计数）。",
    annotations=ToolAnnotations(title="会话状态"),
)
def get_session_status() -> str:
    """获取会话状态。"""
    return _ok(_session.status())


@server.tool(
    description="撤销上一步操作。",
    annotations=ToolAnnotations(title="撤销"),
)
def undo() -> str:
    """撤销。"""
    try:
        desc = _session.undo()
        return _ok({"restored": desc}, f"已撤销: {desc}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="重做上一步撤销的操作。",
    annotations=ToolAnnotations(title="重做"),
)
def redo() -> str:
    """重做。"""
    try:
        desc = _session.redo()
        return _ok({"restored": desc}, f"已重做: {desc}")
    except Exception as e:
        return _err(str(e))


# ══════════════════════════════════════════════════════════════
# Writer 工具
# ══════════════════════════════════════════════════════════════

@server.tool(
    description="【Writer】添加标题。text: 标题文本，level: 1-6（1=H1, 2=H2...）。仅适用于 Writer 文档。",
    annotations=ToolAnnotations(title="添加标题"),
)
def writer_add_heading(
    text: str,
    level: int = 1,
) -> str:
    """添加标题。"""
    try:
        project = _get_project()
        _snapshot(f"add_heading: {text[:30]}")
        item = writer_mod.add_heading(project, text, level)
        return _ok(item, f"已添加 H{level}: {text[:50]}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Writer】添加段落。text: 段落文本。可选 style: JSON 格式的样式（如 {\"bold\": true, \"font_size\": 14}）。",
    annotations=ToolAnnotations(title="添加段落"),
)
def writer_add_paragraph(
    text: str,
    bold: bool = False,
    italic: bool = False,
    font_size: Optional[int] = None,
    align: Optional[str] = None,
) -> str:
    """添加段落。"""
    try:
        project = _get_project()
        _snapshot(f"add_paragraph: {text[:30]}")
        style = {}
        if bold:
            style["bold"] = True
        if italic:
            style["italic"] = True
        if font_size:
            style["font_size"] = font_size
        if align:
            style["align"] = align
        item = writer_mod.add_paragraph(project, text, style if style else None)
        return _ok(item, f"已添加段落: {text[:50]}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Writer】添加列表。items: 列表项数组（如 [\"项目1\", \"项目2\"]）。list_style: bullet(无序)/number(有序)。",
    annotations=ToolAnnotations(title="添加列表"),
)
def writer_add_list(
    items: List[str],
    list_style: str = "bullet",
) -> str:
    """添加列表。"""
    try:
        project = _get_project()
        _snapshot(f"add_list: {len(items)} items")
        item = writer_mod.add_list(project, items, list_style)
        return _ok(item, f"已添加{list_style}列表: {len(items)} 项")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Writer】添加表格。rows: 行数，cols: 列数。
data: 二维数组表格数据（如 [["姓名","年龄"],["张三",25]]）。如果不提供 data 则创建空表格。""",
    annotations=ToolAnnotations(title="添加表格"),
)
def writer_add_table(
    rows: int = 2,
    cols: int = 2,
    data: Optional[List[List[Any]]] = None,
) -> str:
    """添加表格。"""
    try:
        project = _get_project()
        _snapshot(f"add_table: {rows}x{cols}")
        item = writer_mod.add_table(project, rows, cols, data)
        return _ok(item, f"已添加 {rows}x{cols} 表格")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Writer】插入图片。image_path: 图片文件路径。
width/height: 图片尺寸（如 "10cm"、"200px"）。""",
    annotations=ToolAnnotations(title="插入图片"),
)
def writer_add_image(
    image_path: str,
    width: str = "10cm",
    height: str = "10cm",
) -> str:
    """添加图片。"""
    try:
        project = _get_project()
        _snapshot(f"add_image: {image_path}")
        item = writer_mod.add_image(project, image_path, width, height)
        return _ok(item, f"已添加图片: {os.path.basename(image_path)}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Writer】添加分页符。",
    annotations=ToolAnnotations(title="添加分页符"),
)
def writer_add_page_break() -> str:
    """添加分页符。"""
    try:
        project = _get_project()
        _snapshot("add_page_break")
        item = writer_mod.add_page_break(project)
        return _ok(item, "已添加分页符")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Writer】查找并替换文本。find_text: 要查找的文本，replace_text: 替换为的文本。
会搜索段落、标题、列表和表格中的所有文本。""",
    annotations=ToolAnnotations(title="查找替换"),
)
def writer_find_replace(
    find_text: str,
    replace_text: str,
) -> str:
    """查找替换。"""
    try:
        project = _get_project()
        _snapshot(f"find_replace: {find_text} → {replace_text}")
        result = writer_mod.find_replace(project, find_text, replace_text)
        return _ok(result, f"已替换 {result['replaced']} 处")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Writer】列出所有内容项（含预览）。",
    annotations=ToolAnnotations(title="列出内容"),
)
def writer_list_content() -> str:
    """列出内容。"""
    try:
        project = _get_project()
        items = writer_mod.list_content(project)
        return _ok(items, f"共 {len(items)} 项内容")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Writer】删除指定索引的内容项。index: 内容项索引（从0开始）。",
    annotations=ToolAnnotations(title="删除内容"),
)
def writer_remove_content(index: int) -> str:
    """删除内容项。"""
    try:
        project = _get_project()
        _snapshot(f"remove_content: {index}")
        removed = writer_mod.remove_content(project, index)
        return _ok(removed, f"已删除第 {index} 项")
    except Exception as e:
        return _err(str(e))


# ══════════════════════════════════════════════════════════════
# Calc 工具
# ══════════════════════════════════════════════════════════════

@server.tool(
    description="""【Calc】设置单元格的值。ref: 单元格引用（如 A1、B2）。
value: 单元格值。可选 formula: Excel 公式（如 =B2*C2）。
sheet: 工作表索引（从0开始，默认0）。仅适用于 Calc 文档。""",
    annotations=ToolAnnotations(title="设置单元格"),
)
def calc_set_cell(
    ref: str,
    value: Any,
    formula: Optional[str] = None,
    sheet: int = 0,
) -> str:
    """设置单元格。"""
    try:
        project = _get_project()
        _snapshot(f"set_cell: {ref}={value}")
        result = calc_mod.set_cell(project, ref, value, sheet=sheet, formula=formula)
        return _ok(result, f"已设置 {ref} = {value}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Calc】批量写入一个矩形区域。start_ref: 起始单元格（如 A1）。
data: 二维数组（如 [["张三",95],["李四",87]]），从 start_ref 开始向右向下填充。
sheet: 工作表索引。""",
    annotations=ToolAnnotations(title="批量写入"),
)
def calc_set_range(
    start_ref: str,
    data: List[List[Any]],
    sheet: int = 0,
) -> str:
    """批量写入。"""
    try:
        project = _get_project()
        _snapshot(f"set_range: {start_ref}, {len(data)} rows")
        result = calc_mod.set_range(project, start_ref, data, sheet=sheet)
        return _ok(result, f"已写入 {result['cells_set']} 个单元格")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Calc】合并单元格。start_ref: 起始单元格，end_ref: 结束单元格（如 A1 到 D1）。
sheet: 工作表索引。""",
    annotations=ToolAnnotations(title="合并单元格"),
)
def calc_merge_cells(
    start_ref: str,
    end_ref: str,
    sheet: int = 0,
) -> str:
    """合并单元格。"""
    try:
        project = _get_project()
        _snapshot(f"merge: {start_ref}:{end_ref}")
        result = calc_mod.merge_cells(project, start_ref, end_ref, sheet=sheet)
        return _ok(result, f"已合并 {start_ref}:{end_ref}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Calc】添加新工作表。name: 工作表名称。",
    annotations=ToolAnnotations(title="添加工作表"),
)
def calc_add_sheet(name: str = "Sheet") -> str:
    """添加工作表。"""
    try:
        project = _get_project()
        _snapshot(f"add_sheet: {name}")
        result = calc_mod.add_sheet(project, name)
        return _ok(result, f"已添加工作表: {name}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Calc】列出所有工作表。",
    annotations=ToolAnnotations(title="列出工作表"),
)
def calc_list_sheets() -> str:
    """列出工作表。"""
    try:
        project = _get_project()
        sheets = calc_mod.list_sheets(project)
        return _ok(sheets, f"共 {len(sheets)} 个工作表")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Calc】获取单元格的值。ref: 单元格引用。sheet: 工作表索引。",
    annotations=ToolAnnotations(title="获取单元格"),
)
def calc_get_cell(ref: str, sheet: int = 0) -> str:
    """获取单元格。"""
    try:
        project = _get_project()
        result = calc_mod.get_cell(project, ref, sheet=sheet)
        return _ok(result)
    except Exception as e:
        return _err(str(e))


# ══════════════════════════════════════════════════════════════
# Impress 工具
# ══════════════════════════════════════════════════════════════

@server.tool(
    description="""【Impress】添加幻灯片。title: 幻灯片标题，content: 幻灯片内容文本。
仅适用于 Impress 文档。""",
    annotations=ToolAnnotations(title="添加幻灯片"),
)
def impress_add_slide(
    title: str = "",
    content: str = "",
) -> str:
    """添加幻灯片。"""
    try:
        project = _get_project()
        _snapshot(f"add_slide: {title}")
        slide = impress_mod.add_slide(project, title, content)
        return _ok(slide, f"已添加幻灯片: {title}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Impress】在幻灯片中添加元素。slide_index: 幻灯片索引（从0开始）。
element_type: text_box/image/shape。
text: 文本内容（仅 text_box）。
x/y/width/height: 位置和尺寸（如 "2cm"、"10cm"）。""",
    annotations=ToolAnnotations(title="添加元素"),
)
def impress_add_element(
    slide_index: int,
    element_type: str = "text_box",
    text: str = "",
    x: str = "2cm",
    y: str = "2cm",
    width: str = "10cm",
    height: str = "5cm",
) -> str:
    """添加元素。"""
    try:
        project = _get_project()
        _snapshot(f"add_element: slide {slide_index}")
        elem = impress_mod.add_slide_element(
            project, slide_index, element_type, text, x, y, width, height
        )
        return _ok(elem, f"已添加 {element_type} 到幻灯片 {slide_index}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Impress】复制幻灯片。index: 要复制的幻灯片索引。",
    annotations=ToolAnnotations(title="复制幻灯片"),
)
def impress_duplicate_slide(index: int) -> str:
    """复制幻灯片。"""
    try:
        project = _get_project()
        _snapshot(f"duplicate_slide: {index}")
        slide = impress_mod.duplicate_slide(project, index)
        return _ok(slide, f"已复制幻灯片 {index}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Impress】移动幻灯片。from_index: 源位置，to_index: 目标位置。""",
    annotations=ToolAnnotations(title="移动幻灯片"),
)
def impress_move_slide(
    from_index: int,
    to_index: int,
) -> str:
    """移动幻灯片。"""
    try:
        project = _get_project()
        _snapshot(f"move_slide: {from_index}→{to_index}")
        result = impress_mod.move_slide(project, from_index, to_index)
        return _ok(result, f"已移动: {from_index} → {to_index}")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="【Impress】列出所有幻灯片。",
    annotations=ToolAnnotations(title="列出幻灯片"),
)
def impress_list_slides() -> str:
    """列出幻灯片。"""
    try:
        project = _get_project()
        slides = impress_mod.list_slides(project)
        return _ok(slides, f"共 {len(slides)} 张幻灯片")
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="""【Impress】更新幻灯片内容。index: 幻灯片索引。
title: 新标题（可选），content: 新内容（可选）。""",
    annotations=ToolAnnotations(title="更新幻灯片"),
)
def impress_set_slide_content(
    index: int,
    title: Optional[str] = None,
    content: Optional[str] = None,
) -> str:
    """更新幻灯片。"""
    try:
        project = _get_project()
        _snapshot(f"set_slide_content: {index}")
        slide = impress_mod.set_slide_content(project, index, title, content)
        return _ok(slide, f"已更新幻灯片 {index}")
    except Exception as e:
        return _err(str(e))


# ══════════════════════════════════════════════════════════════
# 导出工具
# ══════════════════════════════════════════════════════════════

@server.tool(
    description="""导出文档为真实文件（通过 WPS COM 自动化）。

output_path: 输出文件路径。
preset: 导出格式 —— docx/xlsx/pptx/pdf/txt/html/csv 等。
overwrite: 是否覆盖已有文件（默认 true）。
live: 实时打字模式 —— AI 逐字输入，肉眼可见（默认 false）。
speed: 打字速度 fast/normal/slow（默认 normal，仅 live=true 时有效）。
live_delay: 自定义逐字间隔秒数（覆盖 speed 预设，仅 live=true 时有效）。

导出后返回文件路径、大小、耗时等信息。""",
    annotations=ToolAnnotations(title="导出文档"),
)
def export_document(
    output_path: str,
    preset: str = "docx",
    overwrite: bool = True,
    live: bool = False,
    speed: str = "normal",
    live_delay: Optional[float] = None,
) -> str:
    """导出文档。"""
    try:
        project = _get_project()

        # live 模式默认 visible
        result = export_mod.export(
            project,
            output_path=output_path,
            preset=preset,
            overwrite=overwrite,
            visible=live,  # live 模式自动可见
            live=live,
            live_delay=live_delay if live_delay is not None else 0.03,
            speed=speed,
        )

        msg = f"已导出: {result['output']} ({result['file_size']:,} bytes)"
        if live:
            msg += f" [LIVE mode, speed={speed}, delay={result.get('char_delay', '?')}s]"

        return _ok(result, msg)
    except Exception as e:
        return _err(str(e))


@server.tool(
    description="列出所有可用的导出格式预设。",
    annotations=ToolAnnotations(title="导出格式列表"),
)
def list_export_presets() -> str:
    """列出导出预设。"""
    presets = export_mod.list_presets()
    preset_list = [
        {"name": k, **v} for k, v in presets.items()
    ]
    return _ok(preset_list, f"共 {len(preset_list)} 个预设")


@server.tool(
    description="列出可用的页面配置文件（A4、Letter、16:9 等）。",
    annotations=ToolAnnotations(title="页面配置列表"),
)
def list_page_profiles() -> str:
    """列出页面配置。"""
    return _ok(doc_mod.list_profiles())


# ══════════════════════════════════════════════════════════════
# 入口
# ══════════════════════════════════════════════════════════════

def main():
    """启动 MCP Server（stdio 模式）。"""
    server.run(transport="stdio")


if __name__ == "__main__":
    main()
