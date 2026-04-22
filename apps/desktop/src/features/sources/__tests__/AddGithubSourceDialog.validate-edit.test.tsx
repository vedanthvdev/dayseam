// DAY-99 validate-edit regression — mirrors the Atlassian and GitLab
// variants.
//
// `AddGithubSourceDialog` must drop any cached validation back to
// `{ kind: "idle" }` whenever the user edits the API base URL or the
// PAT field. Without that invalidation, a user who validates a PAT
// against one GitHub host (say `api.github.com`) and then pivots the
// URL to point at a different tenant (say `ghe.acme.com/api/v3/`)
// could click Add source and persist a `SourceIdentity` whose
// numeric `user_id` came from the first tenant — a silent "wrong
// account bound to the source" bug identical to the one DAY-90's
// TST-v0.2-05 test pinned for Atlassian.
//
// Three invariants this file locks down:
//
//   1. A second Validate click after editing the URL issues a fresh
//      `github_validate_credentials` IPC (not a cached result).
//   2. The "✓ Connected as …" ribbon disappears the moment any
//      credential input changes, so the user sees the stale state
//      was discarded.
//   3. The Add source button re-disables between ok → edit → ok
//      transitions, so a stale-ok can never round-trip into
//      `github_sources_add`.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { GithubValidationResult } from "@dayseam/ipc-types";
import { AddGithubSourceDialog } from "../AddGithubSourceDialog";
import {
  mockInvoke,
  registerInvokeHandler,
  resetTauriMocks,
} from "../../../__tests__/tauri-mock";

const ACCOUNT_CLOUD: GithubValidationResult = {
  user_id: 1001,
  login: "cloud-user",
  name: "Cloud User",
};

const ACCOUNT_GHES: GithubValidationResult = {
  user_id: 2002,
  login: "ghes-user",
  name: "Enterprise User",
};

/** Register a validate handler that routes on the `apiBaseUrl` the
 *  dialog fires. Routing on the URL rather than call ordinality lets
 *  each test assert which tenant actually got probed, not just that
 *  two calls happened. */
function registerValidatingHandler() {
  registerInvokeHandler("github_validate_credentials", async (args) => {
    const url = String(args.apiBaseUrl ?? "");
    if (url.includes("api.github.com")) return ACCOUNT_CLOUD;
    if (url.includes("ghe.acme.com")) return ACCOUNT_GHES;
    throw new Error(`unexpected apiBaseUrl in test: ${url}`);
  });
}

function validateCallCount(): number {
  return mockInvoke.mock.calls.filter(
    ([name]) => name === "github_validate_credentials",
  ).length;
}

describe("AddGithubSourceDialog validate-edit regression", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerValidatingHandler();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("issues a fresh IPC call on the second Validate click after editing the URL", async () => {
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );

    // The URL prefills to `https://api.github.com/`, so the first
    // validate routes to the cloud fixture without editing the URL.
    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_token_cloud" },
    });
    fireEvent.click(screen.getByTestId("add-github-validate"));
    await waitFor(() =>
      expect(screen.getByTestId("add-github-validation-ok")).toHaveTextContent(
        /Cloud User/,
      ),
    );
    expect(validateCallCount()).toBe(1);

    // Pivot to a GHES tenant. The `useEffect` on `[normalisedUrl,
    // pat]` must drop the cached `{ kind: "ok" }` back to `{ kind:
    // "idle" }` — asserted by the disappearance of the ok-ribbon
    // below.
    fireEvent.change(screen.getByTestId("add-github-api-base-url"), {
      target: { value: "https://ghe.acme.com/api/v3/" },
    });
    await waitFor(() =>
      expect(
        screen.queryByTestId("add-github-validation-ok"),
      ).not.toBeInTheDocument(),
    );

    // Second click must actually probe the GHES URL — if the dialog
    // cached the first result, this call never fires and both the
    // call-count and the per-tenant routing assertion below fail.
    fireEvent.click(screen.getByTestId("add-github-validate"));
    await waitFor(() =>
      expect(screen.getByTestId("add-github-validation-ok")).toHaveTextContent(
        /Enterprise User/,
      ),
    );
    expect(validateCallCount()).toBe(2);

    const secondCall = mockInvoke.mock.calls.filter(
      ([name]) => name === "github_validate_credentials",
    )[1];
    expect(secondCall?.[1]).toMatchObject({
      apiBaseUrl: "https://ghe.acme.com/api/v3/",
    });
  });

  it("clears the ✓ Connected ribbon as soon as the PAT changes", async () => {
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );

    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_first" },
    });
    fireEvent.click(screen.getByTestId("add-github-validate"));
    await waitFor(() =>
      expect(
        screen.getByTestId("add-github-validation-ok"),
      ).toBeInTheDocument(),
    );

    // Editing the PAT alone must drop validation back to idle —
    // this guards against a narrow "only-watches-URL" fix that
    // would still let a stale `user_id` round-trip to the backend
    // when the user swaps in a different token against the same URL.
    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_different" },
    });
    await waitFor(() =>
      expect(
        screen.queryByTestId("add-github-validation-ok"),
      ).not.toBeInTheDocument(),
    );
  });

  it("disables Add source between ok → edit → ok transitions", async () => {
    render(
      <AddGithubSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );

    fireEvent.change(screen.getByTestId("add-github-pat"), {
      target: { value: "ghp_token_cloud" },
    });
    fireEvent.click(screen.getByTestId("add-github-validate"));
    await waitFor(() =>
      expect(
        screen.getByTestId("add-github-validation-ok"),
      ).toBeInTheDocument(),
    );
    const submit = screen.getByRole("button", { name: /add source/i });
    expect(submit).toBeEnabled();

    fireEvent.change(screen.getByTestId("add-github-api-base-url"), {
      target: { value: "https://ghe.acme.com/api/v3/" },
    });
    await waitFor(() => expect(submit).toBeDisabled());

    fireEvent.click(screen.getByTestId("add-github-validate"));
    await waitFor(() =>
      expect(screen.getByTestId("add-github-validation-ok")).toHaveTextContent(
        /Enterprise User/,
      ),
    );
    await waitFor(() => expect(submit).toBeEnabled());

    // Wrap a trailing microtask flush so any pending setState from
    // the resolved invoke lands inside the test's act boundary.
    await act(async () => {});
  });
});
