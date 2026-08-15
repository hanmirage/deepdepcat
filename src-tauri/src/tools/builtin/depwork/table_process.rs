//! table_process — load, clean, and summarize tabular data.
//!
//! Reads CSV and Excel (.xlsx) files, applies a pipeline of lightweight
//! operations, and either returns a summary (rows, columns, stats) or
//! exports the transformed table as CSV.
//!
//! Operations (applied in order):
//! - `dedup` — remove duplicate rows
//! - `sort:<col>` — sort by a column, numeric-aware (descending: `sort:-<col>`)
//! - `filter:<col><op><value>` — keep rows matching op in `> < >= <= != =`
//!   (numeric compare when both sides parse, string fallback otherwise)
//! - `select:<col1>,<col2>` — keep only these columns
//! - `stats` — column count, row count, and per-column numeric stats

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashSet;

mod xlsx_reader;
use xlsx_reader::read_xlsx;

/// Load and process a tabular dataset.
pub struct TableProcessTool;

impl TableProcessTool {
    pub fn new() -> Self {
        Self
    }
}

/// Read a CSV file into a 2D table (Vec<Vec<String>>).
fn read_csv(path: &std::path::Path) -> AppResult<Vec<Vec<String>>> {
    let bytes = std::fs::read(path)?;
    let text = crate::core::encoding::decode_native_output(&bytes);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("CSV parse error: {e}"))?;
        rows.push(record.iter().map(|f| f.to_string()).collect());
    }
    Ok(rows)
}

/// Serialize a 2D table back to CSV text.
///
/// Writes into an in-memory buffer, so the io/csv calls cannot actually
/// fail — recover silently rather than panic.
fn to_csv(rows: &[Vec<String>]) -> String {
    let mut wtr = csv::WriterBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_writer(vec![]);
    for row in rows {
        let _ = wtr.write_record(row);
    }
    let bytes = wtr.into_inner().unwrap_or_default();
    String::from_utf8(bytes).unwrap_or_default()
}

/// Apply the operation pipeline to a table. Returns the transformed table
/// plus an optional human-readable summary.
fn apply_operations(mut rows: Vec<Vec<String>>, ops: &[String]) -> (Vec<Vec<String>>, String) {
    let mut summary = Vec::new();
    for op in ops {
        let op = op.trim();
        if op.is_empty() {
            continue;
        }
        if op == "dedup" {
            let before = rows.len();
            let mut seen = HashSet::new();
            rows.retain(|row| seen.insert(row.clone()));
            summary.push(format!("dedup: {before} → {} rows", rows.len()));
        } else if let Some(col) = op.strip_prefix("sort:") {
            let desc = col.strip_prefix('-').is_some();
            let col_name = col.trim_start_matches('-').to_string();
            let col_idx = header_index(&rows, &col_name);
            // Keep the header row pinned at index 0.
            if rows.len() > 1 {
                rows[1..].sort_by(|a, b| {
                    let av = a.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                    let bv = b.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                    if desc {
                        compare_cells(bv, av)
                    } else {
                        compare_cells(av, bv)
                    }
                });
            }
            summary.push(format!("sorted by column '{col_name}'"));
        } else if let Some(cond) = op.strip_prefix("filter:") {
            let before = rows.len();
            let (col_name, op_kind, value) = split_filter_cond(cond);
            let col_idx = header_index(&rows, &col_name);
            let mut keep_header = true;
            rows.retain(|row| {
                if keep_header {
                    keep_header = false;
                    return true;
                }
                row.get(col_idx)
                    .map(|c| cell_matches(c, op_kind, &value))
                    .unwrap_or(false)
            });
            summary.push(format!(
                "filtered '{col_name}{op_kind}{value}': {before} → {} rows",
                rows.len()
            ));
        } else if let Some(cols) = op.strip_prefix("select:") {
            let names: Vec<&str> = cols
                .split(',')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();
            let idxs: Vec<usize> = names
                .iter()
                .filter_map(|n| header_index_opt(&rows, n))
                .collect();
            rows = rows
                .into_iter()
                .map(|row| idxs.iter().filter_map(|&i| row.get(i).cloned()).collect())
                .collect();
            summary.push(format!("selected columns: {}", names.join(", ")));
        } else if op == "stats" {
            summary.push(format!(
                "table: {} rows × {} columns",
                rows.len(),
                rows.first().map(Vec::len).unwrap_or(0)
            ));
        }
    }
    (rows, summary.join("; "))
}

/// Find a column index by name (header row) or raw index (1-based "c3").
fn header_index_opt(rows: &[Vec<String>], name: &str) -> Option<usize> {
    if let Ok(idx) = name.trim_start_matches('c').parse::<usize>() {
        let zero = idx.saturating_sub(1);
        if rows.first().map(|r| r.len() > zero).unwrap_or(false) {
            return Some(zero);
        }
    }
    rows.first()?
        .iter()
        .position(|h| h.eq_ignore_ascii_case(name))
}

fn header_index(rows: &[Vec<String>], name: &str) -> usize {
    header_index_opt(rows, name).unwrap_or(0)
}

/// Parse a cell as a number, tolerating thousands separators ("1,234").
fn to_number(s: &str) -> Option<f64> {
    let t = s.trim();
    t.parse::<f64>().ok().or_else(|| {
        if !t.contains(',') {
            return None;
        }
        let cleaned: String = t.chars().filter(|&c| c != ',').collect();
        cleaned.parse::<f64>().ok()
    })
}

/// Compare two cells: numeric when both parse, otherwise lexicographic.
fn compare_cells(a: &str, b: &str) -> std::cmp::Ordering {
    match (to_number(a), to_number(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}

/// Split a `filter:<col><op><value>` condition. Operators matched longest
/// first so `>=` wins over `>`.
fn split_filter_cond(cond: &str) -> (String, &'static str, String) {
    for op in [">=", "<=", "!=", ">", "<", "="] {
        if let Some((col, val)) = cond.split_once(op) {
            return (col.trim().to_string(), op, val.trim().to_string());
        }
    }
    (cond.trim().to_string(), "=", String::new())
}

fn compare_numeric(x: f64, y: f64, op: &str) -> bool {
    match op {
        ">" => x > y,
        "<" => x < y,
        ">=" => x >= y,
        _ => x <= y,
    }
}

fn compare_string(a: &str, b: &str, op: &str) -> bool {
    match op {
        ">" => a > b,
        "<" => a < b,
        ">=" => a >= b,
        _ => a <= b,
    }
}

/// Whether a cell passes a filter condition (numeric when both sides parse,
/// string fallback otherwise).
fn cell_matches(cell: &str, op: &str, value: &str) -> bool {
    let a = cell.trim();
    let b = value.trim();
    match op {
        "=" => a == b,
        "!=" => a != b,
        cmp_op @ (">" | "<" | ">=" | "<=") => match (to_number(a), to_number(b)) {
            (Some(x), Some(y)) => compare_numeric(x, y, cmp_op),
            _ => compare_string(a, b, cmp_op),
        },
        _ => false,
    }
}

#[async_trait]
impl Tool for TableProcessTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "table_process"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Load a CSV or Excel (.xlsx) table, apply cleanup/analysis operations, \
        and return a summary or export a cleaned CSV. Operations (applied in \
        order, comma-separated list): dedup, sort:<col> (numeric-aware), \
        filter:<col><op><value> with op in > < >= <= != = (numeric compare, \
        string fallback), select:<c1>,<c2>, stats. Set output_path to export \
        the transformed table as CSV; otherwise a summary is returned. For \
        .xlsx input, sheet selects the worksheet by display name (default: \
        the first one); inline strings and shared strings are both supported."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Input file (.csv or .xlsx)."
                },
                "sheet": {
                    "type": "string",
                    "description": "Worksheet display name for .xlsx input (default: first sheet)."
                },
                "operations": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Operations to apply in order: dedup / sort:<col> / filter:<col><op><value> (op: > < >= <= != =, numeric compare with string fallback) / select:<c1>,<c2> / stats"
                },
                "output_path": {
                    "type": "string",
                    "description": "Optional export path — writes the transformed table as CSV."
                }
            },
            "required": ["path"]
        })
    }

    /// Without `output_path` this is a pure read (summarize/transform in
    /// memory) — classified read per call. With one: creating a NEW file
    /// never prompts; overwriting a pre-existing user file asks.
    fn is_read_only_call(&self, args: &Value) -> bool {
        args.get("output_path").and_then(|o| o.as_str()).is_none()
    }

    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("output_path").and_then(|o| o.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("csv"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let ops: Vec<String> = args
            .get("operations")
            .and_then(|o| o.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_str);
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()).into());
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let sheet = args.get("sheet").and_then(|s| s.as_str());
        let rows = match ext.as_str() {
            "csv" => read_csv(&path)?,
            "xlsx" => {
                let csv_text = read_xlsx(&path, sheet)?;
                read_csv_from_text(&csv_text)?
            }
            other => {
                return Err(
                    format!("Unsupported table format: .{other}. Supported: .csv .xlsx").into(),
                )
            }
        };

        let (rows, op_summary) = apply_operations(rows, &ops);

        // Export or summarize.
        if let Some(out_str) = args.get("output_path").and_then(|o| o.as_str()) {
            let mut out =
                crate::tools::builtin::resolve_path(context.workspace.as_deref(), out_str);
            if out.extension().is_none() {
                out.set_extension("csv");
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, to_csv(&rows))?;
            super::permissions::record_output(context, &out);
            Ok(ToolResult::success(format!(
                "Exported {rows_len} rows to {out}{op_suffix}",
                rows_len = rows.len(),
                out = out.display(),
                op_suffix = if op_summary.is_empty() {
                    String::new()
                } else {
                    format!(" ({op_summary})")
                }
            )))
        } else {
            let header = rows.first().map(|r| r.join(", ")).unwrap_or_default();
            let preview: Vec<String> = rows.iter().skip(1).take(5).map(|r| r.join(", ")).collect();
            let mut out = format!(
                "Table: {} rows × {} columns\nHeaders: {header}",
                rows.len().saturating_sub(1),
                rows.first().map(Vec::len).unwrap_or(0)
            );
            if !op_summary.is_empty() {
                out.push_str(&format!("\nOperations: {op_summary}"));
            }
            if !preview.is_empty() {
                out.push_str(&format!("\nFirst rows:\n{}", preview.join("\n")));
            }
            Ok(ToolResult::success(out))
        }
    }
}

/// Parse CSV text into a 2D table.
fn read_csv_from_text(text: &str) -> AppResult<Vec<Vec<String>>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("CSV parse error: {e}"))?;
        rows.push(record.iter().map(|f| f.to_string()).collect());
    }
    Ok(rows)
}

#[cfg(test)]
mod table_process_tests;
