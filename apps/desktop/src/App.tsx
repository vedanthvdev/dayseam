import { useCallback, useEffect, useMemo, useState } from "react";
import { Footer } from "./components/Footer";
import { LogDrawer } from "./components/LogDrawer";
import { TitleBar } from "./components/TitleBar";
import { ToastHost } from "./components/ToastHost";
import { IdentityManagerDialog } from "./features/identities/IdentityManagerDialog";
import { ActionRow } from "./features/report/ActionRow";
import { SaveReportDialog } from "./features/report/SaveReportDialog";
import { StreamingPreview } from "./features/report/StreamingPreview";
import { SinksDialog } from "./features/sinks/SinksDialog";
import { SourcesSidebar } from "./features/sources/SourcesSidebar";
import { useReport } from "./ipc";
import { dismissSplash } from "./splash";
import { ThemeProvider } from "./theme";

function lastProgressMessage(
  progress: ReturnType<typeof useReport>["progress"],
): string | null {
  const last = progress[progress.length - 1];
  if (!last) return null;
  const phase = last.phase;
  if ("message" in phase) return phase.message;
  return null;
}

export default function App() {
  const [logsOpen, setLogsOpen] = useState(false);
  const [identitiesOpen, setIdentitiesOpen] = useState(false);
  const [sinksOpen, setSinksOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);

  const report = useReport();

  const toggleLogs = useCallback(() => setLogsOpen((prev) => !prev), []);
  const closeLogs = useCallback(() => setLogsOpen(false), []);

  const progressMessage = useMemo(
    () => lastProgressMessage(report.progress),
    [report.progress],
  );

  useEffect(() => {
    dismissSplash();
  }, []);

  // ⌘L (macOS) / Ctrl+L (Linux/Windows) toggles the log drawer.
  // Tauri already blocks the browser's "focus address bar" default
  // for Ctrl+L inside a webview, so we only need to guard against
  // our own listener firing when a text field is focused.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const isMod = event.metaKey || event.ctrlKey;
      if (!isMod || event.key.toLowerCase() !== "l") return;
      const target = event.target as HTMLElement | null;
      if (target && /^(input|textarea|select)$/i.test(target.tagName)) return;
      event.preventDefault();
      toggleLogs();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [toggleLogs]);

  const handleGenerate = useCallback(
    (date: string, sourceIds: string[]) => {
      void report.generate(date, sourceIds).catch(() => {
        /* surfaced via report.error and StreamingPreview */
      });
    },
    [report],
  );

  const handleCancel = useCallback(() => {
    void report.cancel();
  }, [report]);

  const saveEnabled = report.status === "completed" && report.draft !== null;

  return (
    <ThemeProvider>
      <div className="flex min-h-screen flex-col bg-white text-neutral-900 dark:bg-neutral-950 dark:text-neutral-100">
        <TitleBar />
        <ActionRow
          status={report.status}
          progressMessage={progressMessage}
          onGenerate={handleGenerate}
          onCancel={handleCancel}
        />
        <SourcesSidebar />
        <StreamingPreview
          status={report.status}
          progress={report.progress}
          draft={report.draft}
          error={report.error}
        />
        <Footer
          onOpenLogs={toggleLogs}
          onOpenIdentities={() => setIdentitiesOpen(true)}
          onOpenSinks={() => setSinksOpen(true)}
          onOpenSave={saveEnabled ? () => setSaveOpen(true) : undefined}
        />
      </div>
      <LogDrawer
        open={logsOpen}
        onClose={closeLogs}
        currentRunId={report.runId}
        liveLogs={report.logs}
      />
      <IdentityManagerDialog
        open={identitiesOpen}
        onClose={() => setIdentitiesOpen(false)}
      />
      <SinksDialog open={sinksOpen} onClose={() => setSinksOpen(false)} />
      <SaveReportDialog
        open={saveOpen}
        onClose={() => setSaveOpen(false)}
        hasDraft={saveEnabled}
        onSave={(sinkId) => report.save(sinkId)}
      />
      <ToastHost />
    </ThemeProvider>
  );
}
