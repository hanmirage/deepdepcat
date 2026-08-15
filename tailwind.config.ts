import type { Config } from "tailwindcss";

/**
 * Tailwind CSS configuration for DeepDepCat
 *
 * Design system:
 * - Light: #FFFFFF background, dark text
 * - Dark: #1C1C1E background, light text
 * - Accent: #E86336 (orange-red) for highlights, active states
 * - Rounded corners: 12px base radius
 * - CSS variables drive all theme colors for easy dark/light switching
 */
const config: Config = {
  darkMode: "class",
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      fontFamily: {
        // Product stack — Inter (bundled) for latin/digits, CJK falls back
        // to system Chinese fonts (PingFang / Microsoft YaHei / Noto Sans SC).
        sans: [
          "Inter",
          "system-ui",
          "-apple-system",
          "PingFang SC",
          "Microsoft YaHei",
          "Noto Sans SC",
          "sans-serif",
        ],
        mono: [
          "JetBrains Mono",
          "Cascadia Code",
          "Consolas",
          "SFMono-Regular",
          "monospace",
        ],
      },
      boxShadow: {
        soft: "0 2px 8px 0 rgba(0, 0, 0, 0.04)",
        card: "0 1px 3px 0 rgba(0, 0, 0, 0.06), 0 1px 2px 0 rgba(0, 0, 0, 0.04)",
        popover: "0 4px 16px 0 rgba(0, 0, 0, 0.08)",
        // Paper-cut hierarchy — real layered paper shadows (theme variables).
        "paper-sm": "var(--shadow-paper-sm)",
        "paper-md": "var(--shadow-paper-md)",
        "paper-lg": "var(--shadow-paper-lg)",
        "paper-xl": "var(--shadow-paper-xl)",
      },
    },
  },
  plugins: [require("tailwindcss-animate"), require("@tailwindcss/typography")],
};

export default config;
