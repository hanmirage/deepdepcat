#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""实时演示：打开 WPS 窗口，agent 逐字输入内容，肉眼可见。"""

import sys
import os
import time
import random

sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")

# COM 常量
wdFormatDocumentDefault = 16
wdPageBreak = 7
wdCollapseEnd = 0
wdAlignParagraphCenter = 1


def typewriter(rng, text, delay=0.05):
    """模拟打字机效果：逐字写入。"""
    for ch in text:
        rng.InsertAfter(ch)
        rng.Collapse(wdCollapseEnd)
        time.sleep(delay + random.uniform(0, 0.03))


def main():
    import win32com.client
    import pythoncom

    pythoncom.CoInitialize()

    print("启动 WPS Writer（可见窗口）...")
    app = win32com.client.Dispatch("KWPS.Application")
    app.Visible = True
    time.sleep(1)

    # 新建文档
    print("创建新文档...")
    doc = app.Documents.Add()
    time.sleep(0.5)

    # ── 1. 大标题 ──
    print("\n[1/8] 写入大标题...")
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    rng.ParagraphFormat.Alignment = wdAlignParagraphCenter
    rng.Font.Size = 28
    rng.Font.Bold = True
    typewriter(rng, "2024年度AI工作总结", delay=0.08)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)
    time.sleep(0.5)

    # ── 2. 副标题 ──
    print("[2/8] 写入副标题...")
    rng.Font.Size = 14
    rng.Font.Bold = False
    rng.ParagraphFormat.Alignment = wdAlignParagraphCenter
    rng.Font.Color = 0x808080  # 灰色
    typewriter(rng, "—— 由 AI Agent 自动生成 ——", delay=0.06)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)
    rng.Font.Color = 0x000000  # 恢复黑色
    time.sleep(0.5)

    # ── 3. 正文段落 ──
    print("[3/8] 写入正文段落...")
    rng.Font.Size = 12
    rng.Font.Bold = False
    rng.ParagraphFormat.Alignment = 0  # 左对齐
    rng.ParagraphFormat.FirstLineIndent = 24  # 首行缩进2字符
    paragraph_text = (
        "本报告由AI Agent自动撰写。在过去的一年中，"
        "我们的业务取得了显著增长，月活跃用户突破1200万，"
        "营收同比增长35%，客户满意度达到96%。"
        "这些成绩离不开团队的共同努力和技术创新。"
    )
    typewriter(rng, paragraph_text, delay=0.03)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)
    rng.ParagraphFormat.FirstLineIndent = 0
    time.sleep(0.5)

    # ── 4. H2 标题 ──
    print("[4/8] 写入二级标题...")
    rng.Font.Size = 18
    rng.Font.Bold = True
    rng.ParagraphFormat.FirstLineIndent = 0
    typewriter(rng, "关键指标", delay=0.06)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)
    time.sleep(0.3)

    # ── 5. 列表项 ──
    print("[5/8] 写入列表项...")
    rng.Font.Size = 12
    rng.Font.Bold = False
    list_items = [
        "月活跃用户：1200万，同比增长40%",
        "年度营收：3.5亿元，同比增长35%",
        "客户满意度：96%，提升5个百分点",
        "新增功能：48项，覆盖核心业务场景",
    ]
    for i, item in enumerate(list_items):
        prefix = f"  {i+1}. "
        typewriter(rng, prefix + item, delay=0.03)
        rng.InsertParagraphAfter()
        rng.Collapse(wdCollapseEnd)
        time.sleep(0.2)

    time.sleep(0.5)

    # ── 6. H2 标题 ──
    print("[6/8] 写入表格标题...")
    rng.Font.Size = 18
    rng.Font.Bold = True
    typewriter(rng, "季度数据对比", delay=0.06)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)
    time.sleep(0.3)

    # ── 7. 表格 ──
    print("[7/8] 插入表格并逐格填写...")
    rng.Font.Size = 12
    rng.Font.Bold = False
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)

    table_data = [
        ["季度", "用户数", "营收", "满意度"],
        ["Q1", "800万", "0.7亿", "91%"],
        ["Q2", "950万", "0.8亿", "93%"],
        ["Q3", "1100万", "0.9亿", "94%"],
        ["Q4", "1200万", "1.1亿", "96%"],
    ]

    rows = len(table_data)
    cols = len(table_data[0])
    table = doc.Tables.Add(rng, rows, cols)
    table.AutoFitBehavior(2)

    for ri in range(rows):
        for ci in range(cols):
            cell = table.Cell(ri + 1, ci + 1)
            val = table_data[ri][ci]
            # 表格单元格逐字写入：用 Selection 逐字输入
            cell.Select()
            sel = app.Selection
            sel.Font.Size = 11
            if ri == 0:
                sel.Font.Bold = True
                sel.ParagraphFormat.Alignment = wdAlignParagraphCenter
            else:
                sel.Font.Bold = False
            # 先清除默认内容（表格新建时单元格可能有段落标记）
            sel.TypeText(val)  # TypeText 会替换选中内容
            time.sleep(0.04 * len(val) + 0.1)
        time.sleep(0.2)

    # 表格后空一行
    rng = doc.Range()
    rng.Collapse(wdCollapseEnd)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)
    time.sleep(0.5)

    # ── 8. 结尾段落 ──
    print("[8/8] 写入结尾段落...")
    rng.Font.Size = 12
    rng.Font.Bold = False
    rng.ParagraphFormat.FirstLineIndent = 24
    rng.ParagraphFormat.Alignment = 0
    ending = (
        "综上所述，2024年是公司高速发展的一年。"
        "展望未来，我们将继续以AI技术驱动业务创新，"
        "为用户创造更大价值。感谢每一位团队成员的辛勤付出！"
    )
    typewriter(rng, ending, delay=0.03)
    rng.InsertParagraphAfter()
    rng.Collapse(wdCollapseEnd)

    time.sleep(1)

    # 保存文件
    save_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "agent_live_demo.docx")
    if os.path.exists(save_path):
        os.remove(save_path)
    doc.SaveAs2(save_path, FileFormat=wdFormatDocumentDefault)
    print(f"\n文档已保存: {save_path}")
    print(f"文件大小: {os.path.getsize(save_path):,} bytes")

    print("\n文档已保存，WPS 窗口保持打开 5 秒供查看...")
    time.sleep(5)

    doc.Close(False)
    app.Quit()
    print("演示完成！")


if __name__ == "__main__":
    main()
