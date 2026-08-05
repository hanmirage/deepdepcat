#!/usr/bin/env python3
"""WPS Office CLI —— 通过命令行操控 WPS Office 文档。

用法:
    # 一次性命令（适合 Rust subprocess 调用）
    python -m wps_controller.wps_cli --json document new --type writer --name "报告"
    python -m wps_controller.wps_cli --json --project report.json writer add-paragraph -t "Hello"
    python -m wps_controller.wps_cli --json --project report.json export render output.docx -p docx

    # 所有命令加 --json 返回结构化 JSON，Rust 可直接解析
"""

import sys
import os
import json
import click
from typing import Optional

# 确保包可导入
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from wps_controller.core.session import Session
from wps_controller.core import document as doc_mod
from wps_controller.core import writer as writer_mod
from wps_controller.core import calc as calc_mod
from wps_controller.core import impress as impress_mod
from wps_controller.core import styles as styles_mod
from wps_controller.core import export as export_mod

# 全局会话状态
_session: Optional[Session] = None
_json_output = False
_repl_mode = False


def get_session() -> Session:
    global _session
    if _session is None:
        _session = Session()
    return _session


def output(data, message: str = ""):
    if _json_output:
        click.echo(json.dumps(data, indent=2, default=str, ensure_ascii=False))
    else:
        if message:
            click.echo(message)
        if isinstance(data, dict):
            _print_dict(data)
        elif isinstance(data, list):
            _print_list(data)
        else:
            click.echo(str(data))


def _print_dict(d: dict, indent: int = 0):
    prefix = "  " * indent
    for k, v in d.items():
        if isinstance(v, dict):
            click.echo(f"{prefix}{k}:")
            _print_dict(v, indent + 1)
        elif isinstance(v, list):
            click.echo(f"{prefix}{k}:")
            _print_list(v, indent + 1)
        else:
            click.echo(f"{prefix}{k}: {v}")


def _print_list(items: list, indent: int = 0):
    prefix = "  " * indent
    for i, item in enumerate(items):
        if isinstance(item, dict):
            click.echo(f"{prefix}[{i}]")
            _print_dict(item, indent + 1)
        else:
            click.echo(f"{prefix}- {item}")


def handle_error(func):
    def wrapper(*args, **kwargs):
        try:
            return func(*args, **kwargs)
        except FileNotFoundError as e:
            if _json_output:
                click.echo(json.dumps({"error": str(e), "type": "file_not_found"}, ensure_ascii=False))
            else:
                click.echo(f"错误: {e}", err=True)
            if not _repl_mode:
                sys.exit(1)
        except FileExistsError as e:
            if _json_output:
                click.echo(json.dumps({"error": str(e), "type": "file_exists"}, ensure_ascii=False))
            else:
                click.echo(f"错误: {e}", err=True)
            if not _repl_mode:
                sys.exit(1)
        except (ValueError, IndexError, RuntimeError) as e:
            if _json_output:
                click.echo(json.dumps({"error": str(e), "type": type(e).__name__}, ensure_ascii=False))
            else:
                click.echo(f"错误: {e}", err=True)
            if not _repl_mode:
                sys.exit(1)
    wrapper.__name__ = func.__name__
    wrapper.__doc__ = func.__doc__
    return wrapper


# ── 主 CLI ──────────────────────────────────────────────────

@click.group(invoke_without_command=True)
@click.option("--json", "use_json", is_flag=True, help="以 JSON 格式输出（Rust 调用必用）")
@click.option("--project", "project_path", type=str, default=None, help="项目 JSON 文件路径")
@click.option("--dry-run", "dry_run", is_flag=True, default=False, help="运行但不保存")
@click.pass_context
def cli(ctx, use_json, project_path, dry_run):
    """WPS Office CLI —— 从命令行编辑文档。"""
    global _json_output
    _json_output = use_json

    if project_path:
        sess = get_session()
        if not sess.has_project():
            proj = doc_mod.open_document(project_path)
            sess.set_project(proj, project_path)

    if ctx.invoked_subcommand is None:
        click.echo(ctx.get_help())


@cli.result_callback()
def auto_save_on_exit(result, use_json, project_path, dry_run, **kwargs):
    """单次命令执行后自动保存项目。"""
    if _repl_mode or dry_run:
        return
    sess = get_session()
    if sess.has_project() and sess._modified and sess.project_path:
        try:
            sess.save_session()
        except Exception as e:
            if _json_output:
                click.echo(json.dumps({"warning": f"自动保存失败: {e}"}, ensure_ascii=False))
            else:
                click.echo(f"警告: 自动保存失败: {e}", err=True)


# ── 文档命令 ──────────────────────────────────────────────────

@cli.group()
def document():
    """文档管理命令。"""


@document.command("new")
@click.option("--type", "doc_type", type=click.Choice(["writer", "calc", "impress"]), default="writer")
@click.option("--name", "-n", default="untitled")
@click.option("--profile", "-p", type=str, default=None)
@click.option("--output", "-o", "output_path", type=str, default=None)
@handle_error
def document_new(doc_type, name, profile, output_path):
    """创建新文档。"""
    proj = doc_mod.create_document(doc_type=doc_type, name=name, profile=profile)
    sess = get_session()
    sess.set_project(proj, output_path)
    if output_path:
        doc_mod.save_document(proj, output_path)
    info = doc_mod.get_document_info(proj)
    output(info, f"创建了 {doc_type} 文档: {name}")


@document.command("open")
@click.argument("path")
@handle_error
def document_open(path):
    """打开项目 JSON 文件。"""
    proj = doc_mod.open_document(path)
    sess = get_session()
    sess.set_project(proj, path)
    info = doc_mod.get_document_info(proj)
    output(info, f"已打开: {path}")


@document.command("save")
@click.argument("path", required=False)
@handle_error
def document_save(path):
    """保存当前文档。"""
    sess = get_session()
    saved = sess.save_session(path)
    output({"saved": saved}, f"已保存到: {saved}")


@document.command("info")
@handle_error
def document_info():
    """显示文档信息。"""
    sess = get_session()
    info = doc_mod.get_document_info(sess.get_project())
    output(info)


@document.command("profiles")
@handle_error
def document_profiles():
    """列出可用的页面配置。"""
    profiles = doc_mod.list_profiles()
    output(profiles, "可用的页面配置:")


@document.command("json")
@handle_error
def document_json():
    """打印原始项目 JSON。"""
    sess = get_session()
    click.echo(json.dumps(sess.get_project(), indent=2, default=str, ensure_ascii=False))


# ── Writer 命令 ────────────────────────────────────────────────

@cli.group()
def writer():
    """Writer（文字处理）命令。"""


@writer.command("add-paragraph")
@click.option("--text", "-t", default="")
@click.option("--position", "-p", type=int, default=None)
@click.option("--font-size", type=str, default=None)
@click.option("--bold", is_flag=True)
@click.option("--italic", is_flag=True)
@click.option("--alignment", type=click.Choice(["left", "center", "right", "justify"]), default=None)
@handle_error
def writer_add_paragraph(text, position, font_size, bold, italic, alignment):
    """添加段落。"""
    sess = get_session()
    sess.snapshot("添加段落")
    style = {}
    if font_size: style["font_size"] = font_size
    if bold: style["bold"] = True
    if italic: style["italic"] = True
    if alignment: style["alignment"] = alignment
    item = writer_mod.add_paragraph(sess.get_project(), text=text, style=style or None, position=position)
    output(item, "已添加段落")


@writer.command("add-heading")
@click.option("--text", "-t", default="")
@click.option("--level", "-l", type=int, default=1)
@click.option("--position", "-p", type=int, default=None)
@handle_error
def writer_add_heading(text, level, position):
    """添加标题。"""
    sess = get_session()
    sess.snapshot("添加标题")
    item = writer_mod.add_heading(sess.get_project(), text=text, level=level, position=position)
    output(item, f"已添加标题（级别 {level}）")


@writer.command("add-list")
@click.option("--items", "-i", multiple=True)
@click.option("--style", "list_style", type=click.Choice(["bullet", "number"]), default="bullet")
@click.option("--position", "-p", type=int, default=None)
@handle_error
def writer_add_list(items, list_style, position):
    """添加列表。"""
    sess = get_session()
    sess.snapshot("添加列表")
    item = writer_mod.add_list(sess.get_project(), items=list(items), list_style=list_style, position=position)
    output(item, f"已添加 {list_style} 列表")


@writer.command("add-table")
@click.option("--rows", "-r", type=int, default=2)
@click.option("--cols", "-c", type=int, default=2)
@click.option("--position", "-p", type=int, default=None)
@handle_error
def writer_add_table(rows, cols, position):
    """添加表格。"""
    sess = get_session()
    sess.snapshot("添加表格")
    item = writer_mod.add_table(sess.get_project(), rows=rows, cols=cols, position=position)
    output(item, f"已添加 {rows}x{cols} 表格")


@writer.command("add-page-break")
@click.option("--position", "-p", type=int, default=None)
@handle_error
def writer_add_page_break(position):
    """添加分页符。"""
    sess = get_session()
    sess.snapshot("添加分页符")
    item = writer_mod.add_page_break(sess.get_project(), position=position)
    output(item, "已添加分页符")


@writer.command("add-image")
@click.argument("image_path")
@click.option("--width", "-w", default="10cm")
@click.option("--height", "-h", default="10cm")
@click.option("--position", "-p", type=int, default=None)
@handle_error
def writer_add_image(image_path, width, height, position):
    """添加图片。"""
    sess = get_session()
    sess.snapshot("添加图片")
    item = writer_mod.add_image(sess.get_project(), image_path, width=width, height=height, position=position)
    output(item, f"已添加图片: {image_path}")


@writer.command("remove")
@click.argument("index", type=int)
@handle_error
def writer_remove(index):
    """按索引删除内容项。"""
    sess = get_session()
    sess.snapshot(f"删除内容 {index}")
    removed = writer_mod.remove_content(sess.get_project(), index)
    output(removed, f"已删除内容索引 {index}")


@writer.command("list")
@handle_error
def writer_list():
    """列出所有内容项。"""
    sess = get_session()
    items = writer_mod.list_content(sess.get_project())
    output(items, "内容项:")


@writer.command("set-text")
@click.argument("index", type=int)
@click.argument("text")
@handle_error
def writer_set_text(index, text):
    """设置内容项的文本。"""
    sess = get_session()
    sess.snapshot(f"设置索引 {index} 的文本")
    item = writer_mod.set_content_text(sess.get_project(), index, text)
    output(item, f"已更新索引 {index} 的文本")


@writer.command("find-replace")
@click.argument("find_text")
@click.argument("replace_text")
@handle_error
def writer_find_replace(find_text, replace_text):
    """查找并替换文本。"""
    sess = get_session()
    sess.snapshot(f"查找替换: {find_text} → {replace_text}")
    result = writer_mod.find_replace(sess.get_project(), find_text, replace_text)
    output(result, f"已替换 {result['replaced']} 处")


# ── Calc 命令 ──────────────────────────────────────────────────

@cli.group()
def calc():
    """Calc（电子表格）命令。"""


@calc.command("add-sheet")
@click.option("--name", "-n", default="Sheet")
@click.option("--position", "-p", type=int, default=None)
@handle_error
def calc_add_sheet(name, position):
    """添加新工作表。"""
    sess = get_session()
    sess.snapshot(f"添加工作表: {name}")
    sheet = calc_mod.add_sheet(sess.get_project(), name=name, position=position)
    output(sheet, f"已添加工作表: {name}")


@calc.command("remove-sheet")
@click.argument("index", type=int)
@handle_error
def calc_remove_sheet(index):
    """按索引删除工作表。"""
    sess = get_session()
    sess.snapshot(f"删除工作表 {index}")
    removed = calc_mod.remove_sheet(sess.get_project(), index)
    output(removed, f"已删除工作表索引 {index}")


@calc.command("rename-sheet")
@click.argument("index", type=int)
@click.argument("name")
@handle_error
def calc_rename_sheet(index, name):
    """重命名工作表。"""
    sess = get_session()
    sess.snapshot(f"重命名工作表 {index}")
    sheet = calc_mod.rename_sheet(sess.get_project(), index, name)
    output(sheet, f"已重命名工作表 {index} 为: {name}")


@calc.command("set-cell")
@click.argument("ref")
@click.argument("value")
@click.option("--type", "cell_type", default="string")
@click.option("--sheet", "-s", type=int, default=0)
@click.option("--formula", type=str, default=None)
@handle_error
def calc_set_cell(ref, value, cell_type, sheet, formula):
    """设置单元格的值。"""
    sess = get_session()
    sess.snapshot(f"设置单元格 {ref}")
    result = calc_mod.set_cell(sess.get_project(), ref=ref, value=value, cell_type=cell_type, sheet=sheet, formula=formula)
    output(result, f"设置 {ref} = {value}")


@calc.command("get-cell")
@click.argument("ref")
@click.option("--sheet", "-s", type=int, default=0)
@handle_error
def calc_get_cell(ref, sheet):
    """获取单元格的值。"""
    sess = get_session()
    result = calc_mod.get_cell(sess.get_project(), ref=ref, sheet=sheet)
    output(result)


@calc.command("set-range")
@click.argument("start_ref")
@click.option("--data", "-d", type=str, required=True, help="JSON 格式的二维数据数组")
@click.option("--sheet", "-s", type=int, default=0)
@handle_error
def calc_set_range(start_ref, data, sheet):
    """批量写入矩形区域数据。"""
    sess = get_session()
    sess.snapshot(f"批量写入从 {start_ref}")
    arr = json.loads(data)
    result = calc_mod.set_range(sess.get_project(), start_ref, arr, sheet=sheet)
    output(result, f"已写入 {result['cells_set']} 个单元格")


@calc.command("merge-cells")
@click.argument("start_ref")
@click.argument("end_ref")
@click.option("--sheet", "-s", type=int, default=0)
@handle_error
def calc_merge_cells(start_ref, end_ref, sheet):
    """合并单元格区域。"""
    sess = get_session()
    sess.snapshot(f"合并单元格 {start_ref}:{end_ref}")
    result = calc_mod.merge_cells(sess.get_project(), start_ref, end_ref, sheet=sheet)
    output(result, f"已标记合并: {start_ref}:{end_ref}")


@calc.command("list-sheets")
@handle_error
def calc_list_sheets():
    """列出所有工作表。"""
    sess = get_session()
    sheets = calc_mod.list_sheets(sess.get_project())
    output(sheets, "工作表:")


# ── Impress 命令 ───────────────────────────────────────────────

@cli.group()
def impress():
    """Impress（演示文稿）命令。"""


@impress.command("add-slide")
@click.option("--title", "-t", default="")
@click.option("--content", "-c", default="")
@click.option("--position", "-p", type=int, default=None)
@handle_error
def impress_add_slide(title, content, position):
    """添加幻灯片。"""
    sess = get_session()
    sess.snapshot("添加幻灯片")
    slide = impress_mod.add_slide(sess.get_project(), title=title, content=content, position=position)
    output(slide, f"已添加幻灯片: {title}")


@impress.command("remove-slide")
@click.argument("index", type=int)
@handle_error
def impress_remove_slide(index):
    """按索引删除幻灯片。"""
    sess = get_session()
    sess.snapshot(f"删除幻灯片 {index}")
    removed = impress_mod.remove_slide(sess.get_project(), index)
    output(removed, f"已删除幻灯片 {index}")


@impress.command("set-content")
@click.argument("index", type=int)
@click.option("--title", "-t", type=str, default=None)
@click.option("--content", "-c", type=str, default=None)
@handle_error
def impress_set_content(index, title, content):
    """更新幻灯片内容。"""
    sess = get_session()
    sess.snapshot(f"更新幻灯片 {index}")
    slide = impress_mod.set_slide_content(sess.get_project(), index, title=title, content=content)
    output(slide, f"已更新幻灯片 {index}")


@impress.command("list-slides")
@handle_error
def impress_list_slides():
    """列出所有幻灯片。"""
    sess = get_session()
    slides = impress_mod.list_slides(sess.get_project())
    output(slides, "幻灯片:")


@impress.command("add-element")
@click.argument("slide_index", type=int)
@click.option("--type", "element_type", default="text_box")
@click.option("--text", "-t", default="")
@click.option("--x", default="2cm")
@click.option("--y", default="2cm")
@click.option("--width", "-w", default="10cm")
@click.option("--height", "-h", default="5cm")
@handle_error
def impress_add_element(slide_index, element_type, text, x, y, width, height):
    """向幻灯片添加元素。"""
    sess = get_session()
    sess.snapshot(f"向幻灯片 {slide_index} 添加元素")
    elem = impress_mod.add_slide_element(
        sess.get_project(), slide_index,
        element_type=element_type, text=text, x=x, y=y, width=width, height=height,
    )
    output(elem, f"已添加 {element_type} 到幻灯片 {slide_index}")


@impress.command("move-slide")
@click.argument("from_index", type=int)
@click.argument("to_index", type=int)
@handle_error
def impress_move_slide(from_index, to_index):
    """移动幻灯片。"""
    sess = get_session()
    sess.snapshot(f"移动幻灯片 {from_index} → {to_index}")
    result = impress_mod.move_slide(sess.get_project(), from_index, to_index)
    output(result, f"已移动幻灯片 {from_index} → {to_index}")


@impress.command("duplicate-slide")
@click.argument("index", type=int)
@handle_error
def impress_duplicate_slide(index):
    """复制幻灯片。"""
    sess = get_session()
    sess.snapshot(f"复制幻灯片 {index}")
    slide = impress_mod.duplicate_slide(sess.get_project(), index)
    output(slide, f"已复制幻灯片 {index}")


# ── 样式命令 ──────────────────────────────────────────────────

@cli.group("style")
def style_group():
    """样式管理命令。"""


@style_group.command("create")
@click.argument("name")
@click.option("--family", type=click.Choice(["paragraph", "text"]), default="paragraph")
@click.option("--parent", type=str, default=None)
@click.option("--prop", "-p", multiple=True, help="属性: key=value")
@handle_error
def style_create(name, family, parent, prop):
    """创建新样式。"""
    props = _parse_props(prop)
    sess = get_session()
    sess.snapshot(f"创建样式: {name}")
    result = styles_mod.create_style(sess.get_project(), name=name, family=family, parent=parent, properties=props)
    output(result, f"已创建样式: {name}")


@style_group.command("modify")
@click.argument("name")
@click.option("--prop", "-p", multiple=True, help="属性: key=value")
@handle_error
def style_modify(name, prop):
    """修改已有样式。"""
    props = _parse_props(prop)
    sess = get_session()
    sess.snapshot(f"修改样式: {name}")
    result = styles_mod.modify_style(sess.get_project(), name=name, properties=props)
    output(result, f"已修改样式: {name}")


@style_group.command("list")
@handle_error
def style_list():
    """列出所有样式。"""
    sess = get_session()
    styles = styles_mod.list_styles(sess.get_project())
    output(styles, "样式:")


@style_group.command("apply")
@click.argument("style_name")
@click.argument("content_index", type=int)
@handle_error
def style_apply(style_name, content_index):
    """将样式应用于 Writer 内容项。"""
    sess = get_session()
    sess.snapshot(f"应用样式 {style_name} 到 {content_index}")
    result = styles_mod.apply_style(sess.get_project(), style_name, content_index)
    output(result, f"已将 '{style_name}' 应用到内容 {content_index}")


@style_group.command("remove")
@click.argument("name")
@handle_error
def style_remove(name):
    """删除样式。"""
    sess = get_session()
    sess.snapshot(f"删除样式: {name}")
    result = styles_mod.remove_style(sess.get_project(), name)
    output(result, f"已删除样式: {name}")


# ── 导出命令 ──────────────────────────────────────────────────

@cli.group("export")
def export_group():
    """导出/渲染命令。"""


@export_group.command("presets")
@handle_error
def export_presets():
    """列出导出预设。"""
    presets = export_mod.list_presets()
    output(presets, "导出预设:")


@export_group.command("preset-info")
@click.argument("name")
@handle_error
def export_preset_info(name):
    """显示预设详情。"""
    info = export_mod.get_preset_info(name)
    output(info)


@export_group.command("render")
@click.argument("output_path")
@click.option("--preset", "-p", default="docx")
@click.option("--overwrite", is_flag=True)
@click.option("--visible", is_flag=True, help="WPS 窗口可见")
@click.option("--live", is_flag=True, help="实时打字模式（逐字写入，肉眼可见）")
@click.option("--delay", default=0.03, help="Live 模式逐字间隔（秒），覆盖 --speed")
@click.option("--speed", type=click.Choice(["fast", "normal", "slow"]), default="normal",
              help="速度预设: fast(0.01s/字) / normal(0.03s/字) / slow(0.08s/字)")
@handle_error
def export_render(output_path, preset, overwrite, visible, live, delay, speed):
    """导出文档到文件（使用 WPS COM 自动化）。

    \b
    模式:
      默认       后台快速生成（窗口不可见）
      --live     WPS 窗口可见，逐字输入（打字机效果）

    \b
    速度（仅 --live 模式）:
      --speed fast    快速打字（0.01s/字）
      --speed normal  正常速度（0.03s/字）
      --speed slow    慢速打字（0.08s/字）
      --delay 0.05    自定义逐字间隔（覆盖 --speed）
    """
    sess = get_session()
    result = export_mod.export(
        sess.get_project(), output_path,
        preset=preset, overwrite=overwrite, visible=visible,
        live=live, live_delay=delay, speed=speed,
    )
    msg = f"已导出到: {result['output']} ({result.get('file_size', 0):,} 字节)"
    if live:
        msg += f" [Live | speed={result.get('speed','normal')} | {result.get('char_delay',0)}s/字]"
    output(result, msg)


# ── 会话命令 ──────────────────────────────────────────────────

@cli.group()
def session():
    """会话管理命令。"""


@session.command("status")
@handle_error
def session_status():
    """显示会话状态。"""
    sess = get_session()
    output(sess.status())


@session.command("undo")
@handle_error
def session_undo():
    """撤销上一步操作。"""
    sess = get_session()
    desc = sess.undo()
    output({"undone": desc}, f"已撤销: {desc}")


@session.command("redo")
@handle_error
def session_redo():
    """重做上一步撤销。"""
    sess = get_session()
    desc = sess.redo()
    output({"redone": desc}, f"已重做: {desc}")


@session.command("history")
@handle_error
def session_history():
    """显示撤销历史。"""
    sess = get_session()
    history = sess.list_history()
    output(history, "撤销历史:")


# ── 辅助 ───────────────────────────────────────────────────────

def _parse_props(prop_list):
    """解析 CLI 中的 key=value 属性对。"""
    props = {}
    for p in prop_list:
        if "=" not in p:
            raise ValueError(f"属性格式无效: '{p}'。请使用 key=value。")
        k, v = p.split("=", 1)
        if v.lower() == "true":
            v = True
        elif v.lower() == "false":
            v = False
        else:
            try:
                v = float(v) if "." in v else int(v)
            except ValueError:
                pass
        props[k] = v
    return props


# ── 入口 ───────────────────────────────────────────────────────

def main():
    cli()


if __name__ == "__main__":
    main()
