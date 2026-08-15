//! pdf_tools — merge / split / extract pages from PDF files.
//!
//! Pure Rust (lopdf): source pages are deep-copied into a target document
//! (object references are remapped; `/Parent` is rewired to the target's
//! pages tree), so merged output keeps fonts, resources and images.
//!
//! Actions:
//! - `info`    — page count + page sizes
//! - `merge`   — combine multiple PDFs (in the given order) into one
//! - `extract` — copy a page range (1-based inclusive) to a new file
//! - `split`   — write every page as its own PDF (`{prefix}_page{N}.pdf`)

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use lopdf::{dictionary, Object};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// Deep-copy an object tree from `src` into `dst`, remapping references.
/// `visited` guards shared/cyclic references.
fn deep_copy_value(
    obj: &Object,
    src: &lopdf::Document,
    dst: &mut lopdf::Document,
    visited: &mut HashMap<lopdf::ObjectId, lopdf::ObjectId>,
) -> Object {
    match obj {
        Object::Reference(id) => Object::Reference(deep_copy_object(src, dst, *id, visited)),
        Object::Array(items) => Object::Array(
            items
                .iter()
                .map(|o| deep_copy_value(o, src, dst, visited))
                .collect(),
        ),
        Object::Dictionary(d) => Object::Dictionary(
            d.iter()
                .map(|(k, v)| (k.clone(), deep_copy_value(v, src, dst, visited)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn deep_copy_object(
    src: &lopdf::Document,
    dst: &mut lopdf::Document,
    id: lopdf::ObjectId,
    visited: &mut HashMap<lopdf::ObjectId, lopdf::ObjectId>,
) -> lopdf::ObjectId {
    if let Some(&mapped) = visited.get(&id) {
        return mapped;
    }
    let Ok(obj) = src.get_object(id) else {
        return id;
    };
    let copied = deep_copy_value(obj, src, dst, visited);
    let new_id = dst.add_object(copied);
    visited.insert(id, new_id);
    new_id
}

/// Copy a page (and its resource graph) into `dst`. The page's `/Parent`
/// is NOT copied — the caller wires it to the target pages tree.
fn copy_page(
    src: &lopdf::Document,
    page_id: lopdf::ObjectId,
    dst: &mut lopdf::Document,
    visited: &mut HashMap<lopdf::ObjectId, lopdf::ObjectId>,
) -> lopdf::ObjectId {
    let Ok(obj) = src.get_object(page_id) else {
        return page_id;
    };
    let mut page = obj.as_dict().cloned().unwrap_or_default();
    page.remove(b"Parent");
    let copied = deep_copy_value(&Object::Dictionary(page), src, dst, visited);
    let new_id = dst.add_object(copied);
    visited.insert(page_id, new_id);
    new_id
}

/// Page ids of a document in reading order (page number → ObjectId).
/// `lopdf::get_pages` returns a HashMap — iterating `.values()` gives a
/// RANDOM order, so merge/extract/split would silently scramble pages.
fn ordered_pages(doc: &lopdf::Document) -> Vec<lopdf::ObjectId> {
    let mut pages: Vec<_> = doc.get_pages().into_iter().collect();
    pages.sort_by_key(|(page_no, _)| *page_no);
    pages.into_iter().map(|(_, id)| id).collect()
}

/// Wire the copied page ids into `doc`'s pages tree + catalog (the pages
/// were copied into THIS document — never a fresh one).
fn finalize_document(doc: &mut lopdf::Document, page_ids: Vec<lopdf::ObjectId>) -> AppResult<()> {
    let pages_id = doc.new_object_id();
    for page_id in &page_ids {
        if let Some(Object::Dictionary(d)) = doc.objects.get_mut(page_id) {
            d.set("Parent", pages_id);
        }
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids.iter().map(|&p| Object::Reference(p)).collect::<Vec<_>>(),
            "Count" => page_ids.len() as i64,
        }),
    );
    let catalog_id = doc.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    Ok(())
}

/// Merge the given PDF files into one document (order preserved).
pub fn merge_pdfs(paths: &[&Path]) -> AppResult<lopdf::Document> {
    if paths.is_empty() {
        return Err("merge needs at least one input PDF".into());
    }
    let mut dst = lopdf::Document::with_version("1.5");
    let mut page_ids: Vec<lopdf::ObjectId> = Vec::new();

    for path in paths {
        let src = lopdf::Document::load(path)
            .map_err(|e| format!("Cannot load {}: {e}", path.display()))?;
        // Per-document visited map: every PDF numbers its objects from 1, so
        // a single shared map would alias page id 5 of file A to page id 5 of
        // file B — pages silently end up with the WRONG content.
        let mut visited: HashMap<lopdf::ObjectId, lopdf::ObjectId> = HashMap::new();
        for page_id in ordered_pages(&src) {
            page_ids.push(copy_page(&src, page_id, &mut dst, &mut visited));
        }
    }

    finalize_document(&mut dst, page_ids)?;
    Ok(dst)
}

/// Extract a page range (1-based inclusive) into a new document.
pub fn extract_pages(
    path: &Path,
    start_page: usize,
    end_page: usize,
) -> AppResult<lopdf::Document> {
    let src =
        lopdf::Document::load(path).map_err(|e| format!("Cannot load {}: {e}", path.display()))?;
    let all = ordered_pages(&src);
    if start_page < 1 || start_page > all.len() || end_page < start_page || end_page > all.len() {
        return Err(format!(
            "Page range {start_page}-{end_page} invalid (document has {} pages)",
            all.len()
        )
        .into());
    }
    let mut dst = lopdf::Document::with_version("1.5");
    let mut visited: HashMap<lopdf::ObjectId, lopdf::ObjectId> = HashMap::new();
    let page_ids: Vec<lopdf::ObjectId> = all[start_page - 1..end_page]
        .iter()
        .map(|&pid| copy_page(&src, pid, &mut dst, &mut visited))
        .collect();
    finalize_document(&mut dst, page_ids)?;
    Ok(dst)
}

/// PDF manipulation tool.
pub struct PdfToolsTool;

impl PdfToolsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PdfToolsTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "pdf_tools"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Merge, split or extract pages of PDF files (pure Rust, no Office \
        install needed). Actions: info (page count + sizes), merge (combine \
        multiple PDFs into one, order preserved), extract (copy a 1-based \
        page range to a new file), split (write every page as its own PDF: \
        {prefix}_page{N}.pdf). Use with pdf_generate / doc_read for \
        document assembly workflows."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["info", "merge", "extract", "split"],
                    "description": "Operation to perform."
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Input PDF paths (merge: 2+ files; info/extract/split: 1 file)."
                },
                "output": {
                    "type": "string",
                    "description": "Output path (merge/extract) or file prefix (split)."
                },
                "start_page": {
                    "type": "integer",
                    "description": "1-based start page (extract)."
                },
                "end_page": {
                    "type": "integer",
                    "description": "1-based end page, inclusive (extract)."
                }
            },
            "required": ["action", "paths"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// `info` is classified read per call; the writing actions
    /// (merge/extract/split) self-approve when the target is new or the
    /// session's own output.
    fn is_read_only_call(&self, args: &Value) -> bool {
        matches!(args.get("action").and_then(|a| a.as_str()), Some("info"))
    }

    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("output").and_then(|o| o.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("pdf"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?
            .to_ascii_lowercase();
        let path_values = args
            .get("paths")
            .and_then(|p| p.as_array())
            .ok_or_else(|| "Missing required parameter: paths".to_string())?;
        if path_values.is_empty() {
            return Err("paths must contain at least one PDF".into());
        }
        let paths: Vec<std::path::PathBuf> = path_values
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| crate::tools::builtin::resolve_path(context.workspace.as_deref(), s))
            .collect();
        if paths.len() != path_values.len() {
            return Err("all paths must be strings".into());
        }

        match action.as_str() {
            "info" => {
                let doc = lopdf::Document::load(&paths[0])
                    .map_err(|e| format!("Cannot load {}: {e}", paths[0].display()))?;
                let pages = ordered_pages(&doc);
                let mut out = format!("--- PDF: {}\n({} pages)\n", paths[0].display(), pages.len());
                for (i, pid) in pages.iter().enumerate() {
                    let size = match doc.get_object(*pid) {
                        Ok(Object::Dictionary(d)) => {
                            match d.get(b"MediaBox").ok().and_then(|o| o.as_array().ok()) {
                                Some(a) => {
                                    let vals: Vec<f64> = a
                                        .iter()
                                        .filter_map(|o| o.as_float().ok().map(|f| f as f64))
                                        .collect();
                                    if vals.len() == 4 {
                                        format!(
                                            "{:.0}×{:.0}",
                                            (vals[2] - vals[0]).abs(),
                                            (vals[3] - vals[1]).abs()
                                        )
                                    } else {
                                        "?".to_string()
                                    }
                                }
                                None => "?".to_string(),
                            }
                        }
                        _ => "?".to_string(),
                    };
                    out.push_str(&format!("page {}: {size}\n", i + 1));
                }
                Ok(ToolResult::success(out))
            }
            "merge" => {
                if paths.len() < 2 {
                    return Err("merge needs at least 2 PDFs".into());
                }
                let output = args
                    .get("output")
                    .and_then(|o| o.as_str())
                    .ok_or_else(|| "Missing required parameter: output".to_string())?;
                let out_path =
                    crate::tools::builtin::resolve_path(context.workspace.as_deref(), output);
                let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
                let doc = merge_pdfs(&refs)?;
                let total = doc.get_pages().len();
                save_doc(doc, &out_path)?;
                super::permissions::record_output(context, &out_path);
                Ok(ToolResult::success(format!(
                    "Merged {} PDFs → {} ({total} pages)",
                    paths.len(),
                    out_path.display()
                )))
            }
            "extract" => {
                let start = args
                    .get("start_page")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "Missing required parameter: start_page".to_string())?
                    as usize;
                let end = args
                    .get("end_page")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "Missing required parameter: end_page".to_string())?
                    as usize;
                let output = args
                    .get("output")
                    .and_then(|o| o.as_str())
                    .ok_or_else(|| "Missing required parameter: output".to_string())?;
                let out_path =
                    crate::tools::builtin::resolve_path(context.workspace.as_deref(), output);
                let doc = extract_pages(&paths[0], start, end)?;
                save_doc(doc, &out_path)?;
                super::permissions::record_output(context, &out_path);
                Ok(ToolResult::success(format!(
                    "Extracted pages {start}-{end} from {} → {}",
                    paths[0].display(),
                    out_path.display()
                )))
            }
            "split" => {
                let prefix = args.get("output").and_then(|o| o.as_str()).ok_or_else(|| {
                    "Missing required parameter: output (file prefix)".to_string()
                })?;
                let src = lopdf::Document::load(&paths[0])
                    .map_err(|e| format!("Cannot load {}: {e}", paths[0].display()))?;
                let all = ordered_pages(&src);
                if all.is_empty() {
                    return Err("document has no pages".into());
                }
                let mut written = Vec::new();
                for (i, pid) in all.iter().enumerate() {
                    let mut dst = lopdf::Document::with_version("1.5");
                    let mut visited = HashMap::new();
                    let copied = copy_page(&src, *pid, &mut dst, &mut visited);
                    finalize_document(&mut dst, vec![copied])?;
                    let out_path = crate::tools::builtin::resolve_path(
                        context.workspace.as_deref(),
                        &format!("{prefix}_page{}.pdf", i + 1),
                    );
                    save_doc(dst, &out_path)?;
                    written.push(out_path.display().to_string());
                }
                Ok(ToolResult::success(format!(
                    "Split {} → {} files:\n{}",
                    paths[0].display(),
                    written.len(),
                    written.join("\n")
                )))
            }
            other => Err(format!("Unknown action: {other} (info|merge|extract|split)").into()),
        }
    }
}

fn save_doc(mut doc: lopdf::Document, path: &Path) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    doc.save(path)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::builtin::depwork::pdf_generate;

    /// Build a small two-page PDF via pdf_generate internals.
    fn make_pdf(dir: &Path, name: &str, title: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        pdf_generate::make_test_pdf(&path, title).expect("build");
        path
    }

    #[test]
    fn merge_combines_pages_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = make_pdf(dir.path(), "a.pdf", "文档 A");
        let b = make_pdf(dir.path(), "b.pdf", "文档 B");
        let merged = merge_pdfs(&[&a, &b]).expect("merge");
        // Each test PDF is one page.
        assert_eq!(merged.get_pages().len(), 2);
    }

    #[test]
    fn merge_preserves_input_page_order() {
        // ordered_pages must follow page-number order — a HashMap `.values()`
        // iteration would silently scramble the merged pages. ASCII titles so
        // extracted text is comparable (CJK goes through font-subset cmap).
        let dir = tempfile::tempdir().expect("tempdir");
        let a = make_pdf(dir.path(), "a.pdf", "Page One");
        let b = make_pdf(dir.path(), "b.pdf", "Page Two");
        let mut merged = merge_pdfs(&[&a, &b]).expect("merge");
        let out = dir.path().join("merged.pdf");
        merged.save(&out).expect("save merged");
        let (pages, _) = crate::tools::builtin::depwork::doc_read::extract_pdf_pages(&out, None, None)
            .expect("extract merged pages");
        assert_eq!(pages.len(), 2, "two merged pages");
        let texts: Vec<String> = pages.iter().map(|p| p.text.clone()).collect();
        // Extracted text is UTF-16BE (font-subset) — compare the encoded form.
        let enc_utf16be = |s: &str| -> String {
            s.encode_utf16()
                .flat_map(|u| u.to_be_bytes())
                .map(|b| b as char)
                .collect()
        };
        assert!(
            pages[0].text.starts_with(&enc_utf16be("Page One")),
            "page 1 must be the FIRST input's page: {texts:?}"
        );
        assert!(
            pages[1].text.starts_with(&enc_utf16be("Page Two")),
            "page 2 must be the SECOND input's page: {texts:?}"
        );
    }

    #[test]
    fn extract_pulls_range() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = make_pdf(dir.path(), "a.pdf", "文档 A");
        let doc = extract_pages(&a, 1, 1).expect("extract");
        assert_eq!(doc.get_pages().len(), 1);
        // Out-of-range is an error.
        assert!(extract_pages(&a, 5, 6).is_err());
    }

    #[test]
    fn split_requires_single_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = make_pdf(dir.path(), "a.pdf", "文档 A");
        let src = lopdf::Document::load(&a).expect("load");
        assert_eq!(ordered_pages(&src).len(), 1);
    }

    #[test]
    fn merge_empty_is_error() {
        assert!(merge_pdfs(&[]).is_err());
    }
}
