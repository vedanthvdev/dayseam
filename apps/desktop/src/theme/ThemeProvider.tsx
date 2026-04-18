import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ThemeContext,
  THEME_STORAGE_KEY,
  type ResolvedTheme,
  type Theme,
  type ThemeContextValue,
} from "./ThemeContext";

const VALID_THEMES: readonly Theme[] = ["light", "dark", "system"];

function isTheme(value: unknown): value is Theme {
  return (
    typeof value === "string" && (VALID_THEMES as readonly string[]).includes(value)
  );
}

function readInitialTheme(): Theme {
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

function resolveTheme(theme: Theme): ResolvedTheme {
  if (theme !== "system") return theme;
  if (typeof window === "undefined") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function applyResolvedTheme(resolved: ResolvedTheme) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.classList.toggle("dark", resolved === "dark");
  root.setAttribute("data-theme", resolved);
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

  const value = useMemo<ThemeContextValue>(
    () => ({ theme, resolvedTheme, setTheme }),
    [theme, resolvedTheme, setTheme],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}
