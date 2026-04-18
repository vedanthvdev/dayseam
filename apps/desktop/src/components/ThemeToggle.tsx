import { useTheme, type Theme } from "../theme";

const OPTIONS: readonly { value: Theme; label: string; help: string }[] = [
  { value: "light", label: "Light", help: "Force light theme" },
  {
    value: "system",
    label: "System",
    help: "Follow the operating-system appearance",
  },
  { value: "dark", label: "Dark", help: "Force dark theme" },
];

/**
 * Segmented radio control for the theme preference. Keeps the System
 * option in the middle so users reading left-to-right see the
 * least-opinionated default first.
 */
export function ThemeToggle() {
  const { theme, setTheme } = useTheme();

  return (
    <div
      role="radiogroup"
      aria-label="Theme"
      className="inline-flex rounded-md border border-neutral-300 p-0.5 text-xs dark:border-neutral-700"
    >
      {OPTIONS.map((option) => {
        const selected = theme === option.value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            title={option.help}
            onClick={() => setTheme(option.value)}
            className={
              "rounded px-2.5 py-1 transition " +
              (selected
                ? "bg-neutral-900 text-white dark:bg-neutral-100 dark:text-neutral-900"
                : "text-neutral-600 hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-neutral-100")
            }
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
