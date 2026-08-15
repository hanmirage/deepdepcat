"""WPS CLI - Impress（演示文稿）命令实现。

所有操作只修改 JSON 项目字典，不直接操作 WPS。
真正渲染到文件由 core/export.py 完成。
"""

import copy
from typing import Dict, Any, Optional, List


def _ensure_impress(project: Dict[str, Any]) -> None:
    if project.get("type") != "impress":
        raise ValueError("当前文档不是 Impress（演示文稿）类型。")


def add_slide(
    project: Dict[str, Any],
    title: str = "",
    content: str = "",
    position: Optional[int] = None,
) -> Dict[str, Any]:
    """添加幻灯片。"""
    _ensure_impress(project)
    if "slides" not in project:
        project["slides"] = []

    slide = {"title": title, "content": content, "elements": []}
    slides = project["slides"]
    if position is not None and 0 <= position < len(slides):
        slides.insert(position, slide)
    else:
        slides.append(slide)
    return slide


def remove_slide(project: Dict[str, Any], index: int) -> Dict[str, Any]:
    """按索引删除幻灯片。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if index < 0 or index >= len(slides):
        raise IndexError(f"幻灯片索引超出范围: {index}（共 {len(slides)} 张）")
    return slides.pop(index)


def set_slide_content(
    project: Dict[str, Any],
    index: int,
    title: Optional[str] = None,
    content: Optional[str] = None,
) -> Dict[str, Any]:
    """更新幻灯片的标题和/或内容。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if index < 0 or index >= len(slides):
        raise IndexError(f"幻灯片索引超出范围: {index}（共 {len(slides)} 张）")
    slide = slides[index]
    if title is not None:
        slide["title"] = title
    if content is not None:
        slide["content"] = content
    return slide


def add_slide_element(
    project: Dict[str, Any],
    slide_index: int,
    element_type: str = "text_box",
    text: str = "",
    x: str = "2cm",
    y: str = "2cm",
    width: str = "10cm",
    height: str = "5cm",
) -> Dict[str, Any]:
    """在幻灯片中添加元素。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if slide_index < 0 or slide_index >= len(slides):
        raise IndexError(f"幻灯片索引超出范围: {slide_index}（共 {len(slides)} 张）")

    valid_types = ("text_box", "image", "shape")
    if element_type not in valid_types:
        raise ValueError(f"不支持的元素类型: {element_type}。有效值: {valid_types}")

    elem = {
        "type": element_type,
        "text": text,
        "x": x, "y": y,
        "width": width, "height": height,
    }
    slides[slide_index].setdefault("elements", []).append(elem)
    return elem


def remove_slide_element(
    project: Dict[str, Any],
    slide_index: int,
    element_index: int,
) -> Dict[str, Any]:
    """删除幻灯片中的元素。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if slide_index < 0 or slide_index >= len(slides):
        raise IndexError(f"幻灯片索引超出范围: {slide_index}")
    elements = slides[slide_index].get("elements", [])
    if element_index < 0 or element_index >= len(elements):
        raise IndexError(f"元素索引超出范围: {element_index}（共 {len(elements)} 个）")
    return elements.pop(element_index)


def move_slide(
    project: Dict[str, Any],
    from_index: int,
    to_index: int,
) -> Dict[str, Any]:
    """移动幻灯片到新位置。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if from_index < 0 or from_index >= len(slides):
        raise IndexError(f"源幻灯片索引超出范围: {from_index}")
    if to_index < 0 or to_index >= len(slides):
        raise IndexError(f"目标幻灯片索引超出范围: {to_index}")
    slide = slides.pop(from_index)
    slides.insert(to_index, slide)
    return {"moved_from": from_index, "moved_to": to_index, "slide_title": slide.get("title", "")}


def duplicate_slide(project: Dict[str, Any], index: int) -> Dict[str, Any]:
    """复制幻灯片。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if index < 0 or index >= len(slides):
        raise IndexError(f"幻灯片索引超出范围: {index}")
    new_slide = copy.deepcopy(slides[index])
    new_slide["title"] = new_slide.get("title", "") + " (副本)"
    slides.insert(index + 1, new_slide)
    return new_slide


def list_slides(project: Dict[str, Any]) -> List[Dict[str, Any]]:
    """列出所有幻灯片。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    result = []
    for i, slide in enumerate(slides):
        result.append({
            "index": i,
            "title": slide.get("title", "")[:60],
            "content_preview": slide.get("content", "")[:80],
            "element_count": len(slide.get("elements", [])),
        })
    return result


def get_slide(project: Dict[str, Any], index: int) -> Dict[str, Any]:
    """按索引获取幻灯片。"""
    _ensure_impress(project)
    slides = project.get("slides", [])
    if index < 0 or index >= len(slides):
        raise IndexError(f"幻灯片索引超出范围: {index}（共 {len(slides)} 张）")
    return slides[index]
