// DAY-170: `ActionRow` was folded into `SourcesSidebar` so the
// user-facing "pick a day, pick sources, hit Generate" surface fits
// on a single row directly above the preview. This file keeps the
// former `action-row-*` behaviour pinned — auto-selection on first
// load, toggle-to-exclude, Generate/Cancel swap, and the disabled-
// Generate empty state — but now drives them through the merged
// `SourcesSidebar` component via its `reportActions` prop. The
// testids stay stable (`action-row-date`, `action-row-source-<id>`,
// `action-row-generate`, `action-row-cancel`) so a future reader
// searching for "action-row" still lands on the contract. The file
// name is kept as a historical pointer; the contents have moved.

import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source } from "@dayseam/ipc-types";
import { SourcesSidebar } from "../features/sources/SourcesSidebar";
import { registerInvokeHandler, resetTauriMocks } from "./tauri-mock";

const SRC_A: Source = {
  id: "src-a",
  kind: "LocalGit",
  label: "Repo A",
  config: { LocalGit: { scan_roots: ["/tmp/a"] } },
  secret_ref: null,
  created_at: "2026-04-17T00:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
} as unknown as Source;

const SRC_B: Source = { ...SRC_A, id: "src-b", label: "Repo B" };

describe("SourcesSidebar report actions (formerly ActionRow)", () => {
  beforeEach(() => {
    resetTauriMocks();
    // `useLocalRepos` fires for every LocalGit chip; stub it flat
    // so the tests below are not coupled to repo-discovery plumbing.
    registerInvokeHandler("local_repos_list", async () => []);
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("auto-selects every source the first time the list loads", async () => {
    registerInvokeHandler("sources_list", async () => [SRC_A, SRC_B]);
    const onGenerate = vi.fn();
    render(
      <SourcesSidebar
        reportActions={{
          status: "idle",
          onGenerate,
          onCancel: () => {},
        }}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`action-row-source-${SRC_A.id}`)).toBeChecked(),
    );
    expect(screen.getByTestId(`action-row-source-${SRC_B.id}`)).toBeChecked();
  });

  it("fires onGenerate with the selected date and source ids", async () => {
    registerInvokeHandler("sources_list", async () => [SRC_A, SRC_B]);
    const onGenerate = vi.fn();
    render(
      <SourcesSidebar
        reportActions={{
          status: "idle",
          onGenerate,
          onCancel: () => {},
        }}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`action-row-source-${SRC_A.id}`)).toBeChecked(),
    );

    fireEvent.change(screen.getByTestId("action-row-date"), {
      target: { value: "2026-02-14" },
    });
    // Clicking the chip body is the same gesture as clicking the
    // hidden checkbox — the `<label htmlFor=…>` wrapper wires
    // native click-to-toggle semantics. Driving the checkbox
    // directly keeps the assertion about selection explicit.
    fireEvent.click(screen.getByTestId(`action-row-source-${SRC_B.id}`));
    fireEvent.click(screen.getByTestId("action-row-generate"));

    expect(onGenerate).toHaveBeenCalledTimes(1);
    expect(onGenerate).toHaveBeenCalledWith("2026-02-14", [SRC_A.id]);
  });

  it("swaps Generate for Cancel while a run is in flight and wires the cancel handler", async () => {
    registerInvokeHandler("sources_list", async () => [SRC_A]);
    const onCancel = vi.fn();
    render(
      <SourcesSidebar
        reportActions={{
          status: "running",
          onGenerate: () => {},
          onCancel,
        }}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByTestId(`action-row-source-${SRC_A.id}`),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByTestId("action-row-generate"),
    ).not.toBeInTheDocument();
    // DAY-119: ActionRow never rendered a live progress message
    // (StreamingPreview owns live narration). The merged row
    // inherits that contract — no progress text inline.
    expect(
      screen.queryByTestId("action-row-progress-message"),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("action-row-cancel"));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("disables Generate when no sources are connected", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(
      <SourcesSidebar
        reportActions={{
          status: "idle",
          onGenerate: () => {},
          onCancel: () => {},
        }}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/no sources connected/i)).toBeInTheDocument(),
    );
    expect(screen.getByTestId("action-row-generate")).toBeDisabled();
  });
});
