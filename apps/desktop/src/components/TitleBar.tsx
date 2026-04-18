import { ThemeToggle } from "./ThemeToggle";

/**
 * App title row — app name on the left, theme toggle on the right.
 *
 * Purely presentational; no IPC. The subtitle sets expectations that
 * the app is early-stage so the disabled-affordances below don't look
 * like broken controls.
 */
export function TitleBar() {
  return (
    <header className="flex items-center justify-between border-b border-neutral-200 px-6 py-4 dark:border-neutral-800">
      <div className="flex flex-col gap-0.5">
        <h1 className="text-xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-50">
          Dayseam
        </h1>
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          Local-first automated work reporting — early scaffold
        </p>
      </div>
      <ThemeToggle />
    </header>
  );
}
