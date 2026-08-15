/**
 * CSV parsing utilities for the Depwork preview panel.
 *
 * Pure functions, deliberately dependency-free so encoding + quoting rules
 * are unit-testable without React or the xlsx bundle.
 */

/**
 * Parse CSV text (RFC 4180 subset used by Excel/WPS): quoted fields may
 * contain commas and newlines, `""` is an escaped quote, CRLF and LF both
 * terminate rows. A trailing newline does not produce an empty last row.
 */
export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQuotes = false;
  let i = 0;
  const n = text.length;

  while (i < n) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 2;
          continue;
        }
        inQuotes = false;
        i += 1;
        continue;
      }
      field += ch;
      i += 1;
      continue;
    }
    if (ch === '"' && field.length === 0) {
      inQuotes = true;
      i += 1;
      continue;
    }
    if (ch === ",") {
      row.push(field);
      field = "";
      i += 1;
      continue;
    }
    if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && text[i + 1] === "\n") i += 1;
      // A blank line (no accumulated field and no cells yet) is skipped —
      // Excel/WPS export never emits empty records.
      if (field.length === 0 && row.length === 0) {
        i += 1;
        continue;
      }
      row.push(field);
      field = "";
      rows.push(row);
      row = [];
      i += 1;
      continue;
    }
    field += ch;
    i += 1;
  }

  if (field.length > 0 || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

/**
 * Decode CSV bytes: strict UTF-8 first, then GBK/GB18030 — Excel/WPS
 * save Chinese CSVs as GBK without a BOM, and mojibake beats an error.
 */
export function decodeCsvBytes(bytes: Uint8Array): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    try {
      return new TextDecoder("gbk").decode(bytes);
    } catch {
      return new TextDecoder("utf-8").decode(bytes);
    }
  }
}

/** Spreadsheet-style column letters: 1 → A, 27 → AA (cap 100). */
export function columnLetter(index: number): string {
  let out = "";
  let v = index;
  while (v >= 0) {
    out = String.fromCharCode(65 + (v % 26)) + out;
    v = Math.floor(v / 26) - 1;
  }
  return out;
}
