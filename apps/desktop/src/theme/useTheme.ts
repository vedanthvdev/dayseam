import { useContext } from "react";
import { ThemeContext, type ThemeContextValue } from "./ThemeContext";

/**
 * Access the current theme selection + resolver + setter.
 *
 * Throws if called outside a `<ThemeProvider>` so misuses fail loudly
 * at mount rather than silently returning a stale fallback.
 */
export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) {
    throw new Error(
      "useTheme must be used inside a <ThemeProvider>. Wrap your tree in ThemeProvider in main.tsx or the test harness.",
    );
  }
  return ctx;
}
