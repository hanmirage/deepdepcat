/**
 * Vitest global setup �� runs before each test file.
 *
 * Imports @testing-library/jest-dom matchers (toBeInTheDocument, etc.)
 * and provides a clean DOM between tests.
 */

import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import "@/i18n";

afterEach(() => {
  cleanup();
});

// Pretend we are inside a Tauri webview so `isTauri` in src/lib/tauri.ts
// takes the real IPC path (setupFiles run before test-file imports, so the
// flag is in place when the module is evaluated).
Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });

// pdfjs-dist reads DOMMatrix at module scope (PdfViewer) — jsdom does not
// implement it. A minimal identity transform satisfies the import without
// pretending to be a real implementation (PDF rendering is never asserted).
if (typeof window.DOMMatrix === "undefined") {
  class DOMMatrixStub {
    constructor(init?: string | number[]) {
      if (typeof init === "string") {
        const parts = init.split(",").map(Number);
        if (parts.length === 6) {
          this.a = parts[0];
          this.b = parts[1];
          this.c = parts[2];
          this.d = parts[3];
          this.e = parts[4];
          this.f = parts[5];
        }
      }
    }
    a = 1;
    b = 0;
    c = 0;
    d = 1;
    e = 0;
    f = 0;
    multiplySelf(other: DOMMatrixStub) {
      this.a = other.a;
      this.b = other.b;
      this.c = other.c;
      this.d = other.d;
      this.e = other.e;
      this.f = other.f;
      return this;
    }
    translateSelf(tx = 0, ty = 0) {
      this.e += tx;
      this.f += ty;
      return this;
    }
    scaleSelf(sx = 1, sy = 1) {
      this.a *= sx;
      this.d *= sy;
      return this;
    }
    toJSON() {
      return { a: this.a, b: this.b, c: this.c, d: this.d, e: this.e, f: this.f };
    }
  }
  Object.defineProperty(window, "DOMMatrix", { value: DOMMatrixStub, configurable: true });
}
