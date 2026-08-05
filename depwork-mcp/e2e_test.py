#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""模拟 Rust 后端调度 WPS Controller CLI 的端到端测试。

每个调用都通过 subprocess 执行 CLI（和 Rust Command::new 完全等价），
解析 --json 输出，验证结果。
"""

import json
import subprocess
import os
import sys
import time

# 强制 UTF-8 输出（Windows 兼容）
sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")

WORKDIR = os.path.dirname(os.path.abspath(__file__))
PYTHON = sys.executable
CLI_MODULE = "wps_controller.wps_cli"

# 统计
passed = 0
failed = 0
results = []


def call_cli(args: list[str], expect_success: bool = True) -> dict | None:
    """模拟 Rust 的 Command::new("python").args(...).output()"""
    full_args = [PYTHON, "-m", CLI_MODULE, "--json"] + args

    # 设置子进程环境变量，强制 UTF-8 输出
    env = os.environ.copy()
    env["PYTHONIOENCODING"] = "utf-8"
    env["PYTHONUTF8"] = "1"

    proc = subprocess.run(
        full_args,
        capture_output=True,
        text=True,
        cwd=WORKDIR,
        encoding="utf-8",
        errors="replace",
        env=env,
    )

    ok = (proc.returncode == 0) == expect_success
    stdout_json = None
    stderr_text = (proc.stderr or "").strip()

    try:
        stdout_json = json.loads(proc.stdout) if (proc.stdout or "").strip() else None
    except json.JSONDecodeError:
        stdout_json = {"raw": (proc.stdout or "").strip()[:200]}

    return {
        "ok": ok,
        "returncode": proc.returncode,
        "stdout": stdout_json,
        "stderr": stderr_text,
    }


def test(name: str, args: list[str], expect_success: bool = True,
         check: callable = None, desc: str = ""):
    """执行单个测试用例。"""
    global passed, failed

    result = call_cli(args, expect_success)

    detail = ""
    if check and result["stdout"]:
        detail = check(result["stdout"])

    status = "PASS" if result["ok"] else "FAIL"
    if result["ok"] and detail == "":
        passed += 1
    elif result["ok"] and detail and not detail.startswith("!"):
        passed += 1
    else:
        failed += 1
        if not detail:
            detail = result.get("stderr", "") or "unexpected return code"

    results.append({
        "name": name,
        "status": status,
        "detail": detail or "",
        "desc": desc,
    })

    icon = "✅" if result["ok"] and not (detail and detail.startswith("!")) else "❌"
    print(f"  {icon} {name}" + (f" → {detail}" if detail else ""))
    return result


def check_field(field: str, expected=None):
    """返回一个检查函数，验证 stdout 中的某个字段。"""
    def _check(data):
        if field not in data:
            return f"!字段 {field} 不存在"
        if expected is not None and data[field] != expected:
            return f"!{field}={data[field]}（期望 {expected}）"
        return str(data[field])
    return _check


def check_contains(field: str, substring: str):
    def _check(data):
        val = str(data.get(field, ""))
        if substring in val:
            return val[:80]
        return f"!{field} 中未找到 '{substring}'"
    return _check


def check_file_exists(path_key: str):
    def _check(data):
        path = data.get(path_key, "")
        if os.path.exists(path):
            size = os.path.getsize(path)
            return f"{os.path.basename(path)} ({size:,} bytes)"
        return f"!文件不存在: {path}"
    return _check


# ══════════════════════════════════════════════════════════════
# 1. Writer 测试
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  📝 Writer 测试")
print("=" * 60)

project_path = os.path.join(WORKDIR, "e2e_writer.json")
output_docx = os.path.join(WORKDIR, "e2e_writer.docx")
output_pdf = os.path.join(WORKDIR, "e2e_writer.pdf")

# 1.1 创建文档
test("创建 Writer 文档",
     ["document", "new", "--type", "writer", "--name", "E2E测试报告",
      "-o", project_path],
     check=check_field("type", "writer"))

# 1.2 添加标题
test("添加 H1 标题",
     ["--project", project_path, "writer", "add-heading", "-t", "2024年度总结", "-l", "1"],
     check=check_field("type", "heading"))

# 1.3 添加段落（带格式）
test("添加段落（加粗）",
     ["--project", project_path, "writer", "add-paragraph",
      "-t", "本报告由 AI 自动生成，内容涵盖全年数据分析。", "--bold"],
     check=check_contains("style", "bold"))

# 1.4 添加 H2 标题
test("添加 H2 标题",
     ["--project", project_path, "writer", "add-heading", "-t", "关键指标", "-l", "2"])

# 1.5 添加列表
test("添加无序列表",
     ["--project", project_path, "writer", "add-list",
      "-i", "月活跃用户 1200万", "-i", "营收增长 35%", "-i", "客户满意度 96%",
      "--style", "bullet"],
     check=check_field("list_style", "bullet"))

# 1.6 添加表格
test("添加 4x3 表格",
     ["--project", project_path, "writer", "add-table", "-r", "4", "-c", "3"])

# 1.7 添加分页符
test("添加分页符",
     ["--project", project_path, "writer", "add-page-break"])

# 1.8 添加第二页内容
test("添加第二页标题",
     ["--project", project_path, "writer", "add-heading", "-t", "详细分析", "-l", "1"])

test("添加第二页段落",
     ["--project", project_path, "writer", "add-paragraph", "-t", "这是第二页的正文内容。"])

# 1.9 列出内容
test("列出所有内容项",
     ["--project", project_path, "writer", "list"],
     check=lambda d: f"{len(d)} 项" if isinstance(d, list) else "!非列表")

# 1.10 查找替换
test("查找替换",
     ["--project", project_path, "writer", "find-replace", "AI", "人工智能"],
     check=check_field("replaced"))

# 1.11 导出 DOCX
test("导出 DOCX",
     ["--project", project_path, "export", "render", output_docx,
      "-p", "docx", "--overwrite"],
     check=check_file_exists("output"))

# 1.12 导出 PDF
test("导出 PDF",
     ["--project", project_path, "export", "render", output_pdf,
      "-p", "pdf", "--overwrite"],
     check=check_file_exists("output"))


# ══════════════════════════════════════════════════════════════
# 2. Calc 测试
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  📊 Calc 测试")
print("=" * 60)

calc_path = os.path.join(WORKDIR, "e2e_calc.json")
calc_xlsx = os.path.join(WORKDIR, "e2e_calc.xlsx")

# 2.1 创建
test("创建 Calc 文档",
     ["document", "new", "--type", "calc", "--name", "销售数据", "-o", calc_path],
     check=check_field("type", "calc"))

# 2.2 设置表头
test("设置 A1=产品名",
     ["--project", calc_path, "calc", "set-cell", "A1", "产品名"])
test("设置 B1=销量",
     ["--project", calc_path, "calc", "set-cell", "B1", "销量"])
test("设置 C1=单价",
     ["--project", calc_path, "calc", "set-cell", "C1", "单价"])
test("设置 D1=总额",
     ["--project", calc_path, "calc", "set-cell", "D1", "总额"])

# 2.3 批量写入数据
test("批量写入 A2:D4",
     ["--project", calc_path, "calc", "set-range", "A2",
      "-d", json.dumps([["产品A", 100, 50], ["产品B", 200, 30], ["产品C", 150, 80]])],
     check=check_field("cells_set", 9))

# 2.4 设置公式
test("设置公式 D2=SUM",
     ["--project", calc_path, "calc", "set-cell", "D2", "", "--formula", "=B2*C2"])

# 2.5 合并单元格
test("合并 A1:D1",
     ["--project", calc_path, "calc", "merge-cells", "A1", "D1"])

# 2.6 添加第二个工作表
test("添加工作表[汇总]",
     ["--project", calc_path, "calc", "add-sheet", "-n", "汇总"])

# 2.7 列出工作表
test("列出工作表",
     ["--project", calc_path, "calc", "list-sheets"],
     check=lambda d: f"{len(d)} 个工作表" if isinstance(d, list) else "!非列表")

# 2.8 获取单元格
test("获取 A1 值",
     ["--project", calc_path, "calc", "get-cell", "A1"],
     check=check_field("value", "产品名"))

# 2.9 导出 XLSX
test("导出 XLSX",
     ["--project", calc_path, "export", "render", calc_xlsx,
      "-p", "xlsx", "--overwrite"],
     check=check_file_exists("output"))


# ══════════════════════════════════════════════════════════════
# 3. Impress 测试
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  🎬 Impress 测试")
print("=" * 60)

impress_path = os.path.join(WORKDIR, "e2e_impress.json")
impress_pptx = os.path.join(WORKDIR, "e2e_impress.pptx")

# 3.1 创建
test("创建 Impress 文档",
     ["document", "new", "--type", "impress", "--name", "产品演示", "-o", impress_path],
     check=check_field("type", "impress"))

# 3.2 添加标题幻灯片
test("添加标题幻灯片",
     ["--project", impress_path, "impress", "add-slide", "-t", "产品发布", "-c", "2024年度新品"])

# 3.3 添加内容幻灯片
test("添加内容幻灯片1",
     ["--project", impress_path, "impress", "add-slide", "-t", "功能特性", "-c", "AI驱动\n高性能\n易用性"])

test("添加内容幻灯片2",
     ["--project", impress_path, "impress", "add-slide", "-t", "技术架构", "-c", "微服务架构"])

# 3.4 添加元素
test("添加文本框元素",
     ["--project", impress_path, "impress", "add-element", "0",
      "--type", "text_box", "--text", "Hello from Rust!",
      "--x", "3cm", "--y", "5cm", "--width", "8cm", "--height", "3cm"])

# 3.5 复制幻灯片
test("复制幻灯片 1",
     ["--project", impress_path, "impress", "duplicate-slide", "1"])

# 3.6 移动幻灯片
test("移动幻灯片 3→1",
     ["--project", impress_path, "impress", "move-slide", "3", "1"])

# 3.7 列出幻灯片
test("列出幻灯片",
     ["--project", impress_path, "impress", "list-slides"],
     check=lambda d: f"{len(d)} 张" if isinstance(d, list) else "!非列表")

# 3.8 导出 PPTX
test("导出 PPTX",
     ["--project", impress_path, "export", "render", impress_pptx,
      "-p", "pptx", "--overwrite"],
     check=check_file_exists("output"))


# ══════════════════════════════════════════════════════════════
# 4. 会话管理测试
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  🔄 会话管理测试")
print("=" * 60)

# 4.1 会话状态（带项目）
test("会话状态",
     ["--project", impress_path, "session", "status"],
     check=check_field("has_project", True))

# 4.2 撤销（跨进程调用，undo 栈为空，预期失败）
test("撤销操作（跨进程无历史）",
     ["--project", impress_path, "session", "undo"],
     expect_success=False)

# 4.3 重做（同理，预期失败）
test("重做操作（跨进程无历史）",
     ["--project", impress_path, "session", "redo"],
     expect_success=False)

# 4.4 历史
test("查看历史",
     ["--project", impress_path, "session", "history"],
     check=lambda d: f"{len(d)} 条记录" if isinstance(d, list) else "!非列表")


# ══════════════════════════════════════════════════════════════
# 5. 样式管理测试
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  🎨 样式管理测试")
print("=" * 60)

test("创建样式[标题样式]",
     ["--project", project_path, "style", "create", "标题样式",
      "--family", "paragraph", "--prop", "font_size=18pt", "--prop", "bold=true"])

test("列出样式",
     ["--project", project_path, "style", "list"],
     check=lambda d: f"{len(d)} 个样式" if isinstance(d, list) else "!非列表")

test("应用样式到内容 0",
     ["--project", project_path, "style", "apply", "标题样式", "0"])

test("删除样式",
     ["--project", project_path, "style", "remove", "标题样式"])


# ══════════════════════════════════════════════════════════════
# 6. 错误处理测试
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  ⚠️  错误处理测试")
print("=" * 60)

test("打开不存在的文件（应失败）",
     ["document", "open", "nonexistent.json"],
     expect_success=False)

test("对错误类型执行操作（应失败）",
     ["--project", calc_path, "writer", "add-paragraph", "-t", "error"],
     expect_success=False)


# ══════════════════════════════════════════════════════════════
# 汇总报告
# ══════════════════════════════════════════════════════════════
print("\n" + "=" * 60)
print("  📋 测试报告汇总")
print("=" * 60)
print(f"  总用例: {passed + failed}")
print(f"  通过:   {passed}")
print(f"  失败:   {failed}")
print(f"  通过率: {passed / (passed + failed) * 100:.1f}%")

# 列出失败项
if failed > 0:
    print("\n  失败详情:")
    for r in results:
        if r["status"] == "FAIL" or r["detail"].startswith("!"):
            print(f"    ❌ {r['name']}: {r['detail']}")

# 生成文件列表
print("\n  生成的文件:")
generated_files = []
for f in ["e2e_writer.docx", "e2e_writer.pdf", "e2e_calc.xlsx", "e2e_impress.pptx"]:
    fpath = os.path.join(WORKDIR, f)
    if os.path.exists(fpath):
        size = os.path.getsize(fpath)
        generated_files.append(f"    ✅ {f} ({size:,} bytes)")
    else:
        generated_files.append(f"    ❌ {f} (不存在)")
print("\n".join(generated_files))

print("\n" + "=" * 60)
print("  测试完成！")
print("=" * 60)
