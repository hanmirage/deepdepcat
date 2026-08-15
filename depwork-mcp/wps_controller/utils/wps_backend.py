"""WPS COM 后端 —— 通过 COM 自动化接口操控 WPS Office。

本模块是 CLI 与 WPS Office 之间的桥梁。利用 Windows COM 接口
（KWPS.Application / KET.Application / KWPP.Application）实现：

- 文档的创建、打开和保存
- 内容编辑（文字、表格、演示文稿）
- 格式导出（PDF、DOCX、XLSX、PPTX 等）

要求：Windows 系统 + WPS Office（已安装）+ pywin32
"""

import os
import subprocess
from typing import Optional

# COM 常量 —— 与 Microsoft Office 兼容
# ── Word 格式 ──
wdFormatDocumentDefault = 16      # .docx
wdFormatPDF = 17
wdFormatText = 2
wdFormatRTF = 6
wdFormatFilteredHTML = 10
wdFormatDocument97 = 0            # .doc
wdFormatXMLDocument = 12
wdFormatOpenDocumentText = 23     # .odt
wdFormatXPS = 18

# ── Excel 格式 ──
xlOpenXMLWorkbook = 51            # .xlsx
xlCSV = 6
xlCSVUTF8 = 62
xlHtml = 44
xlWorkbookNormal = -4143          # .xls
xlPDF = 0

# ── PowerPoint 格式 ──
ppSaveAsPresentation = 1          # .ppt
ppSaveAsPDF = 32
ppSaveAsHTML = 12
ppSaveAsOpenXMLPresentation = 24  # .pptx

# WPS COM ProgID 映射
PROGID_MAP = {
    "writer": "KWPS.Application",
    "wps": "KWPS.Application",
    "calc": "KET.Application",
    "et": "KET.Application",
    "impress": "KWPP.Application",
    "wpp": "KWPP.Application",
}

# 格式映射表
FORMAT_SAVEAS_MAP = {
    "writer": {
        "docx": wdFormatDocumentDefault,
        "doc": wdFormatDocument97,
        "pdf": wdFormatPDF,
        "txt": wdFormatText,
        "html": wdFormatFilteredHTML,
        "rtf": wdFormatRTF,
        "xml": wdFormatXMLDocument,
        "odt": wdFormatOpenDocumentText,
        "xps": wdFormatXPS,
    },
    "calc": {
        "xlsx": xlOpenXMLWorkbook,
        "xls": xlWorkbookNormal,
        "csv": xlCSVUTF8,
        "html": xlHtml,
        "pdf": xlPDF,
    },
    "impress": {
        "pptx": ppSaveAsOpenXMLPresentation,
        "ppt": ppSaveAsPresentation,
        "pdf": ppSaveAsPDF,
        "html": ppSaveAsHTML,
    },
}


def find_wps(app_type: str = "writer", visible: bool = False):
    """获取 WPS COM Application 对象。

    Args:
        app_type: 应用类型 —— "writer" / "calc" / "impress"
        visible: WPS 窗口是否可见

    Returns:
        win32com COM Dispatch 对象

    Raises:
        RuntimeError: 无法创建 WPS COM 对象
        ValueError: 不支持的应用类型
    """
    progid = PROGID_MAP.get(app_type.lower())
    if not progid:
        raise ValueError(
            f"不支持的应用类型: {app_type}。有效值: {', '.join(sorted(set(PROGID_MAP.keys())))}"
        )

    try:
        import win32com.client
        import pythoncom
        pythoncom.CoInitialize()
        app = win32com.client.Dispatch(progid)
        try:
            app.Visible = visible
        except Exception:
            pass  # 某些应用 Visible 不可写
        return app
    except ImportError:
        raise RuntimeError("缺少 pywin32 库。请运行: pip install pywin32")
    except Exception as e:
        raise RuntimeError(
            f"无法创建 {app_type} COM 对象 ({progid})。"
            f"请确认 WPS Office 已正确安装。错误: {e}"
        )


def get_version(app=None) -> str:
    """获取 WPS 版本号。"""
    if app is None:
        app = find_wps("writer")
        try:
            return app.Version
        finally:
            quit_app(app)
    return app.Version


def create_document(app, doc_type: str = "writer"):
    """在 WPS 中创建新文档。"""
    if doc_type in ("writer", "wps"):
        return app.Documents.Add()
    elif doc_type in ("calc", "et"):
        return app.Workbooks.Add()
    elif doc_type in ("impress", "wpp"):
        return app.Presentations.Add()
    else:
        raise ValueError(f"不支持的文档类型: {doc_type}")


def open_document(app, path: str):
    """打开现有文档。"""
    abs_path = os.path.abspath(path)
    if not os.path.exists(abs_path):
        raise FileNotFoundError(f"文件不存在: {abs_path}")

    ext = os.path.splitext(abs_path)[1].lower()
    if ext in (".doc", ".docx", ".wps", ".wpt", ".rtf", ".txt", ".dot", ".dotx"):
        return app.Documents.Open(abs_path)
    elif ext in (".xls", ".xlsx", ".et", ".csv", ".xlt", ".xltx"):
        return app.Workbooks.Open(abs_path)
    elif ext in (".ppt", ".pptx", ".dps", ".dpt", ".pot", ".potx"):
        return app.Presentations.Open(abs_path)
    else:
        raise ValueError(f"不支持的文件格式: {ext}")


def save_as(doc, path: str, doc_type: str = "writer", format_name: Optional[str] = None):
    """另存为指定格式。

    对于 ET/WPP，优先使用位置参数调用 SaveAs（兼容性更好）。
    如果目标文件已存在，先删除以避免锁定问题。
    """
    abs_path = os.path.abspath(path)
    os.makedirs(os.path.dirname(abs_path) or ".", exist_ok=True)

    if format_name is None:
        format_name = os.path.splitext(abs_path)[1].lower().lstrip(".")

    formats = FORMAT_SAVEAS_MAP.get(doc_type, {})
    fmt_const = formats.get(format_name)

    # 如果文件已存在，先删除（避免 COM 锁定错误）
    if os.path.exists(abs_path):
        try:
            os.remove(abs_path)
        except PermissionError:
            # 文件被锁定，尝试重命名后删除
            try:
                tmp = abs_path + ".old"
                os.rename(abs_path, tmp)
                os.remove(tmp)
            except Exception:
                pass

    # Writer 支持 SaveAs2；ET/WPP 用 SaveAs（位置参数）
    if doc_type in ("writer", "wps"):
        try:
            if fmt_const is None:
                doc.SaveAs2(abs_path)
            else:
                doc.SaveAs2(abs_path, FileFormat=fmt_const)
        except Exception:
            if fmt_const is None:
                doc.SaveAs(abs_path)
            else:
                doc.SaveAs(abs_path, fmt_const)
    else:
        # ET/WPP：使用位置参数（FileFormat 作为第 2 个参数）
        if fmt_const is None:
            doc.SaveAs(abs_path)
        else:
            doc.SaveAs(abs_path, fmt_const)

    return abs_path


def export_pdf(doc, output_path: str, doc_type: str = "writer"):
    """导出为 PDF。"""
    abs_path = os.path.abspath(output_path)
    os.makedirs(os.path.dirname(abs_path) or ".", exist_ok=True)

    # 如果文件已存在，先删除
    if os.path.exists(abs_path):
        try:
            os.remove(abs_path)
        except PermissionError:
            try:
                tmp = abs_path + ".old"
                os.rename(abs_path, tmp)
                os.remove(tmp)
            except Exception:
                pass

    if doc_type == "impress":
        doc.SaveAs(abs_path, ppSaveAsPDF)
    elif doc_type == "calc":
        # Excel/KET: ExportAsFixedFormat(Type, Filename) — Type=0 是 xlTypePDF
        doc.ExportAsFixedFormat(0, abs_path)
    else:
        doc.ExportAsFixedFormat(abs_path, 17)  # Word: (OutputFileName, 17 = wdFormatPDF)
    return abs_path


def close_document(doc, save: bool = False):
    """关闭文档。"""
    try:
        if save:
            doc.Save()
        doc.Close()
    except Exception:
        pass


def quit_app(app, force: bool = False):
    """退出 WPS 应用程序。"""
    try:
        if force:
            try:
                app.DisplayAlerts = False
            except Exception:
                pass
        app.Quit()
    except Exception:
        pass


def kill_all_wps_processes():
    """终止所有 WPS 相关后台进程。"""
    wps_names = ["wps.exe", "et.exe", "wpp.exe", "wpscloudsvr.exe"]
    for name in wps_names:
        try:
            subprocess.run(
                ["taskkill", "/F", "/IM", name, "/T"],
                capture_output=True, text=True,
            )
        except Exception:
            pass


def is_wps_running() -> bool:
    """检查是否有 WPS 进程在运行。"""
    try:
        result = subprocess.run(
            ["tasklist", "/FI", "IMAGENAME eq wps.exe"],
            capture_output=True, text=True,
        )
        return "wps.exe" in result.stdout.lower()
    except Exception:
        return False


def get_doc_type_from_ext(path: str) -> str:
    """根据文件扩展名判断文档类型。"""
    ext = os.path.splitext(path)[1].lower()
    if ext in (".doc", ".docx", ".wps", ".rtf", ".txt", ".odt"):
        return "writer"
    elif ext in (".xls", ".xlsx", ".et", ".csv"):
        return "calc"
    elif ext in (".ppt", ".pptx", ".dps"):
        return "impress"
    return "writer"
