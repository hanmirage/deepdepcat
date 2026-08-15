/**
 * Image helpers — compress a File/Blob to a data URL for the image
 * transcription pipeline.
 *
 * Images attached to a chat message travel as `data:<mime>;base64,...`
 * URLs (no filesystem path) — the backend transcribes them via the vision
 * model. Canvas compression keeps the IPC payload small: a 4K screenshot
 * becomes ~200KB, far below the multi-MB raw bitmap.
 */

import i18n from "@/i18n";

const MAX_SIDE = 1024;
const JPEG_QUALITY = 0.8;

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error(i18n.t("common.decodeImageFailed")));
    img.src = src;
  });
}

/** Downscale + re-encode an image file to a JPEG data URL. */
export async function compressImageToDataUrl(file: File | Blob): Promise<string> {
  const raw = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error(i18n.t("common.readImageFailed")));
    reader.readAsDataURL(file);
  });
  return compressDataUrl(raw);
}

/** Downscale + re-encode an existing data URL (PNG/GIF/WebP → JPEG). */
export async function compressDataUrl(src: string): Promise<string> {
  const img = await loadImage(src);
  const scale = Math.min(1, MAX_SIDE / Math.max(img.naturalWidth, img.naturalHeight));
  if (scale === 1 && !src.includes("image/png") && !src.includes("image/gif")) {
    // Small non-PNG images pass through untouched (JPEG/WebP already lean).
    return src;
  }
  const w = Math.max(1, Math.round(img.naturalWidth * scale));
  const h = Math.max(1, Math.round(img.naturalHeight * scale));
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) return src;
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, w, h);
  ctx.drawImage(img, 0, 0, w, h);
  return canvas.toDataURL("image/jpeg", JPEG_QUALITY);
}

/** Read a File as a data URL without any processing. */
export function fileToDataUrl(file: File | Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error(i18n.t("common.readImageFailed")));
    reader.readAsDataURL(file);
  });
}

/** Extensions that mark a URL as a picture (mirror of the backend sniff list). */
const IMAGE_URL_PATTERN = /\.(png|jpe?g|webp|gif|bmp|tiff?|ico)(?:[?#].*)?$/i;

/**
 * True when the string is an http(s) URL whose path ends in an image
 * extension. The backend downloads such URLs and pushes the picture through
 * the vision pipeline (transcription + visual_describe).
 */
export function isImageUrl(value: string): boolean {
  const v = value.trim();
  if (!/^https?:\/\//i.test(v)) return false;
  try {
    const u = new URL(v);
    return IMAGE_URL_PATTERN.test(u.pathname);
  } catch {
    return false;
  }
}

/** First http(s) URL found in a pasted/dragged text blob, if any. */
export function extractFirstUrl(text: string): string | null {
  const m = text.match(/https?:\/\/[^\s"'<>]+/i);
  return m?.[0].replace(/[),.;]+$/, "") ?? null;
}
