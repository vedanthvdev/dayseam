import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { LocalRepo, Source, SourceHealth } from "@dayseam/ipc-types";
import { SourcesSidebar } from "../features/sources/SourcesSidebar";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
} from "./tauri-mock";

const HEALTHY: SourceHealth = {
  ok: true,
  checked_at: "2026-04-17T12:00:00Z",
  last_error: null,
};

const SOURCE: Source = {
  id: "src-1",
  kind: "LocalGit",
  label: "Work repos",
  config: { LocalGit: { scan_roots: ["/Users/me/code", "/Users/me/work"] } },
  secret_ref: null,
  created_at: "2026-04-10T12:00:00Z",
  last_sync_at: null,
  last_health: HEALTHY,
};

const REPO_A: LocalRepo = {
  path: "/Users/me/code/project-a",
  label: "project-a",
  is_private: false,
  discovered_at: "2026-04-10T12:00:00Z",
};

const REPO_B: LocalRepo = {
  path: "/Users/me/work/project-b",
  label: "project-b",
  is_private: false,
  discovered_at: "2026-04-10T12:00:00Z",
};

describe("SourcesSidebar", () => {
  beforeEach(() => {
    resetTauriMocks();
    // Default: no discovered repos. Individual tests override when
    // they want to assert on the count label.
    registerInvokeHandler("local_repos_list", async () => []);
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("renders the empty state when no sources are configured", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText(/no sources connected/i)).toBeInTheDocument(),
    );
    expect(
      screen.getByRole("button", { name: /add local git source/i }),
    ).toBeInTheDocument();
  });

  it("renders a configured source with its discovered repo count and a healthy dot", async () => {
    registerInvokeHandler("sources_list", async () => [SOURCE]);
    // The chip count surfaces `local_repos_list` — the number of
    // `.git` directories actually discovered under the scan roots —
    // not the raw scan-roots count.
    registerInvokeHandler("local_repos_list", async () => [REPO_A, REPO_B]);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText("Work repos")).toBeInTheDocument(),
    );
    await waitFor(() =>
      expect(screen.getByText(/· 2 repos/i)).toBeInTheDocument(),
    );
    expect(screen.getByTestId("source-chip-src-1")).toHaveAttribute(
      "title",
      expect.stringMatching(/healthy/i),
    );
  });

  it("invokes `sources_healthcheck` when the rescan control is clicked", async () => {
    registerInvokeHandler("sources_list", async () => [SOURCE]);
    registerInvokeHandler("sources_healthcheck", async () => HEALTHY);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText("Work repos")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /rescan work repos/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "sources_healthcheck",
        expect.objectContaining({ id: "src-1" }),
      ),
    );
  });

  it("opens the add-source dialog when the add button is clicked", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText(/no sources connected/i)).toBeInTheDocument(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: /add local git source/i }),
    );
    expect(
      screen.getByRole("dialog", { name: /add local git source/i }),
    ).toBeInTheDocument();
  });
});
