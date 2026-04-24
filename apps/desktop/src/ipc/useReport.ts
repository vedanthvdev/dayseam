// React binding for the `report_*` IPC surface.
//
// The hook drives the full generate-report lifecycle from a single
// hook instance: call `generate(date, sourceIds)`, receive the
// typed `Channel<ProgressEvent>` / `Channel<LogEvent>` streams as
// React state, listen for the `report:completed` Tauri window event
// to pick up the final `draft_id`, and expose `cancel()` and
// `save(sinkId)` helpers. The shape deliberately mirrors the
// Phase-1 `useRunStreams` so a future refactor can merge them if
// that ends up being the right abstraction.
//
// Stale-run protection: each `generate()` call swaps a fresh pair
// of channels into `currentRef` and the `onmessage` callbacks gate
// on identity. A second `generate()` before the previous run
// finishes will start a fresh run on the Rust side and ignore any
// late events from the old one.

import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  LogEvent,
  ProgressEvent,
  ReportCompletedEvent,
  ReportDraft,
  RunId,
  SyncRunStatus,
  WriteReceipt,
} from "@dayseam/ipc-types";
import { Channel, invoke } from "./invoke";

/** Name of the window event fired once a run reaches a terminal
 *  [`SyncRunStatus`]. Matches `REPORT_COMPLETED_EVENT` on the Rust
 *  side of `ipc/commands.rs`. */
export const REPORT_COMPLETED_EVENT = "report:completed";

export type ReportStatus =
  | "idle"
  | "starting"
  | "running"
  | "completed"
  | "cancelled"
  | "failed";

export interface ReportState {
  runId: RunId | null;
  status: ReportStatus;
  progress: ProgressEvent[];
  logs: LogEvent[];
  /** The draft fetched after the run completes. `null` while the
   *  run is still in-flight, during cancel/fail, and before the
   *  first `generate()` call. */
  draft: ReportDraft | null;
  error: string | null;
}

const INITIAL: ReportState = {
  runId: null,
  status: "idle",
  progress: [],
  logs: [],
  draft: null,
  error: null,
};

function deriveStatusFromProgress(events: ProgressEvent[]): ReportStatus {
  // DAY-128 #2: connectors emit `ProgressPhase::Completed` when
  // their per-source walk finishes (see e.g. `connector-gitlab`,
  // `connector-jira`, `connector-github`). If we propagate every
  // terminal phase to the top-level run status the Cancel button
  // flips back to Generate the instant one source finishes and
  // then back to Cancel as the next source starts — which is the
  // "Generate button glitch mid-run" the user is reporting on a
  // multi-source selection. Only the orchestrator's run-level
  // completion event carries `source_id === null`; that is the
  // canonical run terminal on the progress stream. All other
  // terminals are per-source and must not change the button.
  const last = events[events.length - 1];
  if (!last) return "starting";
  const phase = last.phase;
  const isRunLevel = last.source_id === null;
  switch (phase.status) {
    case "starting":
      return "starting";
    case "in_progress":
      return "running";
    case "completed":
      return isRunLevel ? "completed" : "running";
    case "cancelled":
      return isRunLevel ? "cancelled" : "running";
    case "failed":
      return isRunLevel ? "failed" : "running";
    default:
      return "running";
  }
}

function statusFromSyncRunStatus(status: SyncRunStatus): ReportStatus {
  switch (status) {
    case "Completed":
      return "completed";
    case "Cancelled":
      return "cancelled";
    case "Failed":
      return "failed";
    default:
      return "running";
  }
}

export interface UseReportState extends ReportState {
  generate: (
    date: string,
    sourceIds: string[],
    templateId?: string | null,
  ) => Promise<RunId>;
  cancel: () => Promise<void>;
  save: (sinkId: string) => Promise<WriteReceipt[]>;
  reset: () => void;
}

export function useReport(): UseReportState {
  const [state, setState] = useState<ReportState>(INITIAL);
  const currentRef = useRef<{
    runId: RunId | null;
    progress: Channel<ProgressEvent>;
    logs: Channel<LogEvent>;
  } | null>(null);
  // `save()` needs the latest draft id synchronously without
  // blocking on a React render cycle; ref mirrors whatever the
  // completion listener last wrote into state.draft.
  const draftIdRef = useRef<string | null>(null);

  // One listener per mount pipes `report:completed` into the hook.
  // We keep it here rather than in `generate()` so we don't miss
  // the completion of a run that finishes between the invoke
  // returning and the listener being attached.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<ReportCompletedEvent>(REPORT_COMPLETED_EVENT, (evt) => {
      const payload = evt.payload;
      if (
        currentRef.current?.runId &&
        payload.run_id !== currentRef.current.runId
      ) {
        return;
      }
      setState((prev) => ({
        ...prev,
        status: statusFromSyncRunStatus(payload.status),
      }));
      if (payload.draft_id) {
        const draftId = payload.draft_id;
        draftIdRef.current = draftId;
        void invoke("report_get", { draftId })
          .then((draft) => {
            setState((prev) => ({ ...prev, draft }));
          })
          .catch((err) => {
            setState((prev) => ({
              ...prev,
              error: err instanceof Error ? err.message : JSON.stringify(err),
            }));
          });
      }
    })
      .then((fn) => {
        if (cancelled) {
          fn();
          return;
        }
        unlisten = fn;
      })
      .catch((err) => {
        console.warn("dayseam report listener failed to attach", err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const reset = useCallback(() => {
    currentRef.current = null;
    draftIdRef.current = null;
    setState(INITIAL);
  }, []);

  const generate = useCallback(
    async (
      date: string,
      sourceIds: string[],
      templateId: string | null = null,
    ) => {
      const progress = new Channel<ProgressEvent>();
      const logs = new Channel<LogEvent>();
      currentRef.current = { runId: null, progress, logs };
      draftIdRef.current = null;

      setState({ ...INITIAL, status: "starting" });

      progress.onmessage = (event) => {
        if (currentRef.current?.progress !== progress) return;
        setState((prev) => {
          const next = [...prev.progress, event];
          return { ...prev, progress: next, status: deriveStatusFromProgress(next) };
        });
      };
      logs.onmessage = (event) => {
        if (currentRef.current?.logs !== logs) return;
        setState((prev) => ({ ...prev, logs: [...prev.logs, event] }));
      };

      try {
        const runId = await invoke("report_generate", {
          date,
          sourceIds,
          templateId,
          progress,
          logs,
        });
        if (currentRef.current?.progress === progress) {
          currentRef.current.runId = runId;
        }
        setState((prev) => ({ ...prev, runId }));
        return runId;
      } catch (err) {
        const message = err instanceof Error ? err.message : JSON.stringify(err);
        setState((prev) => ({ ...prev, status: "failed", error: message }));
        throw err;
      }
    },
    [],
  );

  const cancel = useCallback(async () => {
    const runId = currentRef.current?.runId;
    if (!runId) return;
    await invoke("report_cancel", { runId });
  }, []);

  const save = useCallback(async (sinkId: string) => {
    const draftId = draftIdRef.current;
    if (!draftId) {
      throw new Error("no draft to save; generate a report first");
    }
    return await invoke("report_save", { draftId, sinkId });
  }, []);

  return { ...state, generate, cancel, save, reset };
}
