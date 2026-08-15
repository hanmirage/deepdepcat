//! LLM-facing parameter schema for `office_automate`.
//!
//! One shared schema covers every app family; each action documents which
//! fields it consumes. Kept in its own module so the tool file stays small.
//! The schema is a static JSON string parsed once (avoids json! macro
//! recursion limits on large documents).

use serde_json::Value;
use std::sync::OnceLock;

static SCHEMA_JSON: &str = r##"{
  "type": "object",
  "properties": {
    "action": {
      "type": "string",
      "enum": [
        "detect", "read", "read_paragraphs", "replace", "insert", "delete",
        "type_text", "replace_all", "set_style", "set_font", "add_paragraph",
        "add_heading", "add_list", "add_table", "add_image", "page_break",
        "set_alignment", "set_line_spacing", "set_paragraph_format", "clear_doc",
        "read_cells", "read_cell", "list_sheets", "write_cell", "write_range",
        "set_formula", "merge_cells", "unmerge_cells", "clear_range",
        "add_sheet", "rename_sheet", "remove_sheet", "set_column_width",
        "set_row_height", "set_cell_style", "read_slides", "add_slide",
        "remove_slide", "set_slide_content", "add_textbox", "add_shape",
        "set_slide_bg", "save_as", "export_pdf"
      ],
      "description": "Operation to perform."
    },
    "app": {
      "type": "string",
      "enum": ["auto", "writer", "calc", "impress", "word", "wps"],
      "description": "App family (default auto: inferred from the file extension; writer/word/wps all drive the Word-compatible object model)."
    },
    "path": {
      "type": "string",
      "description": "Document path (all actions except detect). OMIT or use \"active\" for writer actions to target the user's CURRENTLY OPEN document (ActiveDocument) — the agent writes into whatever the user is looking at."
    },
    "output_path": {
      "type": "string",
      "description": "Target path for save_as / export_pdf."
    },
    "sheet": {
      "type": "integer",
      "description": "1-based sheet index (calc actions, default 1). Prefer sheet_name — indexes shift when sheets are added/removed."
    },
    "sheet_name": {
      "type": "string",
      "description": "Worksheet name (calc actions). Stable across add/remove — preferred over sheet index. Example: \"调研数据\"."
    },
    "position": {
      "type": "integer",
      "description": "1-based paragraph index to insert BEFORE (writer add_paragraph/add_heading/add_list/add_table/add_image/page_break). Omit to append at the end."
    },
    "index": {
      "type": "integer",
      "description": "1-based slide position (impress add_slide/remove_slide/set_slide_content/add_textbox/add_shape/set_slide_bg)."
    },
    "para": {
      "type": "integer",
      "description": "1-based paragraph index (writer replace/insert/delete/set_style/set_font/set_alignment/set_line_spacing/set_paragraph_format; type_text anchor)."
    },
    "text": {
      "type": "string",
      "description": "Text: writer replace/insert/type_text/add_paragraph/add_heading, replace_all replacement, calc write_cell, impress add_textbox/add_shape content."
    },
    "find": {
      "type": "string",
      "description": "Text to find (writer replace_all)."
    },
    "level": {
      "type": "integer",
      "description": "Heading level 1-6 (writer add_heading, default 1)."
    },
    "items": {
      "type": "array",
      "items": { "type": "string" },
      "description": "List items (writer add_list)."
    },
    "list_style": {
      "type": "string",
      "enum": ["bullet", "number"],
      "description": "List numbering style (writer add_list, default bullet)."
    },
    "data": {
      "type": "array",
      "items": {
        "type": "array",
        "items": { "type": "string" }
      },
      "description": "2D array of cell values (writer add_table rows; calc write_range starting at range_ref)."
    },
    "rows": {
      "type": "integer",
      "description": "Table row count when data is omitted (writer add_table)."
    },
    "cols": {
      "type": "integer",
      "description": "Table column count when data is omitted (writer add_table)."
    },
    "header": {
      "type": "boolean",
      "description": "Style the first table row bold + shaded (writer add_table, default true)."
    },
    "header_color": {
      "type": "integer",
      "description": "Header row shading as 0xRRGGBB (writer add_table, default #F2F2F2)."
    },
    "image_path": {
      "type": "string",
      "description": "Image file path (writer add_image; impress add_image with index + optional x/y/width/height in points)."
    },
    "width_cm": {
      "type": "number",
      "description": "Image width in centimeters (writer add_image, optional; aspect ratio locked when only one dimension is set)."
    },
    "height_cm": {
      "type": "number",
      "description": "Image height in centimeters (writer add_image, optional)."
    },
    "align": {
      "type": "string",
      "enum": ["left", "center", "right", "justify"],
      "description": "Horizontal alignment (writer set_alignment paragraph; calc set_cell_style cell)."
    },
    "multiple": {
      "type": "number",
      "description": "Line spacing multiple, e.g. 1.0 / 1.5 / 2.0 (writer set_line_spacing)."
    },
    "space_before": {
      "type": "number",
      "description": "Space before paragraph in points (writer set_paragraph_format)."
    },
    "space_after": {
      "type": "number",
      "description": "Space after paragraph in points (writer set_paragraph_format)."
    },
    "first_line_indent": {
      "type": "number",
      "description": "First line indent in points (writer set_paragraph_format)."
    },
    "left_indent": {
      "type": "number",
      "description": "Left indent in points (writer set_paragraph_format)."
    },
    "style": {
      "type": "string",
      "description": "Paragraph style name (writer set_style, e.g. 'Heading 1', 'Title', 'Normal')."
    },
    "size": {
      "type": "integer",
      "description": "Font size in points (writer set_font/add_paragraph; impress add_textbox/add_shape; calc set_cell_style font_size)."
    },
    "bold": {
      "type": "boolean",
      "description": "Bold on/off (writer set_font/add_paragraph; impress add_textbox; calc set_cell_style)."
    },
    "italic": {
      "type": "boolean",
      "description": "Italic on/off (writer set_font/add_paragraph; calc set_cell_style)."
    },
    "underline": {
      "type": "boolean",
      "description": "Underline on/off (writer set_font/add_paragraph)."
    },
    "font_name": {
      "type": "string",
      "description": "Font family name, e.g. 'Microsoft YaHei' (writer set_font/add_paragraph)."
    },
    "color": {
      "type": "integer",
      "description": "Font color as 0xRRGGBB (writer set_font/add_paragraph; impress set_slide_bg background color; calc set_cell_style font_color)."
    },
    "from": {
      "type": "integer",
      "description": "1-based first paragraph to read (writer read_paragraphs)."
    },
    "to": {
      "type": "integer",
      "description": "1-based last paragraph to read (writer read_paragraphs)."
    },
    "pace": {
      "type": "integer",
      "description": "Milliseconds between type_text chunks (default 180; larger = slower typing)."
    },
    "chunk": {
      "type": "integer",
      "description": "Characters per type_text chunk (default 4; smaller = slower typing)."
    },
    "row": {
      "type": "integer",
      "description": "1-based row (calc write_cell/set_formula/set_cell_style/read_cell; set_row_height)."
    },
    "col": {
      "type": "integer",
      "description": "1-based column (calc write_cell/set_formula/set_cell_style/read_cell). set_column_width accepts letters (\"A\") or number."
    },
    "range_ref": {
      "type": "string",
      "description": "A1-style range reference (calc write_range start e.g. \"A1\"; merge_cells/unmerge_cells/clear_range full range e.g. \"A1:C5\")."
    },
    "formula": {
      "type": "string",
      "description": "Excel formula, e.g. \"=SUM(A1:A10)\" (calc set_formula)."
    },
    "name": {
      "type": "string",
      "description": "New sheet name (calc add_sheet/rename_sheet; auto-suffixed when taken)."
    },
    "width": {
      "type": "number",
      "description": "Column width in characters (calc set_column_width) OR textbox/shape width in points (impress add_textbox/add_shape)."
    },
    "height": {
      "type": "number",
      "description": "Row height in points (calc set_row_height) OR textbox/shape height in points (impress add_textbox/add_shape)."
    },
    "font_size": {
      "type": "number",
      "description": "Font size in points (calc set_cell_style; impress add_textbox/add_shape)."
    },
    "font_color": {
      "type": "integer",
      "description": "Font color as 0xRRGGBB (calc set_cell_style; impress add_textbox)."
    },
    "bg_color": {
      "type": "integer",
      "description": "Cell background as 0xRRGGBB (calc set_cell_style)."
    },
    "wrap": {
      "type": "boolean",
      "description": "Wrap text in cell (calc set_cell_style)."
    },
    "title": {
      "type": "string",
      "description": "Slide title (impress add_slide/set_slide_content)."
    },
    "body": {
      "type": "string",
      "description": "Slide body text (impress add_slide/set_slide_content)."
    },
    "x": {
      "type": "number",
      "description": "Left position in points from slide origin (impress add_textbox/add_shape, default 50)."
    },
    "y": {
      "type": "number",
      "description": "Top position in points from slide origin (impress add_textbox/add_shape, default 50)."
    },
    "shape": {
      "type": "string",
      "enum": ["rectangle", "diamond", "rounded", "triangle", "oval", "hexagon", "heart", "arrow_right", "pentagon", "chevron", "star"],
      "description": "AutoShape kind (impress add_shape, default rectangle)."
    },
    "fill_color": {
      "type": "integer",
      "description": "Shape fill color as 0xRRGGBB (impress add_shape)."
    }
  },
  "required": ["action"]
}
"##;

static SCHEMA: OnceLock<Value> = OnceLock::new();

pub fn schema() -> Value {
    SCHEMA
        .get_or_init(|| {
            serde_json::from_str(SCHEMA_JSON).expect("static office schema is valid JSON")
        })
        .clone()
}
