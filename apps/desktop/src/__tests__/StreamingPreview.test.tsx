import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  ActivityEvent,
  ProgressEvent,
  ReportDraft,
} from "@dayseam/ipc-types";
import { StreamingPreview } from "../features/report/StreamingPreview";
import { registerInvokeHandler, resetTauriMocks } from "./tauri-mock";

function progress(
  completed: number,
  total: number | null,
  message = "scanning",
): ProgressEvent {
  return {
    run_id: "11111111-2222-3333-4444-555555555555",
    source_id: null,
    phase: { status: "in_progress", completed, total, message },
    emitted_at: "2026-04-17T12:00:00Z",
  };
}

const EVENT: ActivityEvent = {
  id: "ev-1",
  source_id: "src-1",
  external_id: "abc",
  kind: "GitCommit" as unknown as ActivityEvent["kind"],
  occurred_at: "2026-04-17T10:00:00Z",
  actor: { display_name: "Alice", email: null, external_id: null },
  title: "Fix the thing",
  body: null,
  links: [{ url: "https://example.com/x", label: "MR !1" }],
  entities: [],
  parent_external_id: null,
  metadata: {},
  raw_ref: { blob_hash: "deadbeef" } as unknown as ActivityEvent["raw_ref"],
  privacy: "Work" as unknown as ActivityEvent["privacy"],
};

const DRAFT: ReportDraft = {
  id: "draft-1",
  date: "2026-04-17",
  template_id: "eod",
  template_version: "1.0.0",
  sections: [
    {
      id: "completed",
      title: "Completed",
      bullets: [
        { id: "b1", text: "Shipped feature X" },
        { id: "b2", text: "Reviewed 3 MRs" },
      ],
    },
  ],
  evidence: [
    { bullet_id: "b1", event_ids: ["ev-1"], reason: "1 commit" },
  ],
  per_source_state: {},
  verbose_mode: false,
  generated_at: "2026-04-17T12:00:00Z",
};

describe("StreamingPreview", () => {
  beforeEach(() => {
    resetTauriMocks();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("renders the empty state before any run starts", () => {
    render(
      <StreamingPreview
        status="idle"
        progress={[]}
        draft={null}
        error={null}
      />,
    );
    expect(screen.getByText(/no report yet/i)).toBeInTheDocument();
  });

  it("renders a determinate progress bar when total is known", () => {
    render(
      <StreamingPreview
        status="running"
        progress={[progress(2, 4)]}
        draft={null}
        error={null}
      />,
    );
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "2");
    expect(bar).toHaveAttribute("aria-valuemax", "4");
    expect(screen.getByTestId("streaming-preview-progress-fill")).toHaveStyle({
      width: "50%",
    });
  });

  it("falls back to an indeterminate bar when total is unknown", () => {
    render(
      <StreamingPreview
        status="running"
        progress={[progress(5, null)]}
        draft={null}
        error={null}
      />,
    );
    const bar = screen.getByRole("progressbar");
    expect(bar).not.toHaveAttribute("aria-valuenow");
    expect(
      screen.queryByTestId("streaming-preview-progress-fill"),
    ).not.toBeInTheDocument();
  });

  it("surfaces generation errors in an alert region", () => {
    render(
      <StreamingPreview
        status="failed"
        progress={[]}
        draft={null}
        error="orchestrator exploded"
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      /orchestrator exploded/i,
    );
  });

  it("renders rendered sections and bullets from the finished draft", () => {
    render(
      <StreamingPreview
        status="completed"
        progress={[]}
        draft={DRAFT}
        error={null}
      />,
    );
    expect(screen.getByText("Completed")).toBeInTheDocument();
    expect(screen.getByTestId("bullet-b1")).toHaveTextContent(
      "Shipped feature X",
    );
    expect(screen.getByTestId("bullet-b2")).toBeDisabled();
  });

  it("hides the template version from the visible label so it cannot be confused with the report date", () => {
    // DAY-68 Phase 3 Task 8: users kept reading the
    // `template_version` (a YYYY-MM-DD schema revision) as the
    // content date because it sat right next to `{draft.date}` in
    // the header. The fix is to render only `template_id` visibly
    // and move the revision into the tooltip + a data attribute.
    render(
      <StreamingPreview
        status="completed"
        progress={[]}
        draft={DRAFT}
        error={null}
      />,
    );

    expect(screen.queryByText(/template v/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/1\.0\.0/)).not.toBeInTheDocument();

    const label = screen.getByText("eod");
    expect(label).toHaveAttribute("data-template-version", "1.0.0");
    expect(label.getAttribute("title") ?? "").toContain(
      "schema revision 1.0.0",
    );
  });

  it("opens the evidence popover for a bullet with evidence and invokes activity_events_get", async () => {
    registerInvokeHandler("activity_events_get", async () => [EVENT]);
    render(
      <StreamingPreview
        status="completed"
        progress={[]}
        draft={DRAFT}
        error={null}
      />,
    );
    fireEvent.click(screen.getByTestId("bullet-b1"));
    await waitFor(() =>
      expect(screen.getByText(/fix the thing/i)).toBeInTheDocument(),
    );
    expect(screen.getByTestId("evidence-link-ev-1")).toHaveTextContent(
      "MR !1",
    );
  });
});
