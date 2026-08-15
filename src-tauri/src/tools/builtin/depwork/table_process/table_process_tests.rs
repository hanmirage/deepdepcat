use super::*;

#[test]
fn dedup_removes_duplicates() {
    let rows = vec![
        vec!["a".into(), "1".into()],
        vec!["a".into(), "1".into()],
        vec!["b".into(), "2".into()],
    ];
    let (out, summary) = apply_operations(rows, &["dedup".to_string()]);
    assert_eq!(out.len(), 2);
    assert!(summary.contains("3 → 2"));
}

#[test]
fn filter_and_select() {
    let rows = vec![
        vec!["name".into(), "score".into()],
        vec!["alice".into(), "90".into()],
        vec!["bob".into(), "60".into()],
    ];
    let (out, _) = apply_operations(
        rows,
        &["filter:score=90".to_string(), "select:name".to_string()],
    );
    assert_eq!(
        out,
        vec![vec!["name".to_string()], vec!["alice".to_string()]]
    );
}

#[test]
fn csv_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("data.csv");
    std::fs::write(&path, "name,score\nalice,90\nbob,60\n").expect("write");
    let rows = read_csv(&path).expect("read");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1][0], "alice");
    let out = to_csv(&rows);
    assert!(out.contains("alice,90"));
}

#[test]
fn xlsx_cells_with_commas_survive_roundtrip() {
    use std::io::Write;
    // Regression: cells containing commas ("Zhang, San") were joined
    // with a bare "," and then re-parsed — the comma inside the cell
    // split it into spurious columns. The emitted CSV must escape it.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("with-comma.xlsx");
    let file = std::fs::File::create(&path).expect("create");
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("xl/sharedStrings.xml", opts)
        .expect("sharedStrings entry");
    zw.write_all(
        br#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Zhang, San</t></si><si><t>score</t></si></sst>"#,
    )
    .expect("write sharedStrings");
    zw.start_file("xl/worksheets/sheet1.xml", opts)
        .expect("sheet1 entry");
    zw.write_all(
        br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row></sheetData></worksheet>"#,
    )
    .expect("write sheet1");
    zw.finish().expect("finish zip");

    let csv_text = read_xlsx(&path, None).expect("read xlsx");
    // The comma inside the shared string must be quoted in the CSV.
    assert!(csv_text.contains("\"Zhang, San\",score"), "got: {csv_text}");
    let rows = read_csv_from_text(&csv_text).expect("re-parse");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], vec!["Zhang, San".to_string(), "score".to_string()]);
}

#[test]
fn xlsx_ragged_rows_padded_to_grid_width() {
    use std::io::Write;
    // Regression: OOXML omits empty cells, so a 5-column header followed by
    // a 7-cell row used to fail CSV serialization with
    // "found record with 7 fields, but the previous record has 5 fields".
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ragged.xlsx");
    let file = std::fs::File::create(&path).expect("create");
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("xl/sharedStrings.xml", opts)
        .expect("sharedStrings entry");
    zw.write_all(
        br#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Zhang, San</t></si><si><t>title</t></si><si><t>source</t></si><si><t>year</t></si><si><t>points</t></si><si><t>tags</t></si></sst>"#,
    )
    .expect("write sharedStrings");
    zw.start_file("xl/worksheets/sheet1.xml", opts)
        .expect("sheet1 entry");
    zw.write_all(
        br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>1</v></c><c r="B1" t="s"><v>2</v></c><c r="C1" t="s"><v>3</v></c><c r="D1" t="s"><v>4</v></c><c r="E1" t="s"><v>5</v></c></row><row r="2"><c r="A2" t="s"><v>0</v></c><c r="B2" t="s"><v>2</v></c><c r="C2"><v>2026</v></c><c r="D2" t="s"><v>4</v></c><c r="E2" t="s"><v>5</v></c><c r="F2"><v>extra</v></c><c r="G2"><v>cell</v></c></row></sheetData></worksheet>"#,
    )
    .expect("write sheet1");
    zw.finish().expect("finish zip");

    let csv_text = read_xlsx(&path, None).expect("read xlsx");
    let rows = read_csv_from_text(&csv_text).expect("re-parse");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].len(), 7, "header padded to widest row: {rows:?}");
    assert_eq!(rows[1].len(), 7);
    assert_eq!(rows[1][0], "Zhang, San");
    assert_eq!(rows[0][5], "", "omitted cells become empty strings");
    assert_eq!(rows[1][6], "cell");
}

#[test]
fn ragged_csv_rows_parse_and_roundtrip() {
    let rows = read_csv_from_text("a,b,c\n1\nx,y\n").expect("parse");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].len(), 3);
    assert_eq!(rows[1].len(), 1);
    assert_eq!(rows[2].len(), 2);
    let out = to_csv(&rows);
    let reparsed = read_csv_from_text(&out).expect("re-parse");
    assert_eq!(reparsed, rows);
}

/// Build an xlsx with workbook.xml + rels + two sheets: sheet "Alpha"
/// (inlineStr cell) and sheet "Beta" (shared string + plain value).
fn two_sheet_xlsx(path: &std::path::Path) {
    use std::io::Write;
    let file = std::fs::File::create(path).expect("create");
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("xl/workbook.xml", opts)
        .expect("workbook entry");
    zw.write_all(
        br#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Alpha" sheetId="1" r:id="rId1"/><sheet name="Beta" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
    )
    .expect("write workbook");
    zw.start_file("xl/_rels/workbook.xml.rels", opts)
        .expect("rels entry");
    zw.write_all(
        br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="..." Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="..." Target="worksheets/sheet2.xml"/></Relationships>"#,
    )
    .expect("write rels");
    zw.start_file("xl/sharedStrings.xml", opts)
        .expect("sharedStrings entry");
    zw.write_all(
        br#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>from-shared</t></si></sst>"#,
    )
    .expect("write sharedStrings");
    // Alpha: inline string cell (previously read as empty).
    zw.start_file("xl/worksheets/sheet1.xml", opts)
        .expect("sheet1 entry");
    zw.write_all(
        br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>alpha-inline</t></is></c></row></sheetData></worksheet>"#,
    )
    .expect("write sheet1");
    // Beta: shared string + plain numeric value.
    zw.start_file("xl/worksheets/sheet2.xml", opts)
        .expect("sheet2 entry");
    zw.write_all(
        br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>42</v></c></row></sheetData></worksheet>"#,
    )
    .expect("write sheet2");
    zw.finish().expect("finish zip");
}

#[test]
fn xlsx_inline_strings_are_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("two-sheets.xlsx");
    two_sheet_xlsx(&path);
    // Default: first sheet (Alpha), inlineStr must not be empty.
    let csv_text = read_xlsx(&path, None).expect("read default sheet");
    assert!(csv_text.contains("alpha-inline"), "got: {csv_text}");
}

#[test]
fn xlsx_sheet_selection_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("two-sheets.xlsx");
    two_sheet_xlsx(&path);

    let beta = read_xlsx(&path, Some("Beta")).expect("read Beta");
    assert!(beta.contains("from-shared"), "got: {beta}");
    assert!(beta.contains("42"), "plain value: {beta}");
    assert!(!beta.contains("alpha-inline"));

    let alpha = read_xlsx(&path, Some("Alpha")).expect("read Alpha");
    assert!(alpha.contains("alpha-inline"));
}

#[test]
fn xlsx_unknown_sheet_name_errors_with_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("two-sheets.xlsx");
    two_sheet_xlsx(&path);
    let err = read_xlsx(&path, Some("Nope")).expect_err("must fail");
    let err = err.to_string();
    assert!(err.contains("'Nope' not found"), "{err}");
    assert!(
        err.contains("Alpha") && err.contains("Beta"),
        "lists sheets: {err}"
    );
}

#[test]
fn numeric_sort_sorts_numbers_numerically() {
    let rows = vec![
        vec!["name".into(), "score".into()],
        vec!["a".into(), "10".into()],
        vec!["b".into(), "9".into()],
        vec!["c".into(), "2".into()],
    ];
    let (out, _) = apply_operations(rows, &["sort:score".to_string()]);
    assert_eq!(
        out.iter().skip(1).map(|r| r[1].as_str()).collect::<Vec<_>>(),
        vec!["2", "9", "10"],
        "numeric ascending, not lexicographic (10 would sort before 9)"
    );
}

#[test]
fn numeric_sort_descending() {
    let rows = vec![
        vec!["name".into(), "score".into()],
        vec!["a".into(), "10".into()],
        vec!["b".into(), "9".into()],
        vec!["c".into(), "2".into()],
    ];
    let (out, _) = apply_operations(rows, &["sort:-score".to_string()]);
    assert_eq!(
        out.iter().skip(1).map(|r| r[1].as_str()).collect::<Vec<_>>(),
        vec!["10", "9", "2"]
    );
}

#[test]
fn filter_compare_ops_numeric() {
    let rows = vec![
        vec!["name".into(), "score".into()],
        vec!["alice".into(), "95".into()],
        vec!["bob".into(), "60".into()],
        vec!["carol".into(), "90".into()],
    ];
    let (gt, _) = apply_operations(rows.clone(), &["filter:score>90".to_string()]);
    assert_eq!(gt.len(), 2, "header + alice(95)");
    assert_eq!(gt[1][0], "alice");
    let (ge, _) = apply_operations(rows.clone(), &["filter:score>=90".to_string()]);
    assert_eq!(ge.len(), 3, "header + alice + carol(90)");
    let (lt, _) = apply_operations(rows.clone(), &["filter:score<90".to_string()]);
    assert_eq!(lt.len(), 2, "header + bob(60)");
    let (ne, _) = apply_operations(rows.clone(), &["filter:score!=90".to_string()]);
    assert_eq!(ne.len(), 3, "header + alice + bob");
}

#[test]
fn filter_string_fallback_for_non_numeric() {
    let rows = vec![
        vec!["name".into(), "x".into()],
        vec!["alpha".into(), "1".into()],
        vec!["beta".into(), "2".into()],
        vec!["gamma".into(), "3".into()],
    ];
    let (out, _) = apply_operations(rows, &["filter:name>alpha".to_string()]);
    assert_eq!(out.len(), 3, "header + beta + gamma");
    assert_eq!(out[1][0], "beta");
    assert_eq!(out[2][0], "gamma");
}

#[test]
fn filter_equals_syntax_still_works() {
    let rows = vec![
        vec!["name".into(), "score".into()],
        vec!["alice".into(), "90".into()],
        vec!["bob".into(), "60".into()],
    ];
    let (out, _) = apply_operations(rows, &["filter:score=90".to_string()]);
    assert_eq!(out.len(), 2);
    assert_eq!(out[1][0], "alice");
}

#[test]
fn to_number_tolerates_thousands_separators() {
    assert_eq!(to_number("1,234"), Some(1234.0));
    assert_eq!(to_number("90"), Some(90.0));
    assert_eq!(to_number(" 90 "), Some(90.0));
    assert_eq!(to_number("abc"), None);
}
