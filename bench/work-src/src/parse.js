/** Parse a JSON config string into { ok, value } (currently throws). */
export function parseConfig(raw) {
  return JSON.parse(raw);
}
