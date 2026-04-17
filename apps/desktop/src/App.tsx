import { useEffect, useState } from "react";

type Theme = "light" | "dark" | "system";

const THEME_STORAGE_KEY = "dayseam:theme";

function applyTheme(theme: Theme) {
  const root = document.documentElement;
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const effective = theme === "system" ? (prefersDark ? "dark" : "light") : theme;
  root.classList.toggle("dark", effective === "dark");
}

function readInitialTheme(): Theme {
  const stored = localStorage.getItem(THEME_STORAGE_KEY);
  if (stored === "light" || stored === "dark" || stored === "system") {
    return stored;
  }
  return "system";
}

export default function App() {
  const [theme, setTheme] = useState<Theme>(readInitialTheme);

  useEffect(() => {
    applyTheme(theme);
    localStorage.setItem(THEME_STORAGE_KEY, theme);
  }, [theme]);

  useEffect(() => {
    if (theme !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const listener = () => applyTheme("system");
    media.addEventListener("change", listener);
    return () => media.removeEventListener("change", listener);
  }, [theme]);

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-6 px-8">
      <h1 className="text-4xl font-semibold tracking-tight">Dayseam</h1>
      <p className="max-w-md text-center text-sm text-neutral-500 dark:text-neutral-400">
        Scaffold ready. Connectors, reports, and sinks land in later Phase 1 tasks.
      </p>
      <div
        role="radiogroup"
        aria-label="Theme"
        className="inline-flex rounded-md border border-neutral-300 p-1 text-sm dark:border-neutral-700"
      >
        {(["light", "system", "dark"] as const).map((option) => (
          <button
            key={option}
            role="radio"
            aria-checked={theme === option}
            onClick={() => setTheme(option)}
            className={
              "rounded px-3 py-1 transition " +
              (theme === option
                ? "bg-neutral-900 text-white dark:bg-neutral-100 dark:text-neutral-900"
                : "text-neutral-600 hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-neutral-100")
            }
          >
            {option[0]!.toUpperCase() + option.slice(1)}
          </button>
        ))}
      </div>
    </main>
  );
}
