/**
 * Read-only preview of the generated draft. In Phase 1 this is a
 * deliberate empty state that explains why nothing is here yet — the
 * design doc's "never look broken, always tell the user what's
 * happening" principle means we do not render a blank white rectangle.
 */
export function ReportPreview() {
  return (
    <section
      aria-label="Report preview"
      className="flex flex-1 flex-col items-center justify-center gap-2 px-6 py-10 text-center"
    >
      <div
        aria-hidden="true"
        className="h-10 w-10 rounded-full border-2 border-dashed border-neutral-300 dark:border-neutral-700"
      />
      <h2 className="text-base font-medium text-neutral-700 dark:text-neutral-200">
        No report yet
      </h2>
      <p className="max-w-sm text-sm text-neutral-500 dark:text-neutral-400">
        Connect a source and pick a date in Phase 2 to generate your first
        draft. The preview, evidence, and export actions all live here.
      </p>
    </section>
  );
}
