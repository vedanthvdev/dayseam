import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LogEntry } from "@dayseam/ipc-types";
import { LogDrawer } from "../components/LogDrawer";
import { registerInvokeHandler, resetTauriMocks } from "./tauri-mock";

const SAMPLE: LogEntry[] = [
  {
    timestamp: "2024-01-01T10:00:00Z",
    level: "Info",
    source_id: null,
    message: "app started",
  },
  {
    timestamp: "2024-01-01T10:00:05Z",
    level: "Error",
    source_id: null,
    message: "something failed",
  },
  {
    timestamp: "2024-01-01T10:00:06Z",
    level: "Debug",
    source_id: null,
    message: "trace msg",
  },
];

describe("LogDrawer", () => {
  beforeEach(() => {
    resetTauriMocks();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("renders nothing when closed", () => {
    registerInvokeHandler("logs_tail", async () => SAMPLE);
    render(<LogDrawer open={false} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("fetches and renders entries when opened", async () => {
    registerInvokeHandler("logs_tail", async () => SAMPLE);
    render(<LogDrawer open onClose={() => {}} />);

    await waitFor(() =>
      expect(screen.getByText(/app started/i)).toBeInTheDocument(),
    );
    expect(screen.getByText(/something failed/i)).toBeInTheDocument();
    expect(screen.getByText(/trace msg/i)).toBeInTheDocument();
  });

  it("hides entries whose level is filtered out", async () => {
    registerInvokeHandler("logs_tail", async () => SAMPLE);
    render(<LogDrawer open onClose={() => {}} />);

    await waitFor(() =>
      expect(screen.getByText(/app started/i)).toBeInTheDocument(),
    );

    // Toggle Debug off.
    fireEvent.click(
      screen.getByRole("checkbox", { name: /debug/i }),
    );

    expect(screen.queryByText(/trace msg/i)).toBeNull();
    expect(screen.getByText(/something failed/i)).toBeInTheDocument();
  });

  it("renders the error state when logs_tail throws", async () => {
    registerInvokeHandler("logs_tail", async () => {
      throw new Error("db closed");
    });
    render(<LogDrawer open onClose={() => {}} />);

    await waitFor(() =>
      expect(screen.getByText(/failed to load logs/i)).toBeInTheDocument(),
    );
    expect(screen.getByText(/db closed/i)).toBeInTheDocument();
  });

  it("calls onClose when Close is clicked", async () => {
    registerInvokeHandler("logs_tail", async () => SAMPLE);
    const onClose = vi.fn();
    render(<LogDrawer open onClose={onClose} />);

    await waitFor(() =>
      expect(screen.getByText(/app started/i)).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: /close log drawer/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("narrows the visible entries to the current run when 'This run' is toggled", async () => {
    registerInvokeHandler("logs_tail", async () => SAMPLE);
    render(
      <LogDrawer
        open
        onClose={() => {}}
        currentRunId="11111111-2222-3333-4444-555555555555"
        liveLogs={[
          {
            run_id: "11111111-2222-3333-4444-555555555555",
            source_id: null,
            level: "Error",
            message: "something failed",
            context: {},
            emitted_at: "2024-01-01T10:00:05Z",
          },
        ]}
      />,
    );

    await waitFor(() =>
      expect(screen.getByText(/app started/i)).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByTestId("log-drawer-run-filter"));

    expect(screen.queryByText(/app started/i)).toBeNull();
    expect(screen.getByText(/something failed/i)).toBeInTheDocument();
  });

  it("disables the run filter when there is no active run", async () => {
    registerInvokeHandler("logs_tail", async () => []);
    render(<LogDrawer open onClose={() => {}} currentRunId={null} />);
    // The filter is disabled synchronously, but `LogDrawer`'s
    // on-mount `logs_tail` resolves as a microtask and flips a
    // loading flag. `findBy*` yields the render loop once so the
    // resulting `setState` lands inside React's automatic `act`
    // boundary, silencing the TST-05 warning without weakening the
    // assertion itself.
    expect(
      await screen.findByTestId("log-drawer-run-filter"),
    ).toBeDisabled();
  });
});
