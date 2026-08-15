/**
 * Big.js — a deliberately oversized module with three separable
 * responsibilities (string helpers, number helpers, array helpers).
 * Task 8 splits it; exports must stay compatible.
 */

// ── String helpers ─────────────────────────────────────────────

/**
 * Pad a string to a target width on the left with the given fill char.
 * Used by table formatting in several places.
 */
export function padLeft(value, width, fill = " ") {
  const text = String(value);
  if (text.length >= width) return text;
  return fill.repeat(width - text.length) + text;
}

/**
 * Pad a string to a target width on the right.
 */
export function padRight(value, width, fill = " ") {
  const text = String(value);
  if (text.length >= width) return text;
  return text + fill.repeat(width - text.length);
}

/**
 * Truncate a string to max length, appending an ellipsis when cut.
 */
export function truncate(text, max) {
  if (text.length <= max) return text;
  if (max <= 3) return ".".repeat(max);
  return text.slice(0, max - 3) + "...";
}

/**
 * Split a comma-separated list, trimming whitespace and dropping empties.
 */
export function splitList(raw) {
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

// ── Number helpers ─────────────────────────────────────────────

/**
 * Clamp a number into the inclusive [min, max] range.
 */
export function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

/**
 * Round to a fixed number of decimals without floating noise.
 */
export function round(value, decimals = 2) {
  const factor = 10 ** decimals;
  return Math.round((value + Number.EPSILON) * factor) / factor;
}

/**
 * Percentage change between two numbers (null when base is 0).
 */
export function percentChange(before, after) {
  if (before === 0) return null;
  return round(((after - before) / before) * 100);
}

// ── Array helpers ──────────────────────────────────────────────

/**
 * Unique values in insertion order (strict equality).
 */
export function unique(values) {
  return [...new Set(values)];
}

/**
 * Chunk an array into arrays of at most size items.
 */
export function chunk(values, size) {
  if (size <= 0) throw new Error("chunk size must be positive");
  const out = [];
  for (let i = 0; i < values.length; i += size) {
    out.push(values.slice(i, i + size));
  }
  return out;
}

/**
 * Group an array by a key extractor, preserving first-seen key order.
 */
export function groupBy(values, keyOf) {
  const out = new Map();
  for (const value of values) {
    const key = keyOf(value);
    if (!out.has(key)) out.set(key, []);
    out.get(key).push(value);
  }
  return out;
}

/**
 * Rolling window sums — useful for moving averages in dashboards.
 */
export function rollingSum(values, windowSize) {
  const out = [];
  let sum = 0;
  for (let i = 0; i < values.length; i += 1) {
    sum += values[i];
    if (i >= windowSize) sum -= values[i - windowSize];
    if (i >= windowSize - 1) out.push(sum);
  }
  return out;
}

/**
 * Zip two arrays into pairs, stopping at the shorter length.
 */
export function zip(a, b) {
  const out = [];
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i += 1) {
    out.push([a[i], b[i]]);
  }
  return out;
}

// ── Combined formatting helpers ────────────────────────────────

/**
 * Format a list of numbers as a fixed-width table row.
 */
export function formatRow(values, width = 8) {
  return values.map((v) => padLeft(round(v), width)).join(" | ");
}

/**
 * Summarize an array with count/sum/avg/min/max in one pass.
 */
export function summarizeArray(values) {
  if (values.length === 0) {
    return { count: 0, sum: 0, avg: 0, min: null, max: null };
  }
  const sum = values.reduce((a, b) => a + b, 0);
  return {
    count: values.length,
    sum,
    avg: sum / values.length,
    min: Math.min(...values),
    max: Math.max(...values),
  };
}

/**
 * Build a simple ASCII bar chart line for a single value.
 */
export function bar(value, scale = 1, width = 20) {
  const filled = clamp(Math.round(value * scale), 0, width);
  return "#".repeat(filled) + ".".repeat(width - filled);
}

/**
 * Render a small key/value table from an object, keys sorted.
 */
export function renderTable(rows) {
  const keys = Object.keys(rows).sort();
  const width = Math.max(...keys.map((k) => k.length), 1);
  return keys.map((k) => `${padRight(k, width)}  ${String(rows[k])}`).join("\n");
}

/**
 * Percent-encode a string for use inside a URL query value.
 */
export function encodeQueryValue(value) {
  return encodeURIComponent(String(value)).replace(/%20/g, "+");
}

/**
 * Decode a query value encoded by encodeQueryValue.
 */
export function decodeQueryValue(value) {
  return decodeURIComponent(String(value).replace(/\+/g, " "));
}

/**
 * Build a query string from an object, skipping undefined values.
 */
export function buildQuery(params) {
  return Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null)
    .map(([k, v]) => `${encodeQueryValue(k)}=${encodeQueryValue(v)}`)
    .join("&");
}

/**
 * Parse a query string back into an object.
 */
export function parseQuery(raw) {
  const out = {};
  for (const part of raw.split("&")) {
    if (!part) continue;
    const eq = part.indexOf("=");
    if (eq < 0) {
      out[decodeQueryValue(part)] = "";
    } else {
      out[decodeQueryValue(part.slice(0, eq))] = decodeQueryValue(part.slice(eq + 1));
    }
  }
  return out;
}

/**
 * Validate that a value is a finite number; throws a typed message.
 */
export function requireNumber(value, name = "value") {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${name} must be a finite number`);
  }
  return value;
}

/**
 * Validate that an array is non-empty; throws when empty.
 */
export function requireNonEmpty(values, name = "values") {
  if (!Array.isArray(values) || values.length === 0) {
    throw new Error(`${name} must be a non-empty array`);
  }
  return values;
}

/**
 * Safe integer parse with fallback.
 */
export function parseIntOr(value, fallback) {
  const n = Number.parseInt(String(value), 10);
  return Number.isNaN(n) ? fallback : n;
}

/**
 * Safe float parse with fallback.
 */
export function parseFloatOr(value, fallback) {
  const n = Number.parseFloat(String(value));
  return Number.isNaN(n) ? fallback : n;
}

/**
 * True when the string looks like a positive integer.
 */
export function isPositiveInt(value) {
  return /^[1-9]\d*$/.test(String(value));
}

/**
 * True when the string looks like a non-negative integer (0 allowed).
 */
export function isNonNegativeInt(value) {
  return /^\d+$/.test(String(value));
}

/**
 * Normalize whitespace: trim and collapse inner runs to single spaces.
 */
export function normalizeWhitespace(text) {
  return String(text).replace(/\s+/g, " ").trim();
}

/**
 * Capitalize the first letter of each whitespace-separated word.
 */
export function titleCase(text) {
  return String(text)
    .split(/\s+/)
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1).toLowerCase())
    .join(" ");
}

/**
 * Snake-case a camelCase identifier.
 */
export function toSnakeCase(text) {
  return String(text)
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/\s+/g, "_")
    .toLowerCase();
}

/**
 * Camel-case a snake_case identifier.
 */
export function toCamelCase(text) {
  return String(text)
    .toLowerCase()
    .replace(/_([a-z])/g, (_, ch) => ch.toUpperCase());
}

/**
 * Strip a leading/trailing quote pair if present.
 */
export function unquote(text) {
  const s = String(text);
  if (s.length >= 2 && ((s[0] === '"' && s.at(-1) === '"') || (s[0] === "'" && s.at(-1) === "'"))) {
    return s.slice(1, -1);
  }
  return s;
}
