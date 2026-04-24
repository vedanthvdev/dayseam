import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ThemeContext,
  THEME_STORAGE_KEY,
  type ResolvedTheme,
  type Theme,
  type ThemeContextValue,
} from "./ThemeContext";
import { applyResolvedTheme, readInitialTheme, resolveTheme } from "./theme-logic";

// DAY-130: the native *View > Theme* submenu emits this event with a
// `"light" | "system" | "dark"` payload. Centralised here so
// `PreferencesDialog` and future entry points reuse the same string.
const VIEW_SET_THEME_EVENT = "view:set-theme";

function isTheme(value: unknown): value is Theme {
  return value === "light" || value === "system" || value === "dark";
}

export interface ThemeProviderProps {
  children: ReactNode;
  /**
   * Override the initial theme — only used by tests that want a known
   * starting state without touching `localStorage`.
   */
  initialTheme?: Theme;
}

export function ThemeProvider({ children, initialTheme }: ThemeProviderProps) {
  const [theme, setThemeState] = useState<Theme>(
    () => initialTheme ?? readInitialTheme(),
  );
  const [resolvedTheme, setResolvedTheme] = useState<ResolvedTheme>(() =>
    resolveTheme(initialTheme ?? readInitialTheme()),
  );

  // Track the last system-resolved value so a system→system reconcile
  // from `matchMedia` doesn't pointlessly re-render the whole tree.
  const lastSystemResolvedRef = useRef<ResolvedTheme | null>(null);

  useEffect(() => {
    const nextResolved = resolveTheme(theme);
    setResolvedTheme(nextResolved);
    applyResolvedTheme(nextResolved);
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch {
      // Ignore — persistence is best-effort.
    }
  }, [theme]);

  useEffect(() => {
    if (theme !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const listener = () => {
      const next: ResolvedTheme = media.matches ? "dark" : "light";
      if (lastSystemResolvedRef.current === next) return;
      lastSystemResolvedRef.current = next;
      setResolvedTheme(next);
      applyResolvedTheme(next);
    };
    media.addEventListener("change", listener);
    return () => media.removeEventListener("change", listener);
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
  }, []);

  // DAY-130: the native *View > Theme* submenu (wired in `main.rs`)
  // emits `view:set-theme` with the user's choice. We listen once at
  // provider mount and delegate to the existing `setTheme` path so
  // the menu and the Preferences dialog produce identical state
  // transitions. If `@tauri-apps/api/event` is unavailable (test
  // harness, browser fallback) we silently skip registering — the
  // in-app controls still work.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<string>(VIEW_SET_THEME_EVENT, (event) => {
      if (isTheme(event.payload)) {
        setThemeState(event.payload);
      }
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {
        // No Tauri bridge; the Preferences UI still drives setTheme.
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ theme, resolvedTheme, setTheme }),
    [theme, resolvedTheme, setTheme],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}
