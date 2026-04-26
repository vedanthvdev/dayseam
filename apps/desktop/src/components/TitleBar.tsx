import { DayseamMark } from "./DayseamMark";

/**
 * App title row — app name and subtitle only.
 *
 * DAY-130 moved the `<ThemeToggle />` out of this always-visible
 * header. Theme is set once and touched rarely; keeping the toggle
 * here wasted screen real estate on every render and pulled attention
 * away from primary surfaces. The control now lives in two places
 * instead: the native macOS *View > Theme* submenu (wired in
 * `main.rs`) and the *View* section of the Preferences dialog
 * (`features/preferences/PreferencesDialog.tsx`). Both entry points
 * drive the same `setTheme` path via `ThemeProvider`.
 *
 * DAY-170: the coloured Dayseam mark sits inline to the left of the
 * wordmark. It renders without the rounded-charcoal box that the OS
 * icon carries — the background-less `DayseamMark` component
 * composites the strands directly onto the title-bar surface so the
 * mark is the one piece of Dayseam-branded colour in the chrome,
 * matching the rule we apply to connector logos ("only source /
 * brand marks wear colour").
 *
 * Purely presentational; no IPC.
 */
export function TitleBar() {
  return (
    <header className="flex items-center justify-between border-b border-neutral-200 px-6 py-4 dark:border-neutral-800">
      <div className="flex items-center gap-3">
        <DayseamMark size={28} className="shrink-0" />
        <div className="flex flex-col gap-0.5">
          <h1 className="text-xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-50">
            Dayseam
          </h1>
          <p className="text-xs text-neutral-500 dark:text-neutral-400">
            Local-first automated work reporting · early scaffold
          </p>
        </div>
      </div>
    </header>
  );
}
