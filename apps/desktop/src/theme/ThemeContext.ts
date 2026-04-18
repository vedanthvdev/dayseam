import { createContext } from "react";

export type Theme = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "dayseam:theme";

export interface ThemeContextValue {
  /** User-selected theme (what the UI radio group should reflect). */
  theme: Theme;
  /**
   * The theme actually applied to the document — `system` resolves to
   * `light` or `dark` based on `prefers-color-scheme`. Components that
   * need to show "currently dark" UI should read this, not `theme`.
   */
  resolvedTheme: ResolvedTheme;
  setTheme: (next: Theme) => void;
}

// Split from `ThemeProvider.tsx` so the component file only exports
// components — this keeps Vite's React Fast Refresh happy.
export const ThemeContext = createContext<ThemeContextValue | null>(null);
