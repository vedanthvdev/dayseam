/**
 * Status footer — occupies the bottom strip of the window. Phase 1
 * shows the app is idle; Phase 2 replaces the text with live sync
 * progress from the log drawer and Task 9's toast system.
 */
export function Footer() {
  return (
    <footer
      aria-label="Status"
      className="flex items-center justify-between border-t border-neutral-200 px-6 py-2 text-xs text-neutral-500 dark:border-neutral-800 dark:text-neutral-400"
    >
      <span>Idle</span>
      <span>Local-only · No data leaves this machine</span>
    </footer>
  );
}
