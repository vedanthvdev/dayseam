import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Footer } from "./components/Footer";
import { LogDrawer } from "./components/LogDrawer";
import { TitleBar } from "./components/TitleBar";
import { ToastHost } from "./components/ToastHost";
import { IdentityManagerDialog } from "./features/identities/IdentityManagerDialog";
import { FirstRunEmptyState } from "./features/onboarding/FirstRunEmptyState";
import { useSetupChecklist } from "./features/onboarding/useSetupChecklist";
import { PreferencesDialog } from "./features/preferences/PreferencesDialog";
import { ActionRow } from "./features/report/ActionRow";
import { SaveReportDialog } from "./features/report/SaveReportDialog";
import { StreamingPreview } from "./features/report/StreamingPreview";
import { SchedulerCatchUpBanner } from "./features/scheduler/SchedulerCatchUpBanner";
import {
  OPEN_PREFERENCES_EVENT,
  useScheduler,
} from "./features/scheduler/useScheduler";
import { SinksDialog } from "./features/sinks/SinksDialog";
import { SourcesSidebar } from "./features/sources/SourcesSidebar";
import { UpdaterBanner } from "./features/updater/UpdaterBanner";
import { useUpdater } from "./features/updater/useUpdater";
import { useReport } from "./ipc";
import { dismissSplash } from "./splash";
import { ThemeProvider } from "./theme";

export default function App() {
  const [logsOpen, setLogsOpen] = useState(false);
  const [identitiesOpen, setIdentitiesOpen] = useState(false);
  const [sinksOpen, setSinksOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);
  const [preferencesOpen, setPreferencesOpen] = useState(false);

  // The setup checklist gates the main layout. We call the hook
  // unconditionally so the same instance drives the gate decision and
  // the `FirstRunEmptyState` content, and so the main layout never
  // remounts when the user completes the final checklist step — only
  // the conditional subtree swaps.
  const setupChecklist = useSetupChecklist();

  // One updater lifecycle per mounted shell. The hook fires a single
  // `check()` on mount and holds the resulting `Update` resource
  // until install-and-relaunch; `<UpdaterBanner />` renders the
  // current slice of state. Mounted above both the onboarding and
  // the main shells so a user who never gets past first-run still
  // sees available upgrades.
  const updater = useUpdater();

  const report = useReport();
  // DAY-130: one scheduler hook instance drives both the banner
  // above the main layout and the Scheduler section of
  // `PreferencesDialog`. Mounted at the `App` root so the banner
  // can fire the moment the Rust cold-start scan emits
  // `scheduler:catch-up-suggested` — including during onboarding.
  const scheduler = useScheduler();

  const toggleLogs = useCallback(() => setLogsOpen((prev) => !prev), []);
  const closeLogs = useCallback(() => setLogsOpen(false), []);

  useEffect(() => {
    dismissSplash();
  }, []);

  // DAY-130: the native *Dayseam > Preferences…* menu item emits
  // `menu://open-preferences` (see `apps/desktop/src-tauri/src/main.rs`).
  // Listening here keeps the dialog reachable via Cmd+, even on
  // the onboarding screen, and routes the menu event through the
  // same `setPreferencesOpen` the footer button uses.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen(OPEN_PREFERENCES_EVENT, () => {
      setPreferencesOpen(true);
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {
        // No Tauri bridge in tests; the footer button still works.
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
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

  // Gate: while setup is incomplete, the main layout is replaced by
  // the full-screen first-run empty state. We still render the
  // `ToastHost` and the title bar so the chrome is consistent (and so
  // "restart required" toasts from `sources_add` are visible during
  // onboarding); everything else is swapped out.
  if (!setupChecklist.complete && !setupChecklist.loading) {
    return (
      <ThemeProvider>
        {/* DOGFOOD-v0.4-06: bound the shell to the viewport (`h-dvh` +
            `overflow-hidden`) so the column height is exact, not "at
            least". Without a hard bound, a tall `FirstRunEmptyState`
            pushes the window's scrollbar to the body and any future
            footer would fall below the fold. */}
        <div
          data-testid="app-shell"
          className="flex h-dvh flex-col overflow-hidden bg-white text-neutral-900 dark:bg-neutral-950 dark:text-neutral-100"
        >
          <TitleBar />
          <UpdaterBanner state={updater} />
          <SchedulerCatchUpBanner scheduler={scheduler} />
          <FirstRunEmptyState checklist={setupChecklist} />
        </div>
        <PreferencesDialog
          open={preferencesOpen}
          onClose={() => setPreferencesOpen(false)}
        />
        <ToastHost />
      </ThemeProvider>
    );
  }

  return (
    <ThemeProvider>
      {/* DOGFOOD-v0.4-06: shell is `h-dvh` + `overflow-hidden`; the
          only scrollable child is `StreamingPreview`'s inner
          `<section>` (which carries `flex-1 min-h-0 overflow-y-auto`).
          That keeps the `<Footer>` pinned to the bottom strip on long
          reports instead of sliding off when the draft outgrows the
          viewport. */}
      <div
        data-testid="app-shell"
        className="flex h-dvh flex-col overflow-hidden bg-white text-neutral-900 dark:bg-neutral-950 dark:text-neutral-100"
      >
        <TitleBar />
        <UpdaterBanner state={updater} />
        <SchedulerCatchUpBanner scheduler={scheduler} />
        <ActionRow
          status={report.status}
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
          onOpenPreferences={() => setPreferencesOpen(true)}
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
      <PreferencesDialog
        open={preferencesOpen}
        onClose={() => setPreferencesOpen(false)}
      />
      <ToastHost />
    </ThemeProvider>
  );
}
