// Pure helpers for reading the user's theme preference, resolving
// `"system"` against `prefers-color-scheme`, and writing the result
// onto `<html>`. Factored out of `ThemeProvider.tsx` so the identical
// logic can run:
//
// 1. At the top of React's lifecycle via `ThemeProvider`.
// 2. Pre-paint from the external hydration script in
//    `apps/desktop/public/hydrate-theme.js`, which is parity-checked
//    against these helpers in `__tests__/hydrate-theme.test.ts`.
//
// If you change the behaviour here, `public/hydrate-theme.js` MUST
// change to match — the parity test fails the suite otherwise.

import { THEME_STORAGE_KEY, type ResolvedTheme, type Theme } from "./ThemeContext";

const VALID_THEMES: readonly Theme[] = ["light", "dark", "system"];

// Kept module-local: `readInitialTheme` is the only caller. If a
// future consumer needs this, promote it through the `theme/index.ts`
// barrel alongside the other public surface.
function isTheme(value: unknown): value is Theme {
  return (
    typeof value === "string" && (VALID_THEMES as readonly string[]).includes(value)
  );
}

/**
 * Read the stored theme from `localStorage`, falling back to
 * `"system"` on any failure (private browsing, storage disabled,
 * Tauri restricted contexts). Never throws.
 */
export function readInitialTheme(): Theme {
  if (typeof window === "undefined") return "system";
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (isTheme(stored)) return stored;
  } catch {
    // localStorage can throw in private-browsing or restricted Tauri
    // contexts; fall back to `system` and let the user pick again.
  }
  return "system";
}

/**
 * Collapse a `Theme` into the concrete `light` / `dark` that actually
 * gets painted. `system` consults `matchMedia`; everything else
 * passes through. Always returns a valid `ResolvedTheme`.
 */
export function resolveTheme(theme: Theme): ResolvedTheme {
  if (theme !== "system") return theme;
  if (typeof window === "undefined") return "light";
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  } catch {
    // `matchMedia` can throw in unusual WebView configurations; treat
    // that as "no preference = light" so we match the CSS default.
    return "light";
  }
}

/**
 * Apply the resolved theme to `<html>` as a `data-theme` attribute,
 * a `.dark` class (Tailwind's `dark:` variant relies on the class),
 * and the CSS `color-scheme` declaration. All three channels must
 * agree so that:
 *
 * - Tailwind variants flip (`.dark` class),
 * - existing attribute-based selectors still work (`data-theme`),
 * - native form controls (`<input type="date">` calendar popover,
 *   `<input type="color">`, scrollbars, etc.) are themed by the OS
 *   in the same mode as the rest of the app. Without `color-scheme`
 *   the WebView keeps drawing those in light mode even when every
 *   other surface is dark.
 */
export function applyResolvedTheme(resolved: ResolvedTheme): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.classList.toggle("dark", resolved === "dark");
  root.setAttribute("data-theme", resolved);
  root.style.colorScheme = resolved;
}
