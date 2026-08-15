/**
 * PptxViewer — renders a .pptx presentation in the Depwork preview panel.
 *
 * Pure-frontend, hand-rolled OOXML reader: unzips with jszip, parses each
 * slide's XML for text runs (paragraphs/spans, layout placeholders), shape
 * geometry (x/y/w/h), and images (embedded via `a:blip` → media file).
 *
 * Scope: text boxes, plain shapes with text, pictures, placeholder text.
 * Complex elements (charts, SmartArt, tables, animations) render as a muted
 * placeholder block with a note — never crashes the viewer.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { logError } from "@/lib/logger";
import JSZip from "jszip";
import { ChevronLeft, ChevronRight, FileWarning, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { readWorkspaceBinaryFile } from "@/lib/tauri";

interface PptxViewerProps {
  filePath: string;
}

/** One rendered slide element: text block or image. */
interface SlideElement {
  kind: "text" | "image" | "unsupported";
  x: number;
  y: number;
  w: number;
  h: number;
  /** Text lines (kind=text) — spans joined per paragraph. */
  lines: string[];
  /** Media reference (kind=image) — resolved against the zip. */
  relTarget?: string;
  /** Human label for unsupported elements (chart/smartart/table). */
  unsupportedLabel?: string;
}

/** Slide geometry in EMU (914400 per inch) — convert to px (96dpi). */
const EMU_PER_PX = 9525;
/** Slide canvas: 16:9 default (12192000 × 6858000 EMU). */
const SLIDE_W_EMU = 12192000;
const SLIDE_H_EMU = 6858000;

const NS = {
  a: "http://schemas.openxmlformats.org/drawingml/2006/main",
  p: "http://schemas.openxmlformats.org/presentationml/2006/main",
  r: "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
  pic: "http://schemas.openxmlformats.org/drawingml/2006/picture",
  c: "http://schemas.openxmlformats.org/drawingml/2006/chart",
};

function emuToPx(emu: number): number {
  return emu / EMU_PER_PX;
}

function parseCoord(value: string | undefined, fallback: number): number {
  const n = Number(value);
  return Number.isFinite(n) ? n : fallback;
}

/** Namespace-aware child lookup. */
function child(el: Element, ns: string, local: string): Element | null {
  return el.getElementsByTagNameNS(ns, local).item(0) ?? null;
}

function children(el: Element, ns: string, local: string): Element[] {
  return Array.from(el.getElementsByTagNameNS(ns, local));
}

/** Extract text lines from an `a:txBody` element (paragraph → spans). */
function parseTextBody(txBody: Element): string[] {
  return children(txBody, NS.a, "p").map((p) => {
    const runs = children(p, NS.a, "r").map((r) => {
      const t = child(r, NS.a, "t");
      return t?.textContent ?? "";
    });
    const breaks = children(p, NS.a, "br");
    return runs.length > 0 ? runs.join("") : breaks.length > 0 ? "\n" : "";
  });
}

/** Parse a single slide XML into ordered renderable elements. */
function parseSlide(xml: string): SlideElement[] {
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  const out: SlideElement[] = [];

  // Graphic frames: pics (images) and chart/graphicData (unsupported).
  for (const gf of children(doc.documentElement, NS.p, "graphicFrame")) {
    const chart = child(gf, NS.c, "chart");
    if (chart) {
      out.push({ kind: "unsupported", x: 0, y: 0, w: 0, h: 0, lines: [], unsupportedLabel: "chart" });
      continue;
    }
    // graphicData → fall through to unsupported (e.g. SmartArt/table).
    out.push({ kind: "unsupported", x: 0, y: 0, w: 0, h: 0, lines: [], unsupportedLabel: "graphic" });
  }

  // Pictures: a:blip embed → r:embed relationship.
  for (const pic of children(doc.documentElement, NS.pic, "pic")) {
    const spPr = child(pic, NS.p, "spPr") ?? child(pic, NS.a, "spPr");
    const off = child(spPr ?? pic, NS.a, "off");
    const ext = child(spPr ?? pic, NS.a, "ext");
    const blip = child(pic, NS.a, "blip");
    const embed = blip?.getAttributeNS(NS.r, "embed");
    out.push({
      kind: "image",
      x: parseCoord(off?.getAttribute("x") ?? undefined, 0),
      y: parseCoord(off?.getAttribute("y") ?? undefined, 0),
      w: parseCoord(ext?.getAttribute("cx") ?? undefined, 0),
      h: parseCoord(ext?.getAttribute("cy") ?? undefined, 0),
      lines: [],
      relTarget: embed ?? undefined,
    });
  }

  // Shape elements (text boxes + autoshapes with text).
  for (const sp of children(doc.documentElement, NS.p, "sp")) {
    const spPr = child(sp, NS.p, "spPr") ?? child(sp, NS.a, "spPr");
    const off = child(spPr ?? sp, NS.a, "off");
    const ext = child(spPr ?? sp, NS.a, "ext");
    // `p:txBody` lives in the presentationML namespace on `p:sp` (its
    // paragraphs/runs are DrawingML `a:p`/`a:r`).
    const txBody = child(sp, NS.p, "txBody") ?? child(sp, NS.a, "txBody");
    const lines = txBody ? parseTextBody(txBody) : [];
    if (lines.every((l) => l.trim() === "")) {
      continue; // empty shape — skip
    }
    out.push({
      kind: "text",
      x: parseCoord(off?.getAttribute("x") ?? undefined, 0),
      y: parseCoord(off?.getAttribute("y") ?? undefined, 0),
      w: parseCoord(ext?.getAttribute("cx") ?? undefined, 0),
      h: parseCoord(ext?.getAttribute("cy") ?? undefined, 0),
      lines,
    });
  }

  return out;
}

/** Resolve a relationship id to its target via the part's .rels file. */
async function relTargetFor(zip: JSZip, part: string, relId: string): Promise<string | null> {
  // part: "ppt/slides/slide1.xml" → rels at "ppt/slides/_rels/slide1.xml.rels"
  const slash = part.lastIndexOf("/");
  const dir = part.slice(0, slash);
  const file = part.slice(slash + 1);
  const relsPath = `${dir}/_rels/${file}.rels`;
  const relsXml = await zip.file(relsPath)?.async("string");
  if (!relsXml) return null;
  const doc = new DOMParser().parseFromString(relsXml, "application/xml");
  const rel = Array.from(doc.getElementsByTagName("Relationship")).find(
    (r) => r.getAttribute("Id") === relId,
  );
  return rel?.getAttribute("Target") ?? null;
}

/** Resolve an image relationship target to an object URL over the zip. */
async function resolveImage(zip: JSZip, target: string | undefined): Promise<string | null> {
  if (!target) return null;
  const norm = target.replace(/^\/+/, "");
  const entry = zip.file(norm) ?? zip.file(`ppt/${norm}`);
  if (!entry) return null;
  const blob = await entry.async("blob");
  return URL.createObjectURL(blob);
}

export function PptxViewer({ filePath }: PptxViewerProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<"loading" | "ready" | "error" | "unavailable">("loading");
  const [slides, setSlides] = useState<SlideElement[][]>([]);
  const [index, setIndex] = useState(0);
  const [containerW, setContainerW] = useState(0);
  const wrapRef = useRef<HTMLDivElement>(null);
  // Every resolved image is an object URL — they MUST be revoked on unmount
  // or file switch, or memory leaks grow with each presentation opened.
  const objectUrlsRef = useRef<string[]>([]);

  // Container width — the scale (fit-to-width) follows the panel's actual
  // width instead of assuming a fixed 288px.
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const update = () => {
      const w = wrap.clientWidth;
      if (w > 0) setContainerW(w);
    };
    update();
    const obs = new ResizeObserver(update);
    obs.observe(wrap);
    return () => obs.disconnect();
  }, []);

  useEffect(() => {
    let cancelled = false;
    setState("loading");
    setIndex(0);

    void (async () => {
      const bytes = await readWorkspaceBinaryFile(filePath);
      if (cancelled) return;
      if (!bytes || bytes.length === 0) {
        setState("unavailable");
        return;
      }
      try {
        const zip = await JSZip.loadAsync(bytes);
        // Direct: list slide parts in order (slide1..N is the canonical order).
        const slideFiles = zip
          .folder("ppt/slides")
          ?.filter((relativePath) => /^slide\d+\.xml$/i.test(relativePath))
          .sort((a, b) => {
            const na = Number(a.name.split(/slide(\d+)/i)[1]);
            const nb = Number(b.name.split(/slide(\d+)/i)[1]);
            return na - nb;
          });
        if (!slideFiles || slideFiles.length === 0) throw new Error("no slides");

        const parsed: SlideElement[][] = [];
        for (const file of slideFiles) {
          // A newer file selection cancelled this parse mid-loop — stop
          // immediately so we don't keep pushing object URLs after the
          // cleanup already revoked them (they'd leak forever).
          if (cancelled) return;
          const xml = await file.async("string");
          const elements = parseSlide(xml);
          // Resolve image embeds to object URLs.
          for (const el of elements) {
            if (el.kind === "image" && el.relTarget) {
              const target = await relTargetFor(zip, file.name, el.relTarget);
              const url = await resolveImage(zip, target ?? undefined);
              if (cancelled) {
                // A newer file selected while this embed was decoding —
                // revoke the freshly-created URL (cleanup already ran) so it
                // can't leak.
                if (url) URL.revokeObjectURL(url);
                return;
              }
              if (url) {
                objectUrlsRef.current.push(url);
                el.relTarget = url;
              } else {
                el.kind = "unsupported";
                el.unsupportedLabel = "image";
              }
            }
          }
          parsed.push(elements);
        }
        if (!cancelled) {
          setSlides(parsed);
          setState("ready");
        }
      } catch (e) {
        logError("PptxViewer", "parse failed:", e);
        if (!cancelled) setState("error");
      }
    })();

    return () => {
      cancelled = true;
      // Revoke object URLs created for the previous file — otherwise every
      // presentation opened leaks its decoded image blobs.
      for (const url of objectUrlsRef.current) URL.revokeObjectURL(url);
      objectUrlsRef.current = [];
    };
  }, [filePath]);

  const scale = useMemo(() => {
    // Fit the 16:9 slide into the ACTUAL container width (observed above) —
    // the panel may be narrower or much wider than the old fixed 288px.
    const avail = Math.max(containerW - 24, 120);
    return avail / (SLIDE_W_EMU / EMU_PER_PX);
  }, [containerW]);

  const current = slides[index] ?? [];

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
      {/* Slide pager */}
      <div className="flex items-center justify-between border-b border-border px-3 py-1.5">
        <button
          onClick={() => setIndex((i) => Math.max(0, i - 1))}
          disabled={index === 0}
          className="rounded p-0.5 text-muted-foreground/70 hover:bg-muted hover:text-foreground disabled:opacity-30"
          aria-label={t("common.prev", { defaultValue: "上一页" })}
        >
          <ChevronLeft className="h-3.5 w-3.5" />
        </button>
        <span className="font-mono text-[11px] tabular-nums text-muted-foreground">
          {slides.length > 0 ? `${index + 1} / ${slides.length}` : "-"}
        </span>
        <button
          onClick={() => setIndex((i) => Math.min(slides.length - 1, i + 1))}
          disabled={index >= slides.length - 1}
          className="rounded p-0.5 text-muted-foreground/70 hover:bg-muted hover:text-foreground disabled:opacity-30"
          aria-label={t("common.next", { defaultValue: "下一页" })}
        >
          <ChevronRight className="h-3.5 w-3.5" />
        </button>
      </div>

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
        <div ref={wrapRef} className="flex-1 overflow-auto bg-muted/20 p-3">
          <div
            className="relative mx-auto bg-white shadow-sm"
            style={{
              width: (SLIDE_W_EMU / EMU_PER_PX) * scale,
              height: (SLIDE_H_EMU / EMU_PER_PX) * scale,
            }}
          >
            {current.map((el, i) => {
              const x = emuToPx(el.x) * scale;
              const y = emuToPx(el.y) * scale;
              const w = emuToPx(el.w) * scale;
              const h = el.h > 0 ? emuToPx(el.h) * scale : undefined;
              if (el.kind === "image" && el.relTarget) {
                return (
                  <img
                    key={i}
                    src={el.relTarget}
                    alt=""
                    className="absolute"
                    style={{ left: x, top: y, width: w, height: h }}
                  />
                );
              }
              if (el.kind === "unsupported") {
                return (
                  <div
                    key={i}
                    className="absolute flex items-center justify-center rounded border border-dashed border-muted-foreground/30 text-[9px] text-muted-foreground/40"
                    style={{ left: x, top: y, width: Math.max(w, 40), height: Math.max(h ?? 16, 16) }}
                  >
                    {el.unsupportedLabel}
                  </div>
                );
              }
              return (
                <div
                  key={i}
                  className="absolute text-[10px] leading-snug text-foreground"
                  style={{
                    left: x,
                    top: y,
                    width: w,
                    height: h,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  {el.lines.map((line, li) => (
                    <p key={li} className="min-h-[1em]">
                      {line || "\u00A0"}
                    </p>
                  ))}
                </div>
              );
            })}
            {current.length === 0 && (
              <p className="absolute inset-0 flex items-center justify-center text-[10px] text-muted-foreground/40">
                {t("depwork.previewEmptySlide", { defaultValue: "空白页" })}
              </p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
