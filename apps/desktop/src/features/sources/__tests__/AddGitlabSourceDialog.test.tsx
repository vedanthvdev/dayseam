// RTL coverage for `AddGitlabSourceDialog` — the three-step flow plus
// the per-step invariants from plan §3:
//   * base-URL normalisation is total (delegates to `base-url.ts`, whose
//     own table test lives next door; here we just verify the dialog
//     reflects the helper's outcome),
//   * http:// warns but is not silently upgraded,
//   * the PAT validation flow renders success + failure state and
//     gates the submit button,
//   * submit in create mode calls `sources_add` with the id GitLab
//     echoed back, not whatever the user typed,
//   * reconnect (edit) mode reuses the existing source: base URL is
//     prefilled + read-only, submit calls `sources_update` with the
//     same source id.

import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source } from "@dayseam/ipc-types";
import { AddGitlabSourceDialog } from "../AddGitlabSourceDialog";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
} from "../../../__tests__/tauri-mock";

const EXISTING_GITLAB_SOURCE: Source = {
  id: "gitlab-source-1",
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
    keychain_account: "gitlab.gitlab-source-1",
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
        message: "401 Unauthorized",
        retryable: false,
        action_hint: null,
      },
    },
  },
};

describe("AddGitlabSourceDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("sources_list", async () => []);
  });
  afterEach(() => resetTauriMocks());

  it("shows the normalised URL preview once the user types a hostname", async () => {
    render(
      <AddGitlabSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
      />,
    );
    fireEvent.change(screen.getByTestId("add-gitlab-base-url"), {
      target: { value: "gitlab.example.com" },
    });
    expect(
      await screen.findByTestId("add-gitlab-url-normalised"),
    ).toHaveTextContent("https://gitlab.example.com");
  });

  it("shows a loud warning (never a silent upgrade) for http:// URLs", async () => {
    render(
      <AddGitlabSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
      />,
    );
    fireEvent.change(screen.getByTestId("add-gitlab-base-url"), {
      target: { value: "http://gitlab.internal.lan" },
    });
    expect(
      await screen.findByTestId("add-gitlab-insecure-warning"),
    ).toBeInTheDocument();
    // Preview must still say http:// — we DO NOT silently upgrade.
    expect(screen.getByTestId("add-gitlab-url-normalised")).toHaveTextContent(
      "http://gitlab.internal.lan",
    );
  });

  it("rejects path-laden URLs with an inline error", async () => {
    render(
      <AddGitlabSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
      />,
    );
    fireEvent.change(screen.getByTestId("add-gitlab-base-url"), {
      target: { value: "gitlab.example.com/some/path" },
    });
    expect(await screen.findByTestId("add-gitlab-url-invalid")).toBeInTheDocument();
  });

  it("validates the PAT via gitlab_validate_pat and enables submit", async () => {
    registerInvokeHandler("gitlab_validate_pat", async () => ({
      user_id: 17,
      username: "vedanth",
    }));
    const onAdded = vi.fn();
    const added: Source = {
      ...EXISTING_GITLAB_SOURCE,
      id: "gitlab-new",
      config: {
        GitLab: { base_url: "https://gitlab.example.com", user_id: 17, username: "vedanth" },
      },
      label: "gitlab.example.com",
      last_health: { ok: true, checked_at: null, last_error: null },
    };
    registerInvokeHandler("sources_add", async () => added);

    render(
      <AddGitlabSourceDialog
        open
        onClose={() => {}}
        onAdded={onAdded}
      />,
    );
    fireEvent.change(screen.getByTestId("add-gitlab-base-url"), {
      target: { value: "gitlab.example.com" },
    });
    fireEvent.change(screen.getByTestId("add-gitlab-pat"), {
      target: { value: "glpat-ok-token" },
    });
    fireEvent.click(screen.getByTestId("add-gitlab-validate"));
    expect(
      await screen.findByTestId("add-gitlab-validation-ok"),
    ).toHaveTextContent("vedanth");

    fireEvent.click(screen.getByRole("button", { name: /add source/i }));
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith(added));

    // The user_id stored on the new source is the one GitLab echoed
    // back via gitlab_validate_pat, NOT anything the user typed.
    expect(mockInvoke).toHaveBeenCalledWith(
      "sources_add",
      expect.objectContaining({
        kind: "GitLab",
        config: {
          GitLab: {
            base_url: "https://gitlab.example.com",
            user_id: 17,
            username: "vedanth",
          },
        },
      }),
    );
  });

  it("surfaces validation failures inline without enabling submit", async () => {
    registerInvokeHandler("gitlab_validate_pat", async () => {
      throw {
        kind: "Auth",
        data: {
          code: "gitlab.auth.invalid_token",
          message: "401 Unauthorized",
        },
      };
    });
    render(
      <AddGitlabSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
      />,
    );
    fireEvent.change(screen.getByTestId("add-gitlab-base-url"), {
      target: { value: "gitlab.example.com" },
    });
    fireEvent.change(screen.getByTestId("add-gitlab-pat"), {
      target: { value: "glpat-bad" },
    });
    fireEvent.click(screen.getByTestId("add-gitlab-validate"));
    expect(
      await screen.findByTestId("add-gitlab-validation-error"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add source/i })).toBeDisabled();
  });

  it("in reconnect mode, the base URL is read-only and submit calls sources_update", async () => {
    registerInvokeHandler("gitlab_validate_pat", async () => ({
      user_id: 42,
      username: "ved",
    }));
    const onSaved = vi.fn();
    registerInvokeHandler("sources_update", async () => ({
      ...EXISTING_GITLAB_SOURCE,
      last_health: { ok: true, checked_at: null, last_error: null },
    }));

    render(
      <AddGitlabSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        editing={EXISTING_GITLAB_SOURCE}
        onSaved={onSaved}
      />,
    );
    const urlField = screen.getByTestId("add-gitlab-base-url") as HTMLInputElement;
    expect(urlField.value).toBe("https://gitlab.acme.test");
    expect(urlField).toHaveAttribute("readonly");

    fireEvent.change(screen.getByTestId("add-gitlab-pat"), {
      target: { value: "glpat-new" },
    });
    fireEvent.click(screen.getByTestId("add-gitlab-validate"));
    await screen.findByTestId("add-gitlab-validation-ok");

    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "sources_update",
        expect.objectContaining({
          id: EXISTING_GITLAB_SOURCE.id,
          patch: expect.objectContaining({
            config: {
              GitLab: {
                base_url: "https://gitlab.acme.test",
                user_id: 42,
                username: "ved",
              },
            },
          }),
        }),
      ),
    );
    expect(onSaved).toHaveBeenCalled();
  });
});
