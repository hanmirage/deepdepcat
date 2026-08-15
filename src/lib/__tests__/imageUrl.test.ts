import { describe, it, expect } from "vitest";
import { isImageUrl, extractFirstUrl } from "@/lib/image";

describe("isImageUrl", () => {
  it("accepts image extensions across hosts", () => {
    expect(isImageUrl("https://cdn.example.com/a.png")).toBe(true);
    expect(isImageUrl("https://img.example.com/photo.JPEG")).toBe(true);
    expect(isImageUrl("https://example.com/a.webp")).toBe(true);
    expect(isImageUrl("https://example.com/a.gif")).toBe(true);
    expect(isImageUrl("https://example.com/scan.tiff")).toBe(true);
    expect(isImageUrl("https://example.com/favicon.ico")).toBe(true);
  });

  it("ignores query strings and fragments", () => {
    expect(isImageUrl("https://cdn.example.com/a.png?token=abc&x=1")).toBe(true);
    expect(isImageUrl("https://img.example.com/1.webp#frag")).toBe(true);
  });

  it("rejects non-image URLs and plain text", () => {
    expect(isImageUrl("https://example.com/page")).toBe(false);
    expect(isImageUrl("https://example.com/a.png.html")).toBe(false);
    expect(isImageUrl("C:\\tmp\\a.png")).toBe(false);
    expect(isImageUrl("a.png")).toBe(false);
    expect(isImageUrl("")).toBe(false);
  });
});

describe("extractFirstUrl", () => {
  it("finds the first http(s) URL in a blob", () => {
    expect(extractFirstUrl("hello https://example.com/a.png world")).toBe(
      "https://example.com/a.png",
    );
    expect(
      extractFirstUrl("https://x.com/1\nhttps://y.com/2\n"),
    ).toBe("https://x.com/1");
  });

  it("strips trailing punctuation", () => {
    expect(extractFirstUrl("see https://example.com/a.png,")).toBe(
      "https://example.com/a.png",
    );
  });

  it("returns null when nothing matches", () => {
    expect(extractFirstUrl("no links here")).toBeNull();
    expect(extractFirstUrl("")).toBeNull();
  });
});
