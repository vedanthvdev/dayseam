// DAY-90 TST-v0.2-05 — validate / edit / re-validate regression.
//
// `AddAtlassianSourceDialog` invalidates cached validation on every
// URL, email, token, or token-mode change (the `useEffect` that
// resets to `{ kind: "idle" }` when any of those inputs change).
// Without that invalidation, a user who validates a token against
// one workspace, then edits the URL to point at a different
// workspace, could click Add source and persist a `SourceIdentity`
// whose `account_id` came from the first workspace but whose URL
// points at the second — a silent "wrong account bound to the
// source" bug that would only surface later when Jira / Confluence
// rows came back empty.
//
// This file pins that invariant with RTL:
//
// 1. `validate-edit-validate_runs_the_ipc_twice` is the core
//    regression test — assert that a second Validate click after
//    editing the URL actually issues a second `atlassian_
//    validate_credentials` IPC call (not a cached result).
//
// 2. `editing_inputs_clears_prior_ok_status` is the user-visible
//    half — the "✓ Connected as …" ribbon must disappear as soon
//    as the user edits the URL so they see a stale state was
//    discarded.
//
// 3. `add_button_redisables_after_edit_until_revalidation` guards
//    the submit-button interlock: a one-product add flow whose
//    validation went `ok → idle` must not let the user click Add
//    source until they re-validate.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AtlassianValidationResult } from "@dayseam/ipc-types";
import { AddAtlassianSourceDialog } from "../AddAtlassianSourceDialog";
import {
  mockInvoke,
  registerInvokeHandler,
  resetTauriMocks,
} from "../../../__tests__/tauri-mock";

const ACCOUNT_A: AtlassianValidationResult = {
  account_id: "acct-a",
  display_name: "Workspace A User",
  email: "a@example.com",
};

const ACCOUNT_B: AtlassianValidationResult = {
  account_id: "acct-b",
  display_name: "Workspace B User",
  email: "b@example.com",
};

/** Register a validate handler that returns `ACCOUNT_A` for
 *  workspace-a.atlassian.net and `ACCOUNT_B` for workspace-b.
 *  Routing on the URL rather than call ordinality means the test
 *  asserts on *which* workspace got probed on each Validate click,
 *  not just that two calls happened. */
function registerValidatingHandler() {
  registerInvokeHandler("atlassian_validate_credentials", async (args) => {
    const url = String(args.workspaceUrl ?? "");
    if (url.includes("workspace-a")) return ACCOUNT_A;
    if (url.includes("workspace-b")) return ACCOUNT_B;
    throw new Error(`unexpected workspaceUrl in test: ${url}`);
  });
}

async function fillValidCredentials(workspace: string): Promise<void> {
  fireEvent.change(screen.getByTestId("add-atlassian-workspace-url"), {
    target: { value: workspace },
  });
  fireEvent.change(screen.getByTestId("add-atlassian-email"), {
    target: { value: "vedanth@example.com" },
  });
  fireEvent.change(screen.getByTestId("add-atlassian-api-token"), {
    target: { value: "ATATT-fixture-token" },
  });
}

function validateCallCount(): number {
  return mockInvoke.mock.calls.filter(
    ([name]) => name === "atlassian_validate_credentials",
  ).length;
}

describe("AddAtlassianSourceDialog validate-edit regression (TST-v0.2-05)", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerValidatingHandler();
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("issues a fresh IPC call on the second Validate click after editing the URL", async () => {
    render(
      <AddAtlassianSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        existingSources={[]}
      />,
    );

    await fillValidCredentials("workspace-a.atlassian.net");
    const validateBtn = await screen.findByTestId("add-atlassian-validate");
    fireEvent.click(validateBtn);
    await waitFor(() =>
      expect(screen.getByTestId("add-atlassian-validation-ok")).toHaveTextContent(
        /Workspace A User/,
      ),
    );
    expect(validateCallCount()).toBe(1);

    // User notices the URL was wrong and fixes it. The `useEffect`
    // on `[normalisedUrl, email, apiToken, tokenMode]` must drop
    // the cached `{ kind: "ok" }` back to `{ kind: "idle" }` —
    // asserted by the disappearance of the ok-ribbon below.
    fireEvent.change(screen.getByTestId("add-atlassian-workspace-url"), {
      target: { value: "workspace-b.atlassian.net" },
    });
    await waitFor(() =>
      expect(
        screen.queryByTestId("add-atlassian-validation-ok"),
      ).not.toBeInTheDocument(),
    );

    // Second click must actually probe `workspace-b` — if the
    // dialog cached the first result, this call never fires and
    // the assertion on call count and the per-workspace routing
    // below both fail.
    fireEvent.click(screen.getByTestId("add-atlassian-validate"));
    await waitFor(() =>
      expect(screen.getByTestId("add-atlassian-validation-ok")).toHaveTextContent(
        /Workspace B User/,
      ),
    );
    expect(validateCallCount()).toBe(2);

    // The second call must have routed to workspace-b's URL
    // specifically, not replayed the first call's args.
    const secondCall = mockInvoke.mock.calls.filter(
      ([name]) => name === "atlassian_validate_credentials",
    )[1];
    expect(secondCall?.[1]).toMatchObject({
      workspaceUrl: "https://workspace-b.atlassian.net",
    });
  });

  it("clears the ✓ Connected ribbon as soon as any credential input changes", async () => {
    render(
      <AddAtlassianSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        existingSources={[]}
      />,
    );

    await fillValidCredentials("workspace-a.atlassian.net");
    fireEvent.click(await screen.findByTestId("add-atlassian-validate"));
    await waitFor(() =>
      expect(
        screen.getByTestId("add-atlassian-validation-ok"),
      ).toBeInTheDocument(),
    );

    // Each of the three credential inputs should drop validation
    // back to idle; assert on the email field specifically (the
    // core regression was "url change", so the email variant
    // guards against a narrow fix that only watched URL).
    fireEvent.change(screen.getByTestId("add-atlassian-email"), {
      target: { value: "somebody-else@example.com" },
    });
    await waitFor(() =>
      expect(
        screen.queryByTestId("add-atlassian-validation-ok"),
      ).not.toBeInTheDocument(),
    );
  });

  it("disables the Add source button between ok → edit → ok transitions", async () => {
    render(
      <AddAtlassianSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        existingSources={[]}
      />,
    );

    await fillValidCredentials("workspace-a.atlassian.net");
    fireEvent.click(await screen.findByTestId("add-atlassian-validate"));
    await waitFor(() =>
      expect(
        screen.getByTestId("add-atlassian-validation-ok"),
      ).toBeInTheDocument(),
    );
    const submit = screen.getByRole("button", { name: /add source/i });
    expect(submit).toBeEnabled();

    // Edit invalidates the cached validation → Add source must
    // redisable until the user re-validates. This is the
    // user-visible interlock that stops a stale-ok from
    // round-tripping into `atlassian_sources_add`.
    fireEvent.change(screen.getByTestId("add-atlassian-workspace-url"), {
      target: { value: "workspace-b.atlassian.net" },
    });
    await waitFor(() => expect(submit).toBeDisabled());

    fireEvent.click(screen.getByTestId("add-atlassian-validate"));
    await waitFor(() =>
      expect(screen.getByTestId("add-atlassian-validation-ok")).toHaveTextContent(
        /Workspace B User/,
      ),
    );
    await waitFor(() => expect(submit).toBeEnabled());

    // Wrap a trailing microtask flush so any pending setState
    // from the resolved invoke lands inside the test's act
    // boundary instead of leaking into the next test.
    await act(async () => {});
  });
});
