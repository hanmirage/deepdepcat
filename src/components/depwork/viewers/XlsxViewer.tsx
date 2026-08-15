/**
 * XlsxViewer — renders a .xlsx spreadsheet in the Depwork preview panel.
 *
 * Parses with SheetJS (`xlsx`, pure frontend) and renders a read-only table
 * (rows × columns, basic alignment). Multi-sheet workbooks get a sheet tab
 * row to switch between sheets. Large sheets are capped so the panel stays
 * responsive.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { logError } from "@/lib/logger";
import * as XLSX from "xlsx";
import { FileWarning, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { readWorkspaceBinaryFile } from "@/lib/tauri";
import { cn } from "@/lib/utils";

interface XlsxViewerProps {
  filePath: string;
}

/** Cap on rendered rows/cols — beyond this the sheet shows a note. */
const MAX_ROWS = 500;
const MAX_COLS = 100;

export function XlsxViewer({ filePath }: XlsxViewerProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<"loading" | "ready" | "error" | "unavailable">("loading");
  const [sheets, setSheets] = useState<string[]>([]);
  const [activeSheet, setActiveSheet] = useState("");
  const [rows, setRows] = useState<string[][]>([]);
  const [truncated, setTruncated] = useState(false);
  const wbRef = useRef<XLSX.WorkBook | null>(null);

  useEffect(() => {
    let cancelled = false;
    setState("loading");

    void (async () => {
      try {
        const bytes = await readWorkspaceBinaryFile(filePath);
        if (cancelled) return;
        if (!bytes || bytes.length === 0) {
          setState("unavailable");
          return;
        }
        const wb = XLSX.read(bytes, { type: "array", cellDates: false });
        const names = wb.SheetNames;
        if (names.length === 0) throw new Error("no sheets");
        if (!cancelled) {
          wbRef.current = wb;
          setSheets(names);
          setActiveSheet((prev) => (names.includes(prev) ? prev : names[0]));
          setState("ready");
        }
      } catch (e) {
        logError("XlsxViewer", "read/parse failed:", e);
        if (!cancelled) setState("error");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [filePath]);

  // Re-extract the active sheet's matrix whenever the workbook or the
  // selected sheet changes.
  useEffect(() => {
    if (state !== "ready") return;
    const wb = wbRef.current;
    if (!wb || !activeSheet) return;
    const ws = wb.Sheets[activeSheet];
    if (!ws) return;
    // Extract at most MAX_ROWS×MAX_COLS so a huge sheet can't build the full
    // matrix in memory. The range end is clamped to the sheet's ACTUAL
    // dimensions — letting it exceed the data makes sheet_to_json pad empty
    // rows/cols all the way to the cap.
    const dims = ws["!ref"] ? XLSX.utils.decode_range(ws["!ref"]) : null;
    const endRow = dims ? Math.min(dims.e.r, MAX_ROWS - 1) : MAX_ROWS - 1;
    const endCol = dims ? Math.min(dims.e.c, MAX_COLS - 1) : MAX_COLS - 1;
    const matrix = XLSX.utils.sheet_to_json<string[]>(ws, {
      header: 1,
      raw: false,
      defval: "",
      range: { s: { r: 0, c: 0 }, e: { r: endRow, c: endCol } },
    });

    const rowCount = Math.min(matrix.length, MAX_ROWS);
    const colCount = Math.min(
      Math.max(1, ...matrix.slice(0, rowCount).map((r) => r.length)),
      MAX_COLS,
    );
    const view = matrix.slice(0, rowCount).map((r) => {
      const out: string[] = [];
      for (let c = 0; c < colCount; c++) out.push(r[c] ?? "");
      return out;
    });

    setRows(view);
    // Truncated only when the sheet actually has more than we rendered —
    // compare against the sheet's declared dimensions (not the extraction
    // cap, which is always exactly MAX_ROWS×MAX_COLS).
    const sheetRows = dims ? dims.e.r + 1 : 0;
    const sheetCols = dims ? dims.e.c + 1 : 0;
    setTruncated(sheetRows > MAX_ROWS || sheetCols > MAX_COLS);
    setState("ready");
  }, [state, activeSheet]);

  const columnLetters = useMemo(
    () => Array.from({ length: Math.max(rows[0]?.length ?? 0, 1) }, (_, i) => i + 1),
    [rows],
  );

  if (state === "unavailable") {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
        <FileWarning className="h-8 w-8 text-muted-foreground/30" />
        <p className="text-xs text-muted-foreground">
          {t("depwork.previewBrowserOnly", {
            defaultValue: "文档预览仅在桌面端可用（浏览器模式无法读取文件）",
          })}
        </p>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <div className="flex items-center justify-between gap-2 border-b border-border px-3 py-1.5">
        <span className="truncate text-[11px] font-medium text-muted-foreground">
          {t("depwork.previewSheets", { defaultValue: "共 {{count}} 个工作表" }).replace("{{count}}", String(sheets.length))}
        </span>
        {truncated && (
          <span className="shrink-0 text-[10px] text-muted-foreground/50">
            {t("depwork.previewTruncated", { defaultValue: "已截断显示" })}
          </span>
        )}
      </div>

      {sheets.length > 1 && (
        <div
          className="flex gap-1 overflow-x-auto border-b border-border px-2 py-1.5"
          role="tablist"
          aria-label={t("depwork.previewSheets", { defaultValue: "工作表" })}
        >
          {sheets.map((name) => (
            <button
              key={name}
              role="tab"
              aria-selected={name === activeSheet}
              onClick={() => setActiveSheet(name)}
              className={cn(
                "shrink-0 rounded-md px-2 py-1 text-[11px] transition-colors",
                name === activeSheet
                  ? "bg-primary/10 font-medium text-primary"
                  : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
              )}
            >
              {name}
            </button>
          ))}
        </div>
      )}

      {state === "loading" ? (
        <div className="flex flex-1 items-center justify-center">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : state === "error" ? (
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          <p className="text-xs text-muted-foreground">
            {t("depwork.previewCantRead", { defaultValue: "无法预览此文档" })}
          </p>
        </div>
      ) : (
        <div className="flex-1 overflow-auto">
          <table className="w-max border-collapse text-[11px]">
            <thead>
              <tr className="sticky top-0 bg-muted/60">
                <th className="min-w-8 border border-border/60 px-1.5 py-1 text-center font-mono text-[10px] font-normal text-muted-foreground/60" />
                {columnLetters.map((c) => (
                  <th
                    key={c}
                    className="min-w-16 border border-border/60 px-1.5 py-1 text-center font-mono text-[10px] font-normal text-muted-foreground/60"
                  >
                    {XLSX.utils.encode_col(c - 1)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, r) => (
                <tr key={r} className={r % 2 === 1 ? "bg-muted/20" : undefined}>
                  <td className="border border-border/60 px-1.5 py-1 text-center font-mono text-[10px] text-muted-foreground/50">
                    {r + 1}
                  </td>
                  {row.map((cell, c) => (
                    <td
                      key={c}
                      className="max-w-64 border border-border/60 px-1.5 py-1 whitespace-pre-wrap break-words"
                    >
                      {cell}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
