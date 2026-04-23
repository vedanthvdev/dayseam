// DAY-121 coverage for `RenameSourceDialog`. Three contract invariants
// matter and each has a dedicated test:
//
//   1. Submit is disabled until the user actually changes the label
//      (whitespace, empty, and "same as current" all keep it off).
//   2. Every supported connector kind — LocalGit, GitLab, GitHub,
//      Jira, Confluence — round-trips through `sources_update` with
//      a label-only patch (`config: null`, `pat: null`). The backend
//      has supported this shape since DAY-70 but the UI never wired
//      a path that exercised it uniformly until now.
//   3. Backend rejection (e.g. `sources_update` throws) surfaces an
//      inline error instead of swallowing it and closing the dialog
//      on an invisible failure.

import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source, SourceHealth } from "@dayseam/ipc-types";
import { RenameSourceDialog } from "../RenameSourceDialog";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
} from "../../../__tests__/tauri-mock";

const HEALTHY: SourceHealth = {
  ok: true,
  checked_at: "2026-04-17T12:00:00Z",
  last_error: null,
};

function makeSource(overrides: Partial<Source>): Source {
  return {
    id: "src-1",
    kind: "LocalGit",
    label: "Current label",
    config: { LocalGit: { scan_roots: ["/Users/me/code"] } },
    secret_ref: null,
    created_at: "2026-04-10T12:00:00Z",
    last_sync_at: null,
    last_health: HEALTHY,
    ...overrides,
  };
}

describe("RenameSourceDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("sources_list", async () => []);
  });
  afterEach(() => resetTauriMocks());

  it("disables Save until the label actually changes", async () => {
    const source = makeSource({ label: "Work repos" });
    render(
      <RenameSourceDialog
        source={source}
        onClose={() => {}}
        onRenamed={() => {}}
      />,
    );

    const saveBtn = await screen.findByRole("button", { name: /save/i });
    // Initial state: the input is prefilled with the current label,
    // so nothing has changed yet and Save stays off.
    expect(saveBtn).toBeDisabled();

    const input = screen.getByTestId("rename-source-label") as HTMLInputElement;
    expect(input.value).toBe("Work repos");

    // Whitespace-only input is not a valid label — Save stays off
    // even though the string technically differs from the current
    // label.
    fireEvent.change(input, { target: { value: "   " } });
    expect(saveBtn).toBeDisabled();

    // Typing back the exact current label (after trim) should also
    // keep Save off so clicking it always produces a visible change.
    fireEvent.change(input, { target: { value: "  Work repos  " } });
    expect(saveBtn).toBeDisabled();

    // Any non-empty, actually-different label enables Save.
    fireEvent.change(input, { target: { value: "Home repos" } });
    expect(saveBtn).toBeEnabled();
  });

  const KINDS: Array<{ name: string; source: Source }> = [
    {
      name: "LocalGit",
      source: makeSource({
        id: "local-1",
        kind: "LocalGit",
        label: "Local work",
        config: { LocalGit: { scan_roots: ["/Users/me/code"] } },
      }),
    },
    {
      name: "GitLab",
      source: makeSource({
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
          keychain_service: "dayseam.gitlab",
          keychain_account: "source:gl-1",
        },
      }),
    },
    {
      name: "GitHub",
      source: makeSource({
        id: "gh-1",
        kind: "GitHub",
        label: "Personal GitHub",
        config: { GitHub: { api_base_url: "https://api.github.com" } },
        secret_ref: {
          keychain_service: "dayseam.github",
          keychain_account: "source:gh-1",
        },
      }),
    },
    {
      name: "Jira",
      source: makeSource({
        id: "jira-1",
        kind: "Jira",
        label: "Jira — acme.atlassian.net",
        config: {
          Jira: {
            workspace_url: "https://acme.atlassian.net",
            email: "me@acme.test",
          },
        },
        secret_ref: {
          keychain_service: "dayseam.atlassian",
          keychain_account: "slot:abc",
        },
      }),
    },
    {
      name: "Confluence",
      source: makeSource({
        id: "conf-1",
        kind: "Confluence",
        label: "Confluence — acme.atlassian.net",
        config: {
          Confluence: {
            workspace_url: "https://acme.atlassian.net",
            email: "me@acme.test",
          },
        },
        secret_ref: {
          keychain_service: "dayseam.atlassian",
          keychain_account: "slot:abc",
        },
      }),
    },
  ];

  for (const { name, source } of KINDS) {
    it(`renames ${name} via sources_update with pat: null and config: null`, async () => {
      const renamed: Source = { ...source, label: "My renamed source" };
      registerInvokeHandler("sources_update", async () => renamed);
      const onRenamed = vi.fn();

      render(
        <RenameSourceDialog
          source={source}
          onClose={() => {}}
          onRenamed={onRenamed}
        />,
      );

      const input = screen.getByTestId("rename-source-label");
      fireEvent.change(input, { target: { value: "My renamed source" } });
      fireEvent.click(screen.getByRole("button", { name: /save/i }));

      await waitFor(() =>
        expect(mockInvoke).toHaveBeenCalledWith("sources_update", {
          id: source.id,
          patch: { label: "My renamed source", config: null },
          // Critical: `pat: null`, not `""`. The Rust-side
          // `validate_pat_arg` rejects an empty string for GitLab as
          // `ipc.gitlab.pat.missing`; `null` picks the no-op arm
          // that leaves the stored secret alone.
          pat: null,
        }),
      );
      expect(onRenamed).toHaveBeenCalledWith(renamed);
    });
  }

  it("surfaces backend errors inline and keeps the dialog open", async () => {
    registerInvokeHandler("sources_update", async () => {
      throw new Error("label conflict with existing source");
    });
    const source = makeSource({ label: "Current" });
    const onRenamed = vi.fn();

    render(
      <RenameSourceDialog
        source={source}
        onClose={() => {}}
        onRenamed={onRenamed}
      />,
    );

    fireEvent.change(screen.getByTestId("rename-source-label"), {
      target: { value: "New name" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(
      await screen.findByTestId("rename-source-error"),
    ).toHaveTextContent(/label conflict/i);
    // onRenamed must not fire so the caller doesn't close the
    // dialog on an invisible failure.
    expect(onRenamed).not.toHaveBeenCalled();
    // The dialog is still mounted and Save is re-enabled so the
    // user can retry after reading the error.
    expect(screen.getByRole("button", { name: /save/i })).toBeEnabled();
  });
});
