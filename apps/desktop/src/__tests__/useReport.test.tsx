import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  LogEvent,
  ProgressEvent,
  ReportCompletedEvent,
  ReportDraft,
  WriteReceipt,
} from "@dayseam/ipc-types";
import { useReport, REPORT_COMPLETED_EVENT } from "../ipc/useReport";
import {
  emitEvent,
  getCreatedChannels,
  mockInvoke,
  registerInvokeHandler,
  resetTauriMocks,
} from "./tauri-mock";

const RUN_ID = "rrrrrrrr-rrrr-rrrr-rrrr-rrrrrrrrrrrr";
const DRAFT_ID = "dddddddd-dddd-dddd-dddd-dddddddddddd";

const DRAFT: ReportDraft = {
  id: DRAFT_ID,
  date: "2026-04-17",
  template_id: "eod",
  template_version: "1.0.0",
  sections: [],
  evidence: [],
  per_source_state: {},
  verbose_mode: false,
  generated_at: "2026-04-17T12:00:00Z",
};

describe("useReport", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  afterEach(() => {
    resetTauriMocks();
  });

  it("starts idle with no draft or runId", () => {
    const { result } = renderHook(() => useReport());
    expect(result.current.status).toBe("idle");
    expect(result.current.runId).toBeNull();
    expect(result.current.draft).toBeNull();
  });

  it("invokes `report_generate` and exposes the returned runId", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);

    const { result } = renderHook(() => useReport());

    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });

    expect(result.current.runId).toBe(RUN_ID);
    expect(mockInvoke).toHaveBeenCalledWith(
      "report_generate",
      expect.objectContaining({
        date: "2026-04-17",
        sourceIds: ["src-1"],
        templateId: null,
      }),
    );
  });

  it("accumulates progress + log events from the channels", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);

    const { result } = renderHook(() => useReport());

    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });

    // The `generate` call constructs progress + logs channels in that
    // order, so those are the two most-recent mock channels.
    const channels = getCreatedChannels();
    const progressCh = channels[channels.length - 2];
    const logsCh = channels[channels.length - 1];

    const progressEvent = {
      run_id: RUN_ID,
      phase: { name: "fetch", status: "in_progress" },
    } as unknown as ProgressEvent;
    const logEvent = {
      run_id: RUN_ID,
      level: "info",
      message: "hello",
    } as unknown as LogEvent;

    act(() => {
      progressCh?.deliver(progressEvent);
      logsCh?.deliver(logEvent);
    });

    expect(result.current.progress).toHaveLength(1);
    expect(result.current.logs).toHaveLength(1);
  });

  it("fetches the draft when `report:completed` fires with a draft id", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);
    registerInvokeHandler("report_get", async () => DRAFT);

    const { result } = renderHook(() => useReport());

    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });

    const completed: ReportCompletedEvent = {
      run_id: RUN_ID,
      status: "Completed",
      draft_id: DRAFT_ID,
      cancel_reason: null,
    };

    await act(async () => {
      emitEvent(REPORT_COMPLETED_EVENT, completed);
      // Allow the micro-task queued by `invoke("report_get")` to flush.
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(result.current.draft).not.toBeNull());
    expect(result.current.status).toBe("completed");
    expect(result.current.draft?.id).toBe(DRAFT_ID);
  });

  it("`save` refuses when no draft has completed yet", async () => {
    const { result } = renderHook(() => useReport());
    await expect(result.current.save("sink-1")).rejects.toThrow(
      /no draft to save/,
    );
  });

  it("`save` forwards draftId and sinkId after completion", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);
    registerInvokeHandler("report_get", async () => DRAFT);
    const receipts: WriteReceipt[] = [];
    registerInvokeHandler("report_save", async (args) => {
      expect(args).toEqual({ draftId: DRAFT_ID, sinkId: "sink-1" });
      return receipts;
    });

    const { result } = renderHook(() => useReport());
    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });

    await act(async () => {
      emitEvent(REPORT_COMPLETED_EVENT, {
        run_id: RUN_ID,
        status: "Completed",
        draft_id: DRAFT_ID,
        cancel_reason: null,
      } satisfies ReportCompletedEvent);
      await Promise.resolve();
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.draft).not.toBeNull());

    await act(async () => {
      const out = await result.current.save("sink-1");
      expect(out).toBe(receipts);
    });
  });

  it("`cancel` is a no-op while no runId has been assigned", async () => {
    const { result } = renderHook(() => useReport());
    await act(async () => {
      await result.current.cancel();
    });
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "report_cancel",
      expect.anything(),
    );
  });

  it("`cancel` calls `report_cancel` with the active runId", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);
    registerInvokeHandler("report_cancel", async () => undefined);

    const { result } = renderHook(() => useReport());
    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });
    await act(async () => {
      await result.current.cancel();
    });
    expect(mockInvoke).toHaveBeenCalledWith("report_cancel", { runId: RUN_ID });
  });

  // DAY-128 #2: connectors emit `ProgressPhase::Completed` when a
  // per-source walk finishes. Before this fix, `deriveStatusFromProgress`
  // turned those into the top-level `"completed"` status, which made
  // the Cancel button flip back to "Generate report" mid-run between
  // sources — the "generate button glitch" the user reported. The
  // status must only settle to a terminal value on a run-level
  // progress event (`source_id === null`) *or* on
  // `report:completed`; per-source terminals stay "running".
  it("keeps status running when a per-source Completed event fires mid-run", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);

    const { result } = renderHook(() => useReport());

    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1", "src-2"]);
    });

    const channels = getCreatedChannels();
    const progressCh = channels[channels.length - 2];

    const sourceACompleted = {
      run_id: RUN_ID,
      source_id: "src-1",
      phase: { status: "completed", message: "src-1 done" },
      emitted_at: "2026-04-17T12:00:00Z",
    } as unknown as ProgressEvent;

    act(() => {
      progressCh?.deliver(sourceACompleted);
    });

    expect(result.current.status).toBe("running");
  });

  it("settles to completed when a run-level (source_id=null) Completed fires", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);

    const { result } = renderHook(() => useReport());

    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });

    const channels = getCreatedChannels();
    const progressCh = channels[channels.length - 2];

    const runLevelCompleted = {
      run_id: RUN_ID,
      source_id: null,
      phase: { status: "completed", message: "run done" },
      emitted_at: "2026-04-17T12:00:00Z",
    } as unknown as ProgressEvent;

    act(() => {
      progressCh?.deliver(runLevelCompleted);
    });

    expect(result.current.status).toBe("completed");
  });

  it("`reset` clears the accumulated state", async () => {
    registerInvokeHandler("report_generate", async () => RUN_ID);

    const { result } = renderHook(() => useReport());
    await act(async () => {
      await result.current.generate("2026-04-17", ["src-1"]);
    });

    act(() => {
      result.current.reset();
    });

    expect(result.current.runId).toBeNull();
    expect(result.current.status).toBe("idle");
    expect(result.current.progress).toEqual([]);
  });
});
