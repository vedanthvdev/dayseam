import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ActivityEvent } from "@dayseam/ipc-types";
import { BulletEvidencePopover } from "../features/report/BulletEvidencePopover";
import {
  mockInvoke,
  registerInvokeHandler,
  resetTauriMocks,
} from "./tauri-mock";

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

describe("BulletEvidencePopover", () => {
  beforeEach(() => {
    resetTauriMocks();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("fetches events for the provided ids and renders them", async () => {
    registerInvokeHandler("activity_events_get", async () => [EVENT]);
    render(
      <BulletEvidencePopover
        bullet={{ id: "b1", text: "Shipped feature X" }}
        eventIds={["ev-1"]}
        reason="1 commit"
        onClose={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/fix the thing/i)).toBeInTheDocument(),
    );
    expect(mockInvoke).toHaveBeenCalledWith(
      "activity_events_get",
      expect.objectContaining({ ids: ["ev-1"] }),
    );
  });

  it("routes link clicks through shell_open with the link's url", async () => {
    registerInvokeHandler("activity_events_get", async () => [EVENT]);
    registerInvokeHandler("shell_open", async () => null);
    render(
      <BulletEvidencePopover
        bullet={{ id: "b1", text: "Shipped feature X" }}
        eventIds={["ev-1"]}
        reason="1 commit"
        onClose={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId("evidence-link-ev-1")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTestId("evidence-link-ev-1"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "shell_open",
        expect.objectContaining({ url: "https://example.com/x" }),
      ),
    );
  });

  it("closes on Escape", async () => {
    registerInvokeHandler("activity_events_get", async () => []);
    const onClose = vi.fn();
    render(
      <BulletEvidencePopover
        bullet={{ id: "b1", text: "Shipped feature X" }}
        eventIds={["ev-gone"]}
        reason="n/a"
        onClose={onClose}
      />,
    );
    // Wait for the activity_events_get promise to settle so the
    // trailing setState doesn't race into React after the test is
    // torn down and trip the act() warning.
    await waitFor(() =>
      expect(screen.getByText(/no longer on disk/i)).toBeInTheDocument(),
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("surfaces the eviction fallback when the event rows are gone", async () => {
    registerInvokeHandler("activity_events_get", async () => []);
    render(
      <BulletEvidencePopover
        bullet={{ id: "b1", text: "Shipped feature X" }}
        eventIds={["ev-gone"]}
        reason="1 commit"
        onClose={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/no longer on disk/i)).toBeInTheDocument(),
    );
  });
});
