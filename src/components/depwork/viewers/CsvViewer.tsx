/**
 * CsvViewer — renders a .csv file as a read-only table in the Depwork
 * preview panel. UTF-8 with GBK fallback (Excel/WPS Chinese CSVs), rows
 * and columns capped so large exports stay responsive.
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileWarning, Loader2 } from "lucide-react";
import { logError } from "@/lib/logger";
import { readWorkspaceBinaryFile } from "@/lib/tauri";
import { columnLetter, decodeCsvBytes, parseCsv } from "./csvUtils";

interface CsvViewerProps {
  filePath: string;
}

/** Cap on rendered rows/cols — beyond this the file shows a note. */
const MAX_ROWS = 500;
const MAX_COLS = 100;
/** Input guard: previews read at most this many bytes so a multi-hundred-MB
 *  export can't freeze the panel while parsing (rows are then truncated to
 *  MAX_ROWS anyway). */
const MAX_INPUT_BYTES = 4 * 1024 * 1024;

export function CsvViewer({ filePath }: CsvViewerProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<"loading" | "ready" | "error" | "unavailable">("loading");
  const [rows, setRows] = useState<string[][]>([]);
  const [truncated, setTruncated] = useState(false);

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
        const cappedBytes =
          bytes.length > MAX_INPUT_BYTES ? bytes.slice(0, MAX_INPUT_BYTES) : bytes;
        const matrix = parseCsv(decodeCsvBytes(cappedBytes));
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
        if (!cancelled) {
          setRows(view);
          setTruncated(
            bytes.length > MAX_INPUT_BYTES ||
              matrix.length > rowCount ||
              matrix.slice(0, rowCount).some((r) => r.length > colCount),
          );
          setState("ready");
        }
      } catch (e) {
        logError("CsvViewer", "read/parse failed:", e);
        if (!cancelled) setState("error");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [filePath]);

  const columnLetters = Array.from({ length: Math.max(rows[0]?.length ?? 0, 1) }, (_, i) => i);

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
          {t("depwork.previewCsvRows", { defaultValue: "共 {{count}} 行" }).replace(
            "{{count}}",
            String(rows.length),
          )}
        </span>
        {truncated && (
          <span className="shrink-0 text-[10px] text-muted-foreground/50">
            {t("depwork.previewTruncated", { defaultValue: "已截断显示" })}
          </span>
        )}
      </div>

      {state === "loading" ? (
        <div className="flex flex-1 items-center justify-center">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : state === "error" ? (
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          <p className="text-xs text-muted-foreground">
            {t("depwork.previewCantRead", { defaultValue: "无法预览此文件" })}
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
                    {columnLetter(c)}
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
