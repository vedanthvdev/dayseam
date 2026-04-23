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
 * Status footer — occupies the bottom strip of the window. Renders
 * entry points for the admin surfaces (Logs / Identities / Sinks) and
 * — once a completed draft is available — a "Save report" primary.
 *
 * DAY-119: the footer used to lead with a literal `"Idle"` label as a
 * placeholder for live sync progress. In practice the streaming
 * progress now lives inside `StreamingPreview`'s determinate bar
 * (DAY-104) and the toast host shows anything out-of-band, so the
 * static `"Idle"` became chrome that never changed state and only
 * added visual noise next to the action buttons. Dropping it leaves
 * the left cluster as the action buttons themselves; the right cluster
 * keeps the local-only reassurance text.
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
