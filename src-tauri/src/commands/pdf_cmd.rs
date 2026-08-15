//! PDF commands — direct extraction for the depwork preview panel.
//!
//! Reuses the battle-tested extraction pipeline from the code-side
//! `read_file_pdf` module (public entry, shared code, zero depwork changes).

use crate::tools::builtin::read_file_pdf;

/// Extract all text from a PDF file (text layer only; scanned pages yield an
/// empty result — the caller is expected to hint at the OCR tool).
#[tauri::command]
pub async fn extract_pdf_text(path: String) -> Result<String, String> {
    let result =
        read_file_pdf::extract_pdf_text(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    if result.trim().is_empty() {
        return Err("PDF 无可提取文本（可能是扫描件，请尝试 OCR 工具）".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Object};

    /// Build a minimal one-page PDF whose content stream carries an explicit
    /// UTF-16BE BOM string — the pipeline decodes it without a ToUnicode CMap.
    fn minimal_pdf() -> Document {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        // <FEFF...> = UTF-16BE BOM + "Hello PDF".
        let content =
            b"BT /F1 12 Tf 72 720 Td <FEFF00480065006C006C006F0020005000440046> Tj ET".to_vec();
        let content_id =
            doc.add_object(Object::Stream(lopdf::Stream::new(dictionary! {}, content)));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc
    }

    #[test]
    fn extracts_text_from_a_real_pdf() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.pdf");
        minimal_pdf().save(&path).expect("save pdf");
        let text = read_file_pdf::extract_pdf_text(&path).expect("extract");
        assert!(text.contains("Hello PDF"), "extracted: {text}");
    }

    #[test]
    fn errors_on_missing_file() {
        let result = read_file_pdf::extract_pdf_text(std::path::Path::new("nope.pdf"));
        assert!(result.is_err());
    }
}
