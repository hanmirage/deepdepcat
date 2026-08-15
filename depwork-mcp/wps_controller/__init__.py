"""
WPS Office COM 自动化控制库

架构：JSON 项目模型 + 延迟渲染
- 所有编辑操作只修改 JSON 项目字典（纯 Python，无需 WPS）
- 只在 export render 时才调用 WPS COM 生成真实文件
- CLI 支持 --json 输出，适合 Rust/其他语言通过 subprocess 调用
"""

__version__ = "2.0.0"
