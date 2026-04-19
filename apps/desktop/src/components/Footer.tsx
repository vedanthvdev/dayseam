interface FooterProps {
  onOpenLogs?: () => void;
  onOpenIdentities?: () => void;
  onOpenSinks?: () => void;
  /** Optional — renders a "Save report" entry once a completed draft
   *  is available. Hidden while no draft exists so the status bar
   *  doesn't advertise an action that can't succeed. */
  onOpenSave?: () => void;
}

/**
 * Status footer — occupies the bottom strip of the window. Phase 1
 * shows the app is idle; Phase 2 replaces the text with live sync
 * progress from the log drawer and Task 9's toast system. Phase 1 also
 * exposes the "Logs" toggle that opens `LogDrawer`, and Phase 2 adds
 * entry points for the admin dialogs (identities, sinks) alongside
 * it.
 */
export function Footer({
  onOpenLogs,
  onOpenIdentities,
  onOpenSinks,
  onOpenSave,
}: FooterProps) {
  return (
    <footer
      aria-label="Status"
      className="flex items-center justify-between border-t border-neutral-200 px-6 py-2 text-xs text-neutral-500 dark:border-neutral-800 dark:text-neutral-400"
    >
      <div className="flex items-center gap-3">
        <span>Idle</span>
        {onOpenLogs ? (
          <button
            type="button"
            onClick={onOpenLogs}
            title="Open activity log (⌘L)"
            className="rounded px-2 py-0.5 text-xs text-neutral-600 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
          >
            Logs
          </button>
        ) : null}
        {onOpenIdentities ? (
          <button
            type="button"
            onClick={onOpenIdentities}
            title="Manage identity mappings"
            className="rounded px-2 py-0.5 text-xs text-neutral-600 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
          >
            Identities
          </button>
        ) : null}
        {onOpenSinks ? (
          <button
            type="button"
            onClick={onOpenSinks}
            title="Manage sinks"
            className="rounded px-2 py-0.5 text-xs text-neutral-600 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
          >
            Sinks
          </button>
        ) : null}
        {onOpenSave ? (
          <button
            type="button"
            onClick={onOpenSave}
            title="Save the current draft to a sink"
            data-testid="footer-save"
            className="rounded bg-neutral-900 px-2 py-0.5 text-xs font-medium text-white hover:bg-neutral-800 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-white"
          >
            Save report
          </button>
        ) : null}
      </div>
      <span>Local-only · No data leaves this machine</span>
    </footer>
  );
}

export default Footer;
