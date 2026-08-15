import { describe, it, expect } from "vitest";
import { columnLetter, decodeCsvBytes, parseCsv } from "../csvUtils";

describe("parseCsv", () => {
  it("parses simple rows", () => {
    expect(parseCsv("a,b,c\n1,2,3\n")).toEqual([
      ["a", "b", "c"],
      ["1", "2", "3"],
    ]);
  });

  it("handles quoted commas, quotes and newlines", () => {
    expect(parseCsv('"a,1","say ""hi""","line1\nline2"')).toEqual([
      ["a,1", 'say "hi"', "line1\nline2"],
    ]);
  });

  it("handles CRLF row endings", () => {
    expect(parseCsv("a,b\r\nc,d\r\n")).toEqual([
      ["a", "b"],
      ["c", "d"],
    ]);
  });

  it("does not append an empty row for a trailing newline", () => {
    expect(parseCsv("a\n")).toEqual([["a"]]);
  });

  it("handles empty fields and empty lines", () => {
    expect(parseCsv("a,,c\n\nx,y")).toEqual([
      ["a", "", "c"],
      ["x", "y"],
    ]);
  });

  it("keeps an empty first cell when a line starts with a comma", () => {
    expect(parseCsv("a,,c\n,x\n")).toEqual([
      ["a", "", "c"],
      ["", "x"],
    ]);
  });

  it("returns an empty matrix for empty input", () => {
    expect(parseCsv("")).toEqual([]);
  });
});

describe("decodeCsvBytes", () => {
  it("decodes UTF-8 bytes", () => {
    const bytes = new TextEncoder().encode("名称,数值\n苹果,3\n");
    expect(decodeCsvBytes(bytes)).toBe("名称,数值\n苹果,3\n");
  });

  it("falls back to GBK for non-UTF-8 bytes", () => {
    const decoder = new TextDecoder("gbk");
    const utf8 = "名称,数值\n苹果,3\n";
    const gbkBytes = encodeGbk(utf8);
    expect(decoder.decode(gbkBytes)).toBe(utf8);
    expect(decodeCsvBytes(gbkBytes)).toBe(utf8);
  });
});

/** Encode a string as GBK by manually mapping code points (test helper). */
function encodeGbk(text: string): Uint8Array {
  // TextEncoder only emits UTF-8; build GBK bytes via a manual table for the
  // characters used in this test (name/value/apple/fruit/3/newline).
  const table = new Map<string, number[]>([
    ["名", [0xC3, 0xFB]],
    ["称", [0xB3, 0xC6]],
    ["数", [0xCA, 0xFD]],
    ["值", [0xD6, 0xB5]],
    ["苹", [0xC6, 0xBB]],
    ["果", [0xB9, 0xFB]],
    [",", [0x2C]],
    ["\n", [0x0A]],
    ["3", [0x33]],
  ]);
  const out: number[] = [];
  for (const ch of text) {
    const bytes = table.get(ch);
    if (!bytes) throw new Error(`no GBK mapping for ${ch}`);
    out.push(...bytes);
  }
  return new Uint8Array(out);
}

describe("columnLetter", () => {
  it("maps spreadsheet-style column indices", () => {
    expect(columnLetter(0)).toBe("A");
    expect(columnLetter(25)).toBe("Z");
    expect(columnLetter(26)).toBe("AA");
    expect(columnLetter(27)).toBe("AB");
  });
});
