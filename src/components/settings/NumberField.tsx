/**
 * NumberField — number input that can be cleared while typing.
 *
 * Keeps a local string while focused (so deleting the digits doesn't snap to
 * the min value mid-edit) and commits the parsed value on blur. Empty/NaN
 * commits `min` (the "unlimited" value for the session limits).
 */

import { useState, useEffect } from "react";
import { Input } from "@/components/ui/input";

export interface NumberFieldProps {
  value: number;
  min?: number;
  max?: number;
  step?: string;
  placeholder?: string;
  onCommit: (v: number) => void;
}

export function NumberField({
  value,
  min = 0,
  max,
  step,
  placeholder,
  onCommit,
}: NumberFieldProps) {
  const [text, setText] = useState(String(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) setText(String(value));
  }, [value, focused]);

  return (
    <Input
      type="number"
      min={min}
      max={max}
      step={step}
      value={focused ? text : String(value)}
      onChange={(e) => setText(e.target.value)}
      onFocus={() => setFocused(true)}
      onBlur={() => {
        setFocused(false);
        const parsed = Number(text);
        let value = text.trim() === "" || Number.isNaN(parsed) ? min : parsed;
        // Clamp typed values into [min, max] — HTML min/max attributes only
        // affect spinner arrows and native validation, not typed values.
        // Guard non-finite inputs (e.g. "1e999" → Infinity): Infinity would
        // otherwise skip the clamp and commit a value that JSON-serializes to
        // null on the backend. Non-finite goes to the max end (or min if
        // unbounded).
        if (!Number.isFinite(value)) {
          value = max !== undefined ? max : min;
        } else {
          if (max !== undefined) value = Math.min(value, max);
          if (value < min) value = min;
        }
        onCommit(value);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
      }}
      placeholder={placeholder}
      className="h-8 w-32 text-xs"
    />
  );
}
