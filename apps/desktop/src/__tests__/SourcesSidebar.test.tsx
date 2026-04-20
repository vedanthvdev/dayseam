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
      screen.getByRole("button", { name: /^add source$/i }),
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

  it("opens the local-git add dialog from the add-source menu", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText(/no sources connected/i)).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^add source$/i }));
    fireEvent.click(
      screen.getByRole("menuitem", { name: /add local git source/i }),
    );
    expect(
      screen.getByRole("dialog", { name: /add local git source/i }),
    ).toBeInTheDocument();
  });

  it("opens the GitLab add dialog from the add-source menu", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText(/no sources connected/i)).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^add source$/i }));
    fireEvent.click(
      screen.getByRole("menuitem", { name: /add gitlab source/i }),
    );
    expect(
      screen.getByRole("dialog", { name: /add gitlab source/i }),
    ).toBeInTheDocument();
  });

  it("renders a SourceErrorCard for a GitLab source with an auth error", async () => {
    const BROKEN_GITLAB: Source = {
      id: "gl-1",
      kind: "GitLab",
      label: "Acme GitLab",
      config: {
        GitLab: {
          base_url: "https://gitlab.acme.test",
          user_id: 42,
          username: "ved",
        },
      },
      secret_ref: {
        keychain_service: "dayseam",
        keychain_account: "gitlab.gl-1",
      },
      created_at: "2026-04-17T12:00:00Z",
      last_sync_at: null,
      last_health: {
        ok: false,
        checked_at: "2026-04-17T12:00:00Z",
        last_error: {
          variant: "Auth",
          data: {
            code: "gitlab.auth.invalid_token",
            message: "401",
            retryable: false,
            action_hint: null,
          },
        },
      },
    };
    registerInvokeHandler("sources_list", async () => [BROKEN_GITLAB]);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByTestId("source-error-card-gl-1")).toBeInTheDocument(),
    );
    // Reconnect button re-opens AddGitlabSourceDialog in edit mode.
    fireEvent.click(
      screen.getByTestId("source-error-card-reconnect-gl-1"),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("dialog", { name: /reconnect.*gitlab/i }),
      ).toBeInTheDocument(),
    );
  });

  // Regression: rows created by the pre-fix `sources_add` (the PAT-
  // drop bug) land with `secret_ref: null` but a never-checked health
  // blob. Without synthesising the Auth error here, the only signal
  // is a failed `report_generate` toast with no pointer back to the
  // source that caused it — exactly the "gitlab is broken, where?"
  // failure mode reported in DAY-70 after the rollout of the keychain
  // plumbing fix.
  it("renders a Reconnect card for a GitLab source whose secret_ref is null, even without a prior healthcheck", async () => {
    const ORPHAN_GITLAB: Source = {
      id: "gl-2",
      kind: "GitLab",
      label: "Acme GitLab",
      config: {
        GitLab: {
          base_url: "https://gitlab.acme.test",
          user_id: 42,
          username: "ved",
        },
      },
      secret_ref: null,
      created_at: "2026-04-17T12:00:00Z",
      last_sync_at: null,
      last_health: {
        ok: true,
        checked_at: null,
        last_error: null,
      },
    };
    registerInvokeHandler("sources_list", async () => [ORPHAN_GITLAB]);
    render(<SourcesSidebar />);
    const card = await screen.findByTestId("source-error-card-gl-2");
    expect(card).toHaveAttribute(
      "data-error-code",
      "gitlab.auth.invalid_token",
    );
    fireEvent.click(
      screen.getByTestId("source-error-card-reconnect-gl-2"),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("dialog", { name: /reconnect.*gitlab/i }),
      ).toBeInTheDocument(),
    );
  });

  // Inverse: a LocalGit source with `secret_ref: null` is healthy,
  // and a GitLab source with a valid `secret_ref` on file is not in
  // the reconnect-needed state. Either synthesis triggering here
  // would hide the Add Source flow behind a red wall on the empty
  // state, so they both explicitly must not render.
  it("does not synthesise a reconnect card for LocalGit or for a GitLab source with a secret_ref", async () => {
    const HEALTHY_GITLAB: Source = {
      id: "gl-3",
      kind: "GitLab",
      label: "Healthy GitLab",
      config: {
        GitLab: {
          base_url: "https://gitlab.acme.test",
          user_id: 42,
          username: "ved",
        },
      },
      secret_ref: {
        keychain_service: "dayseam.gitlab",
        keychain_account: "source:gl-3",
      },
      created_at: "2026-04-17T12:00:00Z",
      last_sync_at: null,
      last_health: { ok: true, checked_at: null, last_error: null },
    };
    registerInvokeHandler("sources_list", async () => [SOURCE, HEALTHY_GITLAB]);
    render(<SourcesSidebar />);
    await waitFor(() =>
      expect(screen.getByText("Healthy GitLab")).toBeInTheDocument(),
    );
    expect(
      screen.queryByTestId("source-error-card-src-1"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("source-error-card-gl-3"),
    ).not.toBeInTheDocument();
  });
});
