"""WPS CLI - 样式管理。"""


from typing import Dict, Any, Optional, List


def create_style(
    project: Dict[str, Any],
    name: str,
    family: str = "paragraph",
    parent: Optional[str] = None,
    properties: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """创建新样式。"""
    if "styles" not in project:
        project["styles"] = {}

    if name in project["styles"]:
        raise ValueError(f"样式 '{name}' 已存在。")

    style = {
        "name": name,
        "family": family,
        "parent": parent,
        "properties": properties or {},
    }
    project["styles"][name] = style
    return style


def modify_style(
    project: Dict[str, Any],
    name: str,
    properties: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """修改已有样式。"""
    if "styles" not in project or name not in project["styles"]:
        raise ValueError(f"样式 '{name}' 不存在。")

    style = project["styles"][name]
    if properties:
        style["properties"].update(properties)
    return style


def list_styles(project: Dict[str, Any]) -> List[Dict[str, Any]]:
    """列出所有样式。"""
    styles = project.get("styles", {})
    return list(styles.values())


def apply_style(
    project: Dict[str, Any],
    style_name: str,
    content_index: int,
) -> Dict[str, Any]:
    """将样式应用于 Writer 内容项。"""
    if "styles" not in project or style_name not in project["styles"]:
        raise ValueError(f"样式 '{style_name}' 不存在。")

    if project.get("type") != "writer":
        raise ValueError("样式仅可应用于 Writer 文档。")

    content = project.get("content", [])
    if content_index < 0 or content_index >= len(content):
        raise IndexError(f"内容索引超出范围: {content_index}")

    content[content_index]["style_name"] = style_name
    return {"style": style_name, "content_index": content_index}


def remove_style(project: Dict[str, Any], name: str) -> Dict[str, Any]:
    """删除样式。"""
    if "styles" not in project or name not in project["styles"]:
        raise ValueError(f"样式 '{name}' 不存在。")

    removed = project["styles"].pop(name)
    return removed
