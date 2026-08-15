//! Minimal OOXML worksheet reader: shared strings, inline strings, plain
//! values, and sheet resolution by display name → CSV text.

use crate::core::error::AppResult;

/// Read an `.xlsx` sheet as CSV text (named sheet; first sheet by default).
pub(super) fn read_xlsx(path: &std::path::Path, sheet: Option<&str>) -> AppResult<String> {
    let file = std::fs::File::open(path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid xlsx package: {e}"))?;

    // Shared strings (sharedStrings.xml) — a flat list of cell texts.
    let mut shared: Vec<String> = Vec::new();
    if let Ok(mut entry) = archive.by_name("xl/sharedStrings.xml") {
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut entry, &mut xml)?;
        let mut in_si = false;
        let mut in_t = false;
        let mut current = String::new();
        let mut chars = xml.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '<' => {
                    let tag: String = chars.by_ref().take_while(|&c| c != '>').collect();
                    let lower = tag.to_ascii_lowercase();
                    if lower.starts_with("/si") {
                        in_si = false;
                        shared.push(std::mem::take(&mut current));
                    } else if lower.starts_with("si") && !lower.starts_with("si/") {
                        in_si = true;
                    } else if lower.starts_with("t")
                        && !lower.starts_with("tr")
                        && !lower.starts_with("tn")
                        && !lower.starts_with("text")
                        && in_si
                    {
                        in_t = true;
                    } else if lower.starts_with("/t") && in_si {
                        in_t = false;
                    }
                }
                c if in_t => current.push(c),
                _ => {}
            }
        }
        if !current.is_empty() {
            shared.push(current);
        }
    }

    let part = resolve_sheet_part(&mut archive, sheet)?;

    let mut sheet_xml = String::new();
    let mut entry = archive
        .by_name(&part)
        .map_err(|e| format!("xlsx has no {part}: {e}"))?;
    std::io::Read::read_to_string(&mut entry, &mut sheet_xml)?;

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell_value = String::new();
    let mut in_value = false;
    let mut is_shared = false;
    let mut is_inline = false;
    let mut in_inline = false;
    let mut in_inline_t = false;
    let mut inline_text = String::new();
    let chars: Vec<char> = sheet_xml.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            let tag_start = i + 1;
            while i < chars.len() && chars[i] != '>' {
                i += 1;
            }
            let tag: String = chars[tag_start..i].iter().collect();
            i += 1; // skip '>'
            let lower = tag.to_ascii_lowercase();
            if lower.starts_with("/row") {
                if is_inline && !inline_text.is_empty() {
                    current_row.push(std::mem::take(&mut inline_text));
                }
                rows.push(std::mem::take(&mut current_row));
                is_inline = false;
            } else if lower.starts_with("c ") || lower == "c" {
                // New cell — flush a pending inline string first.
                if is_inline && !inline_text.is_empty() {
                    current_row.push(std::mem::take(&mut inline_text));
                }
                is_shared = tag.contains(" t=\"s\"");
                is_inline = tag.contains(" t=\"inlineStr\"");
                current_cell_value.clear();
            } else if lower == "is" {
                in_inline = true;
            } else if lower.starts_with("/is") {
                in_inline = false;
                in_inline_t = false;
            } else if lower == "t" && in_inline {
                in_inline_t = true;
            } else if lower.starts_with("/t") && in_inline {
                in_inline_t = false;
            } else if lower == "v" {
                in_value = true;
            } else if lower.starts_with("/v") {
                in_value = false;
                let raw = std::mem::take(&mut current_cell_value);
                if is_shared {
                    let idx: usize = raw.trim().parse().unwrap_or(0);
                    current_row.push(shared.get(idx).cloned().unwrap_or_default());
                } else {
                    current_row.push(raw.trim().to_string());
                }
            }
            continue;
        }
        if in_value {
            current_cell_value.push(c);
        } else if in_inline_t {
            inline_text.push(c);
        }
        i += 1;
    }
    if !current_row.is_empty() {
        if is_inline && !inline_text.is_empty() {
            current_row.push(inline_text);
        }
        rows.push(current_row);
    }

    // Pad rows to the widest row. OOXML omits empty cells entirely, so
    // a sheet grid must be restored before CSV serialization — the csv
    // crate rejects records of differing lengths unless `flexible` is on.
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize(width, String::new());
    }

    // Serialize via the csv crate so cells containing commas/quotes
    // (e.g. "Zhang, San") are escaped instead of being split into
    // spurious columns on the downstream re-parse.
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(vec![]);
    for row in rows {
        wtr.write_record(&row)
            .map_err(|e| format!("CSV write error: {e}"))?;
    }
    let out = String::from_utf8(wtr.into_inner().map_err(|e| format!("CSV error: {e}"))?)
        .map_err(|e| format!("CSV not UTF-8: {e}"))?;
    if out.trim().is_empty() {
        return Err(format!(
            "No table data found in sheet '{}'",
            sheet.unwrap_or("first")
        )
        .into());
    }
    Ok(out)
}

/// Resolve the worksheet part path for `sheet` (display name) — or the
/// first sheet when `sheet` is None. Falls back to `xl/worksheets/sheet1.xml`
/// when the workbook part is missing (minimal packages, e.g. test fixtures).
fn resolve_sheet_part(
    archive: &mut zip::ZipArchive<std::fs::File>,
    sheet: Option<&str>,
) -> AppResult<String> {
    let sheets = list_sheets(archive)?;
    if let Some(name) = sheet {
        if let Some((_, part)) = sheets.iter().find(|(n, _)| n == name) {
            return Ok(part.clone());
        }
        let names: Vec<&str> = sheets.iter().map(|(n, _)| n.as_str()).collect();
        if names.is_empty() {
            return Err(format!("Workbook has no sheets (sheet '{name}' requested)").into());
        }
        return Err(format!("Sheet '{name}' not found. Available: {}", names.join(", ")).into());
    }
    if let Some((_, part)) = sheets.first() {
        return Ok(part.clone());
    }
    Ok("xl/worksheets/sheet1.xml".to_string())
}

/// Parse workbook.xml + workbook.xml.rels into (display name, part path).
fn list_sheets(archive: &mut zip::ZipArchive<std::fs::File>) -> AppResult<Vec<(String, String)>> {
    let mut sheets: Vec<(String, String)> = Vec::new();
    let mut rid_to_target: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    if let Ok(mut entry) = archive.by_name("xl/_rels/workbook.xml.rels") {
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut entry, &mut xml)?;
        for part in xml.split("<Relationship ") {
            let rel = part.split('>').next().unwrap_or("");
            if let (Some(id), Some(target)) = (attr_val(rel, "Id"), attr_val(rel, "Target")) {
                rid_to_target.insert(id, target);
            }
        }
    }

    if let Ok(mut entry) = archive.by_name("xl/workbook.xml") {
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut entry, &mut xml)?;
        let chars: Vec<char> = xml.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] != '<' {
                i += 1;
                continue;
            }
            let tag_start = i + 1;
            while i < chars.len() && chars[i] != '>' {
                i += 1;
            }
            let tag: String = chars[tag_start..i].iter().collect();
            i += 1;
            if tag.starts_with("sheet ") || tag.starts_with("sheet\t") {
                if let (Some(name), Some(rid)) = (attr_val(&tag, "name"), attr_val(&tag, "r:id")) {
                    if let Some(target) = rid_to_target.get(&rid) {
                        let part = if target.starts_with('/') {
                            format!("xl{target}")
                        } else {
                            format!("xl/{target}")
                        };
                        sheets.push((name, part));
                    }
                }
            }
        }
    }
    Ok(sheets)
}

/// Read the value of a double-quoted attribute (`name="value"`).
fn attr_val(tag: &str, name: &str) -> Option<String> {
    for needle in [format!("{name}=\""), format!("{name} = \"")] {
        if let Some(pos) = tag.find(&needle) {
            let rest = &tag[pos + needle.len()..];
            let end = rest.find('"')?;
            return Some(rest[..end].to_string());
        }
    }
    None
}
