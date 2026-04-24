// Thin strip under the title bar that surfaces update state.
//
// The banner is deliberately non-blocking: it never intercepts
// keyboard focus, never opens a modal, and renders as a single
// horizontal row so the underlying report workflow is never
// obscured. That matches the DOGFOOD-v0.4-06 "pin chrome, flex
// content" layout the rest of the shell already follows.
//
// Visibility rules (see the switch at the bottom):
//
//   - `idle` — renders nothing.
//   - `checking` / `up-to-date` — renders nothing *unless* the
//     hook is in `verbose` mode (i.e. the user triggered the
//     check from the native "Check for Updates…" menu). Silent
//     mount-time checks must not flicker the banner on launch,
//     but a user-initiated check has to give visible feedback
//     (DAY-127 #3) — "Checking…" while the IPC is outstanding,
//     "Dayseam is up to date." on resolution, which self-
//     dismisses after a few seconds via the hook.
//   - `available` AND version not on the skip list — renders the
//     upgrade prompt with Install / Skip actions.
//   - `downloading` / `ready` — renders the progress / restart
//     state regardless of skip status (the user has already
//     opted in).
//   - `error` — renders a compact "couldn't check" row with a
//     Retry button, so a flaky network after install doesn't
//     leave the banner wedged.

import { useCallback } from "react";
import type { UpdaterState, UpdaterStatus } from "./useUpdater";

interface Props {
  state: UpdaterState;
}

function StatusBar({
  tone,
  children,
  testId,
}: {
  tone: "info" | "warning";
  children: React.ReactNode;
  testId: string;
}) {
  const toneClasses =
    tone === "info"
      ? "bg-blue-50 text-blue-900 dark:bg-blue-950 dark:text-blue-100 border-blue-200 dark:border-blue-900"
      : "bg-amber-50 text-amber-900 dark:bg-amber-950 dark:text-amber-100 border-amber-200 dark:border-amber-900";
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid={testId}
      className={`flex items-center gap-3 border-b px-4 py-2 text-sm ${toneClasses}`}
    >
      {children}
    </div>
  );
}

function AvailableRow({
  status,
  onInstall,
  onSkip,
}: {
  status: Extract<UpdaterStatus, { kind: "available" }>;
  onInstall: () => void;
  onSkip: () => void;
}) {
  return (
    <StatusBar tone="info" testId="updater-banner-available">
      <span className="flex-1">
        <strong>Dayseam {status.version}</strong> is available
        {status.currentVersion ? (
          <span className="text-blue-700 dark:text-blue-300">
            {" "}
            (you have {status.currentVersion})
          </span>
        ) : null}
        .
      </span>
      <button
        type="button"
        onClick={onInstall}
        className="rounded bg-blue-600 px-3 py-1 text-xs font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-1"
      >
        Install and restart
      </button>
      <button
        type="button"
        onClick={onSkip}
        className="rounded border border-blue-300 px-3 py-1 text-xs font-medium text-blue-900 hover:bg-blue-100 dark:border-blue-800 dark:text-blue-100 dark:hover:bg-blue-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-1"
      >
        Skip this version
      </button>
    </StatusBar>
  );
}

function DownloadingRow({
  status,
}: {
  status: Extract<UpdaterStatus, { kind: "downloading" }>;
}) {
  const label =
    status.percent === null
      ? `Downloading Dayseam ${status.version}…`
      : `Downloading Dayseam ${status.version} — ${status.percent}%`;
  return (
    <StatusBar tone="info" testId="updater-banner-downloading">
      <span className="flex-1">{label}</span>
      <div
        role="progressbar"
        aria-label={`Update download progress for ${status.version}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={status.percent ?? undefined}
        className="h-2 w-24 overflow-hidden rounded bg-blue-200 dark:bg-blue-900"
      >
        <div
          className="h-full bg-blue-600 transition-[width] duration-150"
          style={{
            width: status.percent === null ? "25%" : `${status.percent}%`,
          }}
        />
      </div>
    </StatusBar>
  );
}

function ReadyRow({
  status,
}: {
  status: Extract<UpdaterStatus, { kind: "ready" }>;
}) {
  return (
    <StatusBar tone="info" testId="updater-banner-ready">
      <span className="flex-1">
        Dayseam {status.version} installed. Restarting…
      </span>
    </StatusBar>
  );
}

function CheckingRow() {
  return (
    <StatusBar tone="info" testId="updater-banner-checking">
      <span className="flex-1">Checking for updates…</span>
      <span
        aria-hidden="true"
        className="inline-block h-3 w-3 rounded-full border-2 border-blue-300 border-t-blue-700 motion-safe:animate-spin dark:border-blue-700 dark:border-t-blue-200"
      />
    </StatusBar>
  );
}

function UpToDateRow() {
  return (
    <StatusBar tone="info" testId="updater-banner-up-to-date">
      <span className="flex-1">Dayseam is up to date.</span>
    </StatusBar>
  );
}

function ErrorRow({
  status,
  onRetry,
}: {
  status: Extract<UpdaterStatus, { kind: "error" }>;
  onRetry: () => void;
}) {
  return (
    <StatusBar tone="warning" testId="updater-banner-error">
      <span className="flex-1">
        Couldn't check for updates: {status.message}
      </span>
      <button
        type="button"
        onClick={onRetry}
        className="rounded border border-amber-400 px-3 py-1 text-xs font-medium text-amber-900 hover:bg-amber-100 dark:border-amber-700 dark:text-amber-100 dark:hover:bg-amber-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-amber-500 focus-visible:ring-offset-1"
      >
        Retry
      </button>
    </StatusBar>
  );
}

export function UpdaterBanner({ state }: Props) {
  const { status, install, check, skipCurrent, isCurrentSkipped, verbose } =
    state;

  const handleInstall = useCallback(() => {
    void install().catch(() => {
      // The hook already maps errors into `status = error`; the
      // banner re-renders from state, so nothing else to do here.
    });
  }, [install]);

  const handleCheck = useCallback(() => {
    void check().catch(() => {
      // Same contract as install — hook owns error state.
    });
  }, [check]);

  switch (status.kind) {
    case "available":
      if (isCurrentSkipped) return null;
      return (
        <AvailableRow
          status={status}
          onInstall={handleInstall}
          onSkip={skipCurrent}
        />
      );
    case "downloading":
      return <DownloadingRow status={status} />;
    case "ready":
      return <ReadyRow status={status} />;
    case "error":
      return <ErrorRow status={status} onRetry={handleCheck} />;
    case "checking":
      return verbose ? <CheckingRow /> : null;
    case "up-to-date":
      return verbose ? <UpToDateRow /> : null;
    case "idle":
    default:
      return null;
  }
}
