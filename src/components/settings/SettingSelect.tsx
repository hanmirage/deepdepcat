/**
 * SettingSelect — styled select dropdown for settings rows.
 *
 * Renders a native <select> styled with Tailwind, matching the app's
 * settings combobox appearance. Dark mode supported via semantic tokens.
 */

import { cn } from "@/lib/utils";

export interface SettingSelectProps {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
  disabled?: boolean;
  className?: string;
}

export function SettingSelect({
  value,
  onChange,
  options,
  disabled = false,
  className,
}: SettingSelectProps) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      disabled={disabled}
      className={cn(
        "h-8 rounded-md border border-input bg-background px-2 text-xs",
        "focus:outline-none focus:ring-2 focus:ring-ring",
        "disabled:opacity-50 disabled:cursor-not-allowed",
        className,
      )}
    >
      {options.map((opt) => (
        <option key={opt.value} value={opt.value}>
          {opt.label}
        </option>
      ))}
    </select>
  );
}
