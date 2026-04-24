// RTL coverage for `AddGithubSourceDialog` — the happy-path add flow
// plus the per-step invariants the DAY-99 plan pins:
//
//   * API base URL normalisation is total (delegates to
//     `github-api-base-url.ts`, whose own unit tests live next door;
//     here we just verify the dialog reflects the helper's outcome);
//   * https:// is the only accepted scheme — http:// produces an
//     inline error, not a silent upgrade;
//   * the PAT validation flow renders success + failure state and
//     gates the submit button;
//   * submit in add mode calls `github_sources_add` with the
//     `user_id` GitHub echoed back, not anything the user typed;
//   * reconnect mode hides the Validate button, renders the URL
//     read-only, and submit calls `github_sources_reconnect` with
//     the existing source id.
//
// The validate-edit regression test (cached validation must be
// invalidated when the URL or PAT changes) lives in a sibling file so
// the main dialog test stays focused on the happy-path shape.

import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source } from "@dayseam/ipc-types";
import { AddGithubSourceDialog } from "../AddGithubSourceDialog";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
} from "../../../__tests__/tauri-mock";

const EXISTING_GITHUB_SOURCE: Source = {
  id: "github-source-1",
  kind: "GitHub",
  label: "ved @ api.github.com",
  config: {
    GitHub: {
      api_base_url: "https://api.github.com/",
    },
  },
  secret_ref: {
    keychain_service: "dayseam.github",
    keychain_account: "source:github-source-1",
  },
  created_at: "2026-04-20T12:00:00Z",
  last_sync_at: null,
  last_health: {
    ok: false,
    checked_at: "2026-04-20T12:00:00Z",
    last_error: {
      variant: "Auth",
      data: {
        code: "github.auth.invalid_credentials",
        message: "401 Unauthorized",
        retryable: false,
        action_hint: null,
      },
    },
  },
};

const NEW_GITHUB_SOURCE: Source = {
  id: "github-new",
  kind: "GitHub",
  label: "api.github.com",
  config: {
    GitHub: { api_base_url: "https://api.github.com/" },
  },
  secret_ref: {
    keychain_service: "dayseam.github",
    keychain_account: "source:github-new",
  },
  created_at: "2026-04-20T12:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
};

describe("AddGithubSourceDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("sources_list", async () => []);
  });
  afterEach(() => resetTauriMocks());

  it("prefills the cloud API base URL so one-click cloud add needs no typing", () => {
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );
    const urlField = screen.getByTestId(
      "add-github-api-base-url",
    ) as HTMLInputElement;
    expect(urlField.value).toBe("https://api.github.com/");
    expect(screen.getByTestId("add-github-url-normalised")).toHaveTextContent(
      "https://api.github.com/",
    );
    expect(screen.getByTestId("add-github-url-normalised")).toHaveTextContent(
      /GitHub cloud/,
    );
  });

  it("surfaces an inline error — never a silent upgrade — for http:// URLs", async () => {
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );
    fireEvent.change(screen.getByTestId("add-github-api-base-url"), {
      target: { value: "http://api.github.com/" },
    });
    expect(await screen.findByTestId("add-github-url-invalid")).toHaveTextContent(
      /requires https/i,
    );
    // The Validate button is gated on `kind: "ok"`, so an invalid
    // URL leaves it disabled regardless of what the user pastes in
    // the PAT field.
    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_token" },
    });
    expect(screen.getByTestId("add-github-validate")).toBeDisabled();
  });

  it("normalises a GitHub Enterprise path with trailing slash", async () => {
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );
    fireEvent.change(screen.getByTestId("add-github-api-base-url"), {
      target: { value: "https://ghe.acme.com/api/v3" },
    });
    // The canonical form we store carries a trailing slash so
    // `Url::join("user")` on the Rust side preserves the `/api/v3/`
    // prefix. The dialog's preview must match that canonical shape.
    expect(
      await screen.findByTestId("add-github-url-normalised"),
    ).toHaveTextContent("https://ghe.acme.com/api/v3/");
    expect(screen.getByTestId("add-github-url-normalised")).toHaveTextContent(
      /Enterprise/,
    );
  });

  it("validates the PAT via github_validate_credentials and enables submit", async () => {
    registerInvokeHandler("github_validate_credentials", async () => ({
      user_id: 12345,
      login: "vedanth",
      name: "Vedanth Vasudev",
    }));
    const onAdded = vi.fn();
    registerInvokeHandler("github_sources_add", async () => NEW_GITHUB_SOURCE);

    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={onAdded} />,
    );
    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_ok_token" },
    });
    fireEvent.click(screen.getByTestId("add-github-validate"));
    const okRibbon = await screen.findByTestId("add-github-validation-ok");
    // The ribbon prefers `name` (display name) and parenthesises the
    // handle when both are present — this mirrors the attribution
    // shape reports use downstream.
    expect(okRibbon).toHaveTextContent(/Vedanth Vasudev/);
    expect(okRibbon).toHaveTextContent(/@vedanth/);

    // Label should auto-default to the host of the normalised URL
    // so the common cloud case needs no typing in the label field.
    expect(
      (screen.getByTestId("add-github-label") as HTMLInputElement).value,
    ).toBe("api.github.com");

    fireEvent.click(screen.getByRole("button", { name: /add source/i }));
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith(NEW_GITHUB_SOURCE));

    expect(mockInvoke).toHaveBeenCalledWith(
      "github_sources_add",
      expect.objectContaining({
        apiBaseUrl: "https://api.github.com/",
        label: "api.github.com",
        pat: "ghp_ok_token",
        // The `user_id` on the persisted source must be the one
        // GitHub echoed back via `github_validate_credentials`, not
        // anything the user typed. Breaking this is the "wrong
        // account bound to the source" regression DAY-99 invariant 4
        // is designed to prevent.
        userId: 12345,
        // CORR-v0.4-01: the dialog must also thread `login` through
        // so `github_sources_add_impl` can seed the `GitHubLogin`
        // identity row. Missing this value causes the walker to
        // silently return zero events on every sync.
        login: "vedanth",
      }),
    );
  });

  it("falls back to the login when GitHub returns no display name", async () => {
    registerInvokeHandler("github_validate_credentials", async () => ({
      user_id: 7,
      login: "handle-only",
      name: null,
    }));
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );
    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_ok" },
    });
    fireEvent.click(screen.getByTestId("add-github-validate"));
    const ribbon = await screen.findByTestId("add-github-validation-ok");
    expect(ribbon).toHaveTextContent(/handle-only/);
    // No bare "@handle" suffix when the login is the sole label —
    // keeps the ribbon from reading "handle-only (@handle-only)".
    expect(ribbon.textContent ?? "").not.toMatch(/\(@handle-only\)/);
  });

  it("surfaces validation failures inline without enabling submit", async () => {
    registerInvokeHandler("github_validate_credentials", async () => {
      throw {
        kind: "Auth",
        data: {
          code: "github.auth.invalid_credentials",
          message: "401 Unauthorized",
        },
      };
    });
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );
    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_bad" },
    });
    fireEvent.click(screen.getByTestId("add-github-validate"));
    expect(
      await screen.findByTestId("add-github-validation-error"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add source/i })).toBeDisabled();
  });

  it("in edit mode, URL is read-only, label is editable, and a PAT submit calls github_sources_reconnect", async () => {
    const onReconnected = vi.fn();
    registerInvokeHandler(
      "github_sources_reconnect",
      async () => EXISTING_GITHUB_SOURCE.id,
    );

    render(
      <AddGithubSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        reconnect={{ source: EXISTING_GITHUB_SOURCE }}
        onReconnected={onReconnected}
      />,
    );
    const urlField = screen.getByTestId(
      "add-github-api-base-url",
    ) as HTMLInputElement;
    expect(urlField.value).toBe("https://api.github.com/");
    expect(urlField).toHaveAttribute("readonly");

    // Edit mode skips the explicit Validate step — the backend
    // re-runs `/user` as part of the reconnect IPC. The label
    // field is now rendered (DAY-126) so users can rename without
    // needing a separate dialog.
    expect(screen.queryByTestId("add-github-validate")).not.toBeInTheDocument();
    const labelField = screen.getByTestId(
      "add-github-label",
    ) as HTMLInputElement;
    expect(labelField.value).toBe(EXISTING_GITHUB_SOURCE.label);
    expect(
      screen.queryByTestId("add-github-open-token-page"),
    ).not.toBeInTheDocument();

    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_new" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "github_sources_reconnect",
        expect.objectContaining({
          sourceId: EXISTING_GITHUB_SOURCE.id,
          pat: "ghp_new",
        }),
      ),
    );
    // A PAT-only edit must not touch the label via sources_update
    // — otherwise we'd fire a no-op IPC with the unchanged label.
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "sources_update",
      expect.anything(),
    );
    expect(onReconnected).toHaveBeenCalledWith(EXISTING_GITHUB_SOURCE.id);
  });

  // DAY-126: editing only the label (no PAT) must skip the
  // reconnect IPC entirely — the keychain entry is untouched and
  // the only write is a label-only `sources_update` patch. This is
  // the straight-line rename flow the old `RenameSourceDialog`
  // used to own, now covered here.
  it("in edit mode, a label-only change calls sources_update and not github_sources_reconnect", async () => {
    const onReconnected = vi.fn();
    registerInvokeHandler("sources_update", async () => ({
      ...EXISTING_GITHUB_SOURCE,
      label: "Work GitHub",
    }));

    render(
      <AddGithubSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        reconnect={{ source: EXISTING_GITHUB_SOURCE }}
        onReconnected={onReconnected}
      />,
    );

    const submit = screen.getByRole("button", { name: /^save$/i });
    // Nothing dirty yet: PAT empty, label unchanged — Save must
    // stay disabled, otherwise a click would fire a no-op edit.
    expect(submit).toBeDisabled();

    fireEvent.change(screen.getByTestId("add-github-label"), {
      target: { value: "Work GitHub" },
    });
    expect(submit).toBeEnabled();

    fireEvent.click(submit);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "sources_update",
        expect.objectContaining({
          id: EXISTING_GITHUB_SOURCE.id,
          patch: expect.objectContaining({ label: "Work GitHub", config: null }),
          pat: null,
        }),
      ),
    );
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "github_sources_reconnect",
      expect.anything(),
    );
    expect(onReconnected).toHaveBeenCalledWith(EXISTING_GITHUB_SOURCE.id);
  });

  it("in edit mode, Save stays disabled when nothing is dirty and enables on either PAT or label change", () => {
    render(
      <AddGithubSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        reconnect={{ source: EXISTING_GITHUB_SOURCE }}
        onReconnected={() => {}}
      />,
    );
    const submit = screen.getByRole("button", { name: /^save$/i });
    expect(submit).toBeDisabled();

    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_x" },
    });
    expect(submit).toBeEnabled();

    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "" },
    });
    expect(submit).toBeDisabled();

    fireEvent.change(screen.getByTestId("add-github-label"), {
      target: { value: "renamed" },
    });
    expect(submit).toBeEnabled();

    // Trimmed-empty label is rejected (DB pins label NOT NULL).
    fireEvent.change(screen.getByTestId("add-github-label"), {
      target: { value: "   " },
    });
    expect(submit).toBeDisabled();
  });
});
