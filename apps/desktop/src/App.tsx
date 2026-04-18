import { ActionBar } from "./components/ActionBar";
import { Footer } from "./components/Footer";
import { ReportPreview } from "./components/ReportPreview";
import { TitleBar } from "./components/TitleBar";
import { ThemeProvider } from "./theme";

const SOURCE_PLACEHOLDERS = [
  { id: "local-git", label: "Local git" },
  { id: "gitlab", label: "GitLab" },
] as const;

/**
 * Static sources row. Phase 2 wires the cards to the `sources.list`
 * IPC command, at which point each card becomes a sync-status tile.
 */
function SourcesRow() {
  return (
    <section
      aria-label="Connected sources"
      className="flex flex-wrap items-center gap-2 border-b border-neutral-200 px-6 py-3 dark:border-neutral-800"
    >
      <span className="text-xs uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
        Sources
      </span>
      {SOURCE_PLACEHOLDERS.map((source) => (
        <span
          key={source.id}
          title="Connect flow lands in Phase 2."
          className="inline-flex items-center gap-1.5 rounded border border-dashed border-neutral-300 px-2 py-0.5 text-xs text-neutral-500 dark:border-neutral-700 dark:text-neutral-400"
        >
          <span
            aria-hidden="true"
            className="h-1.5 w-1.5 rounded-full bg-neutral-300 dark:bg-neutral-600"
          />
          {source.label}
          <span className="sr-only"> — not connected</span>
        </span>
      ))}
      <span className="ml-auto text-xs text-neutral-400 dark:text-neutral-500">
        No sources connected
      </span>
    </section>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <div className="flex min-h-screen flex-col bg-white text-neutral-900 dark:bg-neutral-950 dark:text-neutral-100">
        <TitleBar />
        <ActionBar />
        <SourcesRow />
        <ReportPreview />
        <Footer />
      </div>
    </ThemeProvider>
  );
}
