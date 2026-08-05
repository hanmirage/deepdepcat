"""WPS CLI - 导出/渲染管道。

将项目 JSON 转换为真实文档文件，通过 WPS COM 自动化完成。
这是唯一真正调用 WPS COM 的地方。
"""

import os
import time
import random
from typing import Dict, Any, Optional, List

from wps_controller.utils.wps_backend import (
    find_wps, create_document, save_as, export_pdf,
    close_document, quit_app, get_version,
)

# COM 常量
wdCollapseEnd = 0
wdAlignParagraphCenter = 1

# ── 速度预设 ──────────────────────────────────────────────────

SPEED_PRESETS = {
    "fast": {
        "char_delay": 0.01,    # 逐字间隔
        "item_pause": 0.05,    # 段落间停顿
        "heading_pause": 0.1,  # 标题后停顿
        "list_pause": 0.05,    # 列表项间停顿
        "cell_pause": 0.02,    # 表格单元格间停顿
        "row_pause": 0.05,     # 表格行间停顿
        "slide_pause": 0.2,    # 幻灯片间停顿
        "image_pause": 0.3,    # 图片插入后停顿
        "jitter": 0.005,       # 随机抖动
    },
    "normal": {
        "char_delay": 0.03,
        "item_pause": 0.2,
        "heading_pause": 0.3,
        "list_pause": 0.15,
        "cell_pause": 0.05,
        "row_pause": 0.1,
        "slide_pause": 0.5,
        "image_pause": 0.5,
        "jitter": 0.02,
    },
    "slow": {
        "char_delay": 0.08,
        "item_pause": 0.5,
        "heading_pause": 0.7,
        "list_pause": 0.35,
        "cell_pause": 0.1,
        "row_pause": 0.2,
        "slide_pause": 1.0,
        "image_pause": 1.0,
        "jitter": 0.04,
    },
}


def get_speed_config(speed: str = "normal", custom_delay: Optional[float] = None) -> Dict[str, float]:
    """获取速度配置。

    Args:
        speed: 预设名称 fast/normal/slow
        custom_delay: 自定义逐字间隔（覆盖预设）

    Returns:
        速度配置字典
    """
    config = dict(SPEED_PRESETS.get(speed, SPEED_PRESETS["normal"]))
    if custom_delay is not None and custom_delay > 0:
        config["char_delay"] = custom_delay
    return config


# ── 导出预设 ──────────────────────────────────────────────────

EXPORT_PRESETS = {
    # Writer
    "docx": {"name": "Word 文档", "doc_type": "writer", "format": "docx", "extension": ".docx"},
    "doc":  {"name": "Word 97-2003", "doc_type": "writer", "format": "doc", "extension": ".doc"},
    "pdf":  {"name": "PDF", "doc_type": "writer", "format": "pdf", "extension": ".pdf"},
    "txt":  {"name": "纯文本", "doc_type": "writer", "format": "txt", "extension": ".txt"},
    "html": {"name": "网页", "doc_type": "writer", "format": "html", "extension": ".html"},
    "rtf":  {"name": "RTF", "doc_type": "writer", "format": "rtf", "extension": ".rtf"},
    # Calc
    "xlsx": {"name": "Excel 工作簿", "doc_type": "calc", "format": "xlsx", "extension": ".xlsx"},
    "xls":  {"name": "Excel 97-2003", "doc_type": "calc", "format": "xls", "extension": ".xls"},
    "csv":  {"name": "CSV", "doc_type": "calc", "format": "csv", "extension": ".csv"},
    "pdf-calc": {"name": "PDF（从 Calc）", "doc_type": "calc", "format": "pdf", "extension": ".pdf"},
    # Impress
    "pptx": {"name": "PowerPoint 演示", "doc_type": "impress", "format": "pptx", "extension": ".pptx"},
    "ppt":  {"name": "PowerPoint 97-2003", "doc_type": "impress", "format": "ppt", "extension": ".ppt"},
    "pdf-impress": {"name": "PDF（从 Impress）", "doc_type": "impress", "format": "pdf", "extension": ".pdf"},
}


def list_presets() -> Dict[str, Any]:
    """列出所有导出预设。"""
    return dict(EXPORT_PRESETS)


def get_preset_info(name: str) -> Optional[Dict[str, Any]]:
    """获取指定预设的详细信息。"""
    preset = EXPORT_PRESETS.get(name)
    if not preset:
        available = ", ".join(EXPORT_PRESETS.keys())
        raise ValueError(f"不支持的导出预设: {name}。可用: {available}")
    return dict(preset)


# ── 导出主函数 ────────────────────────────────────────────────

def export(
    project: Dict[str, Any],
    output_path: str,
    preset: str = "docx",
    overwrite: bool = False,
    visible: bool = False,
    live: bool = False,
    live_delay: float = 0.03,
    speed: str = "normal",
) -> Dict[str, Any]:
    """导出文档到指定格式（使用 WPS COM 自动化）。

    Args:
        project: 项目字典
        output_path: 输出文件路径
        preset: 导出预设名称
        overwrite: 是否覆盖已有文件
        visible: WPS 窗口是否可见
        live: 实时打字模式（逐字写入，肉眼可见）
        live_delay: 打字间隔（秒），覆盖 speed 预设的 char_delay
        speed: 速度预设 fast/normal/slow

    Returns:
        包含输出路径、格式、文件大小等信息的字典
    """
    preset_info = get_preset_info(preset)
    output_path = os.path.abspath(output_path)

    if os.path.exists(output_path) and not overwrite:
        raise FileExistsError(f"输出文件已存在: {output_path}。使用 --overwrite 覆盖。")

    doc_type = preset_info["doc_type"]
    fmt = preset_info["format"]
    extension = preset_info["extension"]

    # 确保输出文件有正确的扩展名
    if not output_path.lower().endswith(extension):
        output_path = output_path + extension

    os.makedirs(os.path.dirname(output_path) or ".", exist_ok=True)

    # live 模式自动开启 visible
    if live:
        visible = True

    # 获取速度配置
    speed_config = get_speed_config(speed, live_delay if live_delay != 0.03 else None)

    # 通过 COM 创建文档并填充内容
    app = None
    doc = None
    try:
        app = find_wps(doc_type, visible=visible)
        doc = create_document(app, doc_type)

        if live:
            _fill_document_live(app, doc, project, doc_type, speed_config)
        else:
            _fill_document(doc, project, doc_type)

        if fmt == "pdf":
            export_pdf(doc, output_path, doc_type)
        else:
            save_as(doc, output_path, doc_type, fmt)

        file_size = os.path.getsize(output_path)

        result = {
            "output": output_path,
            "format": fmt,
            "extension": extension,
            "preset": preset,
            "file_size": file_size,
            "method": "wps-com-live-typing" if live else "wps-com-automation",
            "wps_version": get_version(app),
            "speed": speed if live else None,
            "char_delay": speed_config["char_delay"] if live else None,
        }

        close_document(doc, save=False)
        quit_app(app, force=True)
        return result

    except Exception:
        if doc:
            try:
                close_document(doc, save=False)
            except Exception:
                pass
        if app:
            try:
                quit_app(app, force=True)
            except Exception:
                pass
        raise


# ══════════════════════════════════════════════════════════════
# 快速模式（一次性写入）
# ══════════════════════════════════════════════════════════════

def _fill_document(doc, project: Dict[str, Any], doc_type: str) -> None:
    """将项目数据填充到 WPS COM 文档对象中（快速模式，一次性写入）。"""
    if doc_type == "writer":
        _fill_writer(doc, project)
    elif doc_type == "calc":
        _fill_calc(doc, project)
    elif doc_type == "impress":
        _fill_impress(doc, project)


def _fill_writer(doc, project: Dict[str, Any]) -> None:
    """将内容填充到 WPS Writer 文档。"""
    content_items = project.get("content", [])
    if not content_items:
        return
    for item in content_items:
        item_type = item.get("type", "paragraph")
        try:
            if item_type == "paragraph":
                _add_writer_paragraph(doc, item)
            elif item_type == "heading":
                _add_writer_heading(doc, item)
            elif item_type == "list":
                _add_writer_list(doc, item)
            elif item_type == "table":
                _add_writer_table(doc, item)
            elif item_type == "page_break":
                _add_writer_page_break(doc)
            elif item_type == "image_ref":
                _add_writer_image(doc, item)
        except Exception as e:
            raise RuntimeError(f"写入内容项失败（类型: {item_type}）: {e}")


def _add_writer_paragraph(doc, item: Dict[str, Any]) -> None:
    rng = doc.Range()
    rng.Collapse(0)
    text = item.get("text", "")
    style = item.get("style", {})
    if text:
        rng.InsertAfter(text)
        rng.InsertParagraphAfter()
    _apply_paragraph_style(doc, style)


def _add_writer_heading(doc, item: Dict[str, Any]) -> None:
    text = item.get("text", "")
    level = item.get("level", 1)
    rng = doc.Range()
    rng.Collapse(0)
    rng.InsertAfter(text)
    para = doc.Paragraphs.Last
    try:
        para.Range.ParagraphFormat.OutlineLevel = min(level, 9)
    except Exception:
        pass
    font_sizes = {1: 22, 2: 18, 3: 16, 4: 14, 5: 13, 6: 12}
    try:
        para.Range.Font.Size = font_sizes.get(level, 14)
        para.Range.Font.Bold = True
    except Exception:
        pass
    rng.InsertParagraphAfter()


def _apply_paragraph_style(doc, style: Dict[str, Any]) -> None:
    try:
        para = doc.Paragraphs.Last
        p_format = para.Range.Font
        if "font_size" in style:
            size = str(style["font_size"]).replace("pt", "").strip()
            try:
                p_format.Size = float(size)
            except ValueError:
                pass
        if style.get("bold"):
            p_format.Bold = True
        if style.get("italic"):
            p_format.Italic = True
        if style.get("underline"):
            p_format.Underline = True
    except Exception:
        pass


def _add_writer_list(doc, item: Dict[str, Any]) -> None:
    items = item.get("items", [])
    list_style = item.get("list_style", "bullet")
    rng = doc.Range()
    rng.Collapse(0)
    for i, text in enumerate(items):
        prefix = "• " if list_style == "bullet" else f"{i + 1}. "
        rng.InsertAfter(prefix + text)
        rng.InsertParagraphAfter()


def _add_writer_table(doc, item: Dict[str, Any]) -> None:
    rows = item.get("rows", 2)
    cols = item.get("cols", 2)
    data = item.get("data", [])
    rng = doc.Range()
    rng.Collapse(0)
    table = doc.Tables.Add(rng, rows, cols)
    table.AutoFitBehavior(2)
    for ri, row in enumerate(data):
        for ci, val in enumerate(row):
            if ri < rows and ci < cols:
                try:
                    table.Cell(ri + 1, ci + 1).Range.Text = str(val)
                except Exception:
                    pass
    doc.Range().InsertParagraphAfter()


def _add_writer_page_break(doc) -> None:
    rng = doc.Range()
    rng.Collapse(0)
    rng.InsertBreak(7)


def _add_writer_image(doc, item: Dict[str, Any]) -> None:
    image_path = item.get("path", "")
    if not image_path:
        return
    image_path = os.path.abspath(image_path)
    if not os.path.exists(image_path):
        return
    rng = doc.Range()
    rng.Collapse(0)
    try:
        shape = doc.InlineShapes.AddPicture(image_path, Range=rng)
        # 设置图片尺寸
        width = item.get("width", "")
        height = item.get("height", "")
        if width:
            try:
                shape.Width = _parse_measurement(width)
            except Exception:
                pass
        if height:
            try:
                shape.Height = _parse_measurement(height)
            except Exception:
                pass
    except Exception:
        pass
    doc.Range().InsertParagraphAfter()


def _parse_measurement(value: str) -> float:
    """将 '10cm' / '200pt' / '100px' 转为磅值。"""
    value = str(value).strip().lower()
    if value.endswith("cm"):
        return float(value[:-2]) * 28.35  # 1cm ≈ 28.35pt
    elif value.endswith("pt"):
        return float(value[:-2])
    elif value.endswith("px"):
        return float(value[:-2]) * 0.75
    elif value.endswith("inch"):
        return float(value[:-4]) * 72
    else:
        return float(value)


def _fill_calc(doc, project: Dict[str, Any]) -> None:
    """将内容填充到 WPS Calc 工作簿。"""
    sheets = project.get("sheets", [])
    try:
        while doc.Worksheets.Count > len(sheets):
            doc.Worksheets(1).Delete()
    except Exception:
        pass
    for si, sheet_data in enumerate(sheets):
        if si == 0:
            ws = doc.Worksheets(1)
        else:
            ws = doc.Worksheets.Add()
            # 注意：Move 延迟到写入数据之后，避免活动表切换导致 COM 错误
        ws.Name = sheet_data.get("name", f"Sheet{si + 1}")
        for ref, cell in sheet_data.get("cells", {}).items():
            try:
                cell_value = cell.get("value", "")
                cell_type = cell.get("type", "string")
                # 如果有公式，只设 Formula（避免 Value 先写空字符串导致 COM 错误）
                if cell.get("formula"):
                    ws.Range(ref).Formula = cell["formula"]
                elif cell_type == "float":
                    try:
                        ws.Range(ref).Value = float(cell_value)
                    except (ValueError, TypeError):
                        ws.Range(ref).Value = str(cell_value)
                else:
                    if cell_value:
                        ws.Range(ref).Value = str(cell_value)
            except Exception:
                pass
        for merge in sheet_data.get("merged_cells", []):
            try:
                ws.Range(f"{merge['start']}:{merge['end']}").Merge()
            except Exception:
                pass
        # 注意：不使用 ws.Move()，WPS ET 中 Move 会删除工作表
        # 新工作表默认插入到位置1，顺序反转但所有工作表都会保留


def _fill_impress(doc, project: Dict[str, Any]) -> None:
    """将内容填充到 WPS Impress 演示文稿。"""
    slides = project.get("slides", [])
    if not slides:
        return
    for si, slide_data in enumerate(slides):
        slide = doc.Slides.Add(si + 1, 2)
        title = slide_data.get("title", "")
        content = slide_data.get("content", "")
        for shape in slide.Shapes:
            try:
                if shape.Type == 14:  # msoPlaceholder
                    ph_type = int(shape.PlaceholderFormat.Type)
                    if title and ph_type in (1, 3):  # ppPlaceholderTitle, ppPlaceholderCenterTitle
                        shape.TextFrame.TextRange.Text = title
                        continue
                    if content and ph_type in (2, 4, 7):  # ppPlaceholderBody, ppPlaceholderSubtitle, ppPlaceholderContent
                        shape.TextFrame.TextRange.Text = content
                        continue
            except Exception:
                pass
        # 处理元素
        for elem in slide_data.get("elements", []):
            try:
                _add_impress_element(slide, elem)
            except Exception:
                pass


def _add_impress_element(slide, elem: Dict[str, Any]) -> None:
    """向幻灯片添加元素（快速模式）。"""
    elem_type = elem.get("type", "text_box")
    x = _parse_measurement(elem.get("x", "2cm"))
    y = _parse_measurement(elem.get("y", "2cm"))
    w = _parse_measurement(elem.get("width", "10cm"))
    h = _parse_measurement(elem.get("height", "5cm"))

    if elem_type == "text_box":
        shape = slide.Shapes.AddTextbox(1, x, y, w, h)  # msoTextOrientationHorizontal=1
        if elem.get("text"):
            shape.TextFrame.TextRange.Text = elem["text"]
    elif elem_type == "picture" or elem_type == "image":
        path = elem.get("path", elem.get("src", ""))
        if path and os.path.exists(path):
            slide.Shapes.AddPicture(path, False, True, x, y, w, h)


# ══════════════════════════════════════════════════════════════
# Live 模式（逐字打字，肉眼可见）
# ══════════════════════════════════════════════════════════════

class LiveController:
    """Live 模式控制器，统一管理速度和打字效果。"""

    def __init__(self, app, config: Dict[str, float]):
        self.app = app
        self.config = config

    def _jitter(self) -> float:
        return random.uniform(0, self.config["jitter"])

    def typewriter(self, rng, text: str) -> None:
        """逐字写入文本到 Range 对象。"""
        delay = self.config["char_delay"]
        for ch in text:
            rng.InsertAfter(ch)
            rng.Collapse(wdCollapseEnd)
            time.sleep(delay + self._jitter())

    def typewriter_selection(self, text: str) -> None:
        """逐字写入文本到当前 Selection（用于表格单元格）。"""
        sel = self.app.Selection
        delay = self.config["char_delay"]
        for ch in text:
            sel.TypeText(ch)
            time.sleep(delay + self._jitter())

    def pause(self, key: str = "item_pause") -> None:
        """按速度配置暂停。"""
        time.sleep(self.config.get(key, 0.1))

    def select_cell_and_type(self, table, row: int, col: int, text: str,
                             bold: bool = False, font_size: int = 11) -> None:
        """选中表格单元格并逐字输入。"""
        cell = table.Cell(row, col)
        cell.Select()
        sel = self.app.Selection
        sel.Font.Size = font_size
        sel.Font.Bold = bold
        if bold:
            sel.ParagraphFormat.Alignment = wdAlignParagraphCenter
        self.typewriter_selection(text)

    def insert_image_with_fade(self, doc, image_path: str,
                                width: Optional[str] = None,
                                height: Optional[str] = None) -> Any:
        """插入图片（Live 模式带停顿效果）。"""
        rng = doc.Range()
        rng.Collapse(wdCollapseEnd)
        shape = doc.InlineShapes.AddPicture(image_path, Range=rng)

        # 设置尺寸
        if width:
            try:
                shape.Width = _parse_measurement(width)
            except Exception:
                pass
        if height:
            try:
                shape.Height = _parse_measurement(height)
            except Exception:
                pass

        # 图片插入后停顿
        self.pause("image_pause")
        return shape


def _fill_document_live(app, doc, project: Dict[str, Any], doc_type: str,
                         config: Dict[str, float]) -> None:
    """Live 模式：打开可见 WPS 窗口，逐字写入内容。"""
    ctrl = LiveController(app, config)

    if doc_type == "writer":
        _fill_writer_live(ctrl, doc, project)
    elif doc_type == "calc":
        _fill_calc_live(ctrl, doc, project)
    elif doc_type == "impress":
        _fill_impress_live(ctrl, doc, project)


# ── Writer Live ───────────────────────────────────────────────

def _fill_writer_live(ctrl: LiveController, doc, project: Dict[str, Any]) -> None:
    """Live 模式填充 Writer：逐字打字。"""
    content_items = project.get("content", [])
    if not content_items:
        return

    for idx, item in enumerate(content_items):
        item_type = item.get("type", "paragraph")
        try:
            if item_type == "paragraph":
                _add_writer_paragraph_live(ctrl, doc, item)
            elif item_type == "heading":
                _add_writer_heading_live(ctrl, doc, item)
            elif item_type == "list":
                _add_writer_list_live(ctrl, doc, item)
            elif item_type == "table":
                _add_writer_table_live(ctrl, doc, item)
            elif item_type == "page_break":
                _add_writer_page_break(doc)
                ctrl.pause("item_pause")
            elif item_type == "image_ref":
                _add_writer_image_live(ctrl, doc, item)
        except Exception as e:
            raise RuntimeError(f"Live 写入失败（类型: {item_type}）: {e}")


def _add_writer_paragraph_live(ctrl: LiveController, doc, item: Dict[str, Any]) -> None:
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    text = item.get("text", "")
    style = item.get("style", {})
    if text:
        ctrl.typewriter(rng, text)
        rng.InsertParagraphAfter()
        rng.Collapse(wdCollapseEnd)
    _apply_paragraph_style(doc, style)
    ctrl.pause("item_pause")


def _add_writer_heading_live(ctrl: LiveController, doc, item: Dict[str, Any]) -> None:
    text = item.get("text", "")
    level = item.get("level", 1)
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    ctrl.typewriter(rng, text)

    para = doc.Paragraphs.Last
    try:
        para.Range.ParagraphFormat.OutlineLevel = min(level, 9)
    except Exception:
        pass

    font_sizes = {1: 22, 2: 18, 3: 16, 4: 14, 5: 13, 6: 12}
    try:
        para.Range.Font.Size = font_sizes.get(level, 14)
        para.Range.Font.Bold = True
    except Exception:
        pass
    rng.InsertParagraphAfter()
    ctrl.pause("heading_pause")


def _add_writer_list_live(ctrl: LiveController, doc, item: Dict[str, Any]) -> None:
    items = item.get("items", [])
    list_style = item.get("list_style", "bullet")
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    for i, text in enumerate(items):
        prefix = "• " if list_style == "bullet" else f"{i + 1}. "
        ctrl.typewriter(rng, prefix + text)
        rng.InsertParagraphAfter()
        rng.Collapse(wdCollapseEnd)
        ctrl.pause("list_pause")


def _add_writer_table_live(ctrl: LiveController, doc, item: Dict[str, Any]) -> None:
    rows = item.get("rows", 2)
    cols = item.get("cols", 2)
    data = item.get("data", [])
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    table = doc.Tables.Add(rng, rows, cols)
    table.AutoFitBehavior(2)

    for ri, row in enumerate(data):
        for ci, val in enumerate(row):
            if ri < rows and ci < cols:
                try:
                    val_str = str(val) if val else ""
                    if val_str:
                        ctrl.select_cell_and_type(
                            table, ri + 1, ci + 1, val_str,
                            bold=(ri == 0), font_size=11,
                        )
                        ctrl.pause("cell_pause")
                except Exception:
                    pass
        ctrl.pause("row_pause")

    doc.Range().InsertParagraphAfter()
    ctrl.pause("item_pause")


def _add_writer_image_live(ctrl: LiveController, doc, item: Dict[str, Any]) -> None:
    """Live 模式插入图片：可见插入过程 + 尺寸设置。"""
    image_path = item.get("path", "")
    if not image_path:
        return
    image_path = os.path.abspath(image_path)
    if not os.path.exists(image_path):
        return

    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    # 先插入一个空段落作为占位
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)

    # 插入图片
    width = item.get("width")
    height = item.get("height")
    ctrl.insert_image_with_fade(doc, image_path, width=width, height=height)

    # 图片后加一个空段落
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    rng.InsertParagraphAfter()


# ── Calc Live ─────────────────────────────────────────────────

def _fill_calc_live(ctrl: LiveController, doc, project: Dict[str, Any]) -> None:
    """Live 模式填充 Calc：逐格写入。"""
    sheets = project.get("sheets", [])

    try:
        while doc.Worksheets.Count > len(sheets):
            doc.Worksheets(1).Delete()
    except Exception:
        pass

    for si, sheet_data in enumerate(sheets):
        if si == 0:
            ws = doc.Worksheets(1)
        else:
            ws = doc.Worksheets.Add()
        ws.Name = sheet_data.get("name", f"Sheet{si + 1}")
        ctrl.pause("item_pause")

        for ref, cell in sheet_data.get("cells", {}).items():
            try:
                cell_value = str(cell.get("value", ""))
                cell_type = cell.get("type", "string")
                cell_range = ws.Range(ref)

                # 选中单元格用于视觉效果
                cell_range.Select()

                # 逐字递增设置值（避免进入编辑模式导致值丢失）
                if cell_value:
                    for i in range(1, len(cell_value) + 1):
                        cell_range.Value = cell_value[:i]
                        time.sleep(ctrl.config["char_delay"] + ctrl._jitter())

                # 设置公式
                if cell.get("formula"):
                    cell_range.Formula = cell["formula"]

                # 设置数字格式
                if cell_type == "float":
                    try:
                        cell_range.NumberFormat = "0.00"
                    except Exception:
                        pass

                ctrl.pause("cell_pause")
            except Exception:
                pass

        for merge in sheet_data.get("merged_cells", []):
            try:
                ws.Range(f"{merge['start']}:{merge['end']}").Merge()
                ctrl.pause("cell_pause")
            except Exception:
                pass
        # 注意：不使用 ws.Move()，WPS ET 中 Move 会删除工作表
        # 新工作表默认插入到位置1，顺序反转但所有工作表都会保留


# ── Impress Live ──────────────────────────────────────────────

def _fill_impress_live(ctrl: LiveController, doc, project: Dict[str, Any]) -> None:
    """Live 模式填充 Impress：逐张幻灯片写入。"""
    slides = project.get("slides", [])
    if not slides:
        return

    for si, slide_data in enumerate(slides):
        slide = doc.Slides.Add(si + 1, 2)
        title = slide_data.get("title", "")
        content = slide_data.get("content", "")

        for shape in slide.Shapes:
            try:
                if shape.Type == 14:  # msoPlaceholder
                    ph_type = int(shape.PlaceholderFormat.Type)
                    if title and ph_type in (1, 3):  # ppPlaceholderTitle, ppPlaceholderCenterTitle
                        tr = shape.TextFrame.TextRange
                        tr.Text = ""
                        delay = ctrl.config["char_delay"]
                        for ch in title:
                            tr.Text = tr.Text + ch
                            time.sleep(delay + ctrl._jitter())
                        continue
                    if content and ph_type in (2, 4, 7):  # ppPlaceholderBody, ppPlaceholderSubtitle, ppPlaceholderContent
                        tr = shape.TextFrame.TextRange
                        tr.Text = ""
                        delay = ctrl.config["char_delay"]
                        for ch in content:
                            tr.Text = tr.Text + ch
                            time.sleep(delay + ctrl._jitter())
                        continue
            except Exception:
                pass

        # 处理元素
        for elem in slide_data.get("elements", []):
            try:
                _add_impress_element_live(ctrl, slide, elem)
            except Exception:
                pass

        ctrl.pause("slide_pause")


def _add_impress_element_live(ctrl: LiveController, slide, elem: Dict[str, Any]) -> None:
    """Live 模式向幻灯片添加元素。"""
    elem_type = elem.get("type", "text_box")
    x = _parse_measurement(elem.get("x", "2cm"))
    y = _parse_measurement(elem.get("y", "2cm"))
    w = _parse_measurement(elem.get("width", "10cm"))
    h = _parse_measurement(elem.get("height", "5cm"))

    if elem_type == "text_box":
        shape = slide.Shapes.AddTextbox(1, x, y, w, h)
        text = elem.get("text", "")
        if text:
            tr = shape.TextFrame.TextRange
            tr.Text = ""
            delay = ctrl.config["char_delay"]
            for ch in text:
                tr.Text = tr.Text + ch
                time.sleep(delay + ctrl._jitter())
        ctrl.pause("item_pause")

    elif elem_type == "picture" or elem_type == "image":
        path = elem.get("path", elem.get("src", ""))
        if path and os.path.exists(path):
            slide.Shapes.AddPicture(path, False, True, x, y, w, h)
            ctrl.pause("image_pause")
