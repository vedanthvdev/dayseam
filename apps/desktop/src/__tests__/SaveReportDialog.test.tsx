import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Sink, WriteReceipt } from "@dayseam/ipc-types";
import { SaveReportDialog } from "../features/report/SaveReportDialog";
import { registerInvokeHandler, resetTauriMocks } from "./tauri-mock";

const SINK: Sink = {
  id: "sink-1",
  kind: "MarkdownFile",
  label: "Daily notes",
  config: {
    MarkdownFile: {
      config_version: 1,
      dest_dirs: ["/Users/me/vault/daily"],
      frontmatter: true,
    },
  },
  created_at: "2026-04-17T12:00:00Z",
  last_write_at: null,
};

const RECEIPT: WriteReceipt = {
  run_id: "rrrrrrrr-rrrr-rrrr-rrrr-rrrrrrrrrrrr",
  sink_kind: "MarkdownFile",
  destinations_written: ["/Users/me/vault/daily/2026-04-17.md"],
  external_refs: [],
  bytes_written: 2048,
  written_at: "2026-04-17T12:00:00Z",
};

describe("SaveReportDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("lists configured sinks and calls onSave with the chosen id", async () => {
    registerInvokeHandler("sinks_list", async () => [SINK]);
    const onSave = vi.fn(async () => [RECEIPT]);
    render(
      <SaveReportDialog
        open
        hasDraft
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`save-sink-${SINK.id}`)).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTestId(`save-sink-${SINK.id}`));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(SINK.id));
    await waitFor(() =>
      expect(screen.getByTestId("save-report-receipts")).toBeInTheDocument(),
    );
    expect(
      screen.getByText(/\/Users\/me\/vault\/daily\/2026-04-17\.md/),
    ).toBeInTheDocument();
  });

  it("disables Save when there is no draft to save", async () => {
    registerInvokeHandler("sinks_list", async () => [SINK]);
    render(
      <SaveReportDialog
        open
        hasDraft={false}
        onClose={() => {}}
        onSave={async () => []}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`save-sink-${SINK.id}`)).toBeInTheDocument(),
    );
    expect(screen.getByRole("button", { name: /^save$/i })).toBeDisabled();
  });

  it("surfaces save errors without replacing the sink list", async () => {
    registerInvokeHandler("sinks_list", async () => [SINK]);
    const onSave = vi.fn(async () => {
      throw new Error("disk full");
    });
    render(
      <SaveReportDialog open hasDraft onClose={() => {}} onSave={onSave} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId(`save-sink-${SINK.id}`)).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTestId(`save-sink-${SINK.id}`));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() =>
      expect(screen.getByTestId("save-report-error")).toHaveTextContent(
        /disk full/i,
      ),
    );
  });

  it("tells the user how to add a sink when none are configured", async () => {
    registerInvokeHandler("sinks_list", async () => []);
    render(
      <SaveReportDialog
        open
        hasDraft
        onClose={() => {}}
        onSave={async () => []}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/no sinks configured yet/i)).toBeInTheDocument(),
    );
  });
});
