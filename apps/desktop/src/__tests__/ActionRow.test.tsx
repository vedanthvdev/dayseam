import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source } from "@dayseam/ipc-types";
import { ActionRow } from "../features/report/ActionRow";
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

describe("ActionRow", () => {
  beforeEach(() => {
    resetTauriMocks();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("auto-selects every source the first time the list loads", async () => {
    registerInvokeHandler("sources_list", async () => [SRC_A, SRC_B]);
    const onGenerate = vi.fn();
    render(
      <ActionRow
        status="idle"
        onGenerate={onGenerate}
        onCancel={() => {}}
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
      <ActionRow
        status="idle"
        onGenerate={onGenerate}
        onCancel={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`action-row-source-${SRC_A.id}`)).toBeChecked(),
    );

    fireEvent.change(screen.getByTestId("action-row-date"), {
      target: { value: "2026-02-14" },
    });
    fireEvent.click(screen.getByTestId(`action-row-source-${SRC_B.id}`));
    fireEvent.click(screen.getByTestId("action-row-generate"));

    expect(onGenerate).toHaveBeenCalledTimes(1);
    expect(onGenerate).toHaveBeenCalledWith("2026-02-14", [SRC_A.id]);
  });

  it("swaps Generate for Cancel while a run is in flight and wires the cancel handler", async () => {
    registerInvokeHandler("sources_list", async () => [SRC_A]);
    const onCancel = vi.fn();
    render(
      <ActionRow
        status="running"
        onGenerate={() => {}}
        onCancel={onCancel}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`action-row-source-${SRC_A.id}`)).toBeInTheDocument(),
    );
    expect(
      screen.queryByTestId("action-row-generate"),
    ).not.toBeInTheDocument();
    // DAY-119: ActionRow no longer renders a live progress message — the
    // canonical narration lives in StreamingPreview. Guard against
    // regression so we never surface progress in both places again.
    expect(
      screen.queryByTestId("action-row-progress-message"),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("action-row-cancel"));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("disables Generate with an explanation when no sources are connected", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(
      <ActionRow status="idle" onGenerate={() => {}} onCancel={() => {}} />,
    );
    await waitFor(() =>
      expect(
        screen.getByText(/add a source above to enable generate/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getByTestId("action-row-generate")).toBeDisabled();
  });
});
