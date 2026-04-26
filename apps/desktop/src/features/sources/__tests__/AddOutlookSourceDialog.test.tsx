// RTL coverage for `AddOutlookSourceDialog` — the DAY-203 state
// machine that cross-cuts `oauth_begin_login` →
// `oauth_session_status` (poll + event) → `outlook_validate_credentials`
// → `outlook_sources_add`.
//
// Cases pinned by the DAY-203 plan:
//
//   * Happy path: begin → poll completes → validate → add → onAdded
//     is called with the returned `Source`.
//   * `consent_required`: `oauth_session_status` surfaces the Outlook
//     connector's admin-consent code on the `Failed` payload — the
//     dialog renders the admin-routing guidance with the Azure deep
//     link.
//   * Timeout: session stays `Pending` past the sign-in timeout →
//     distinct "sign-in timed out" copy renders.
//   * User cancel: the "Cancel" affordance on the signingIn state
//     fires `oauth_cancel_login` and resets to idle (distinct from
//     the timeout copy).
//   * State-mismatch: `oauth_begin_login` returns a `Failed` view with
//     a generic OAuth failure code → the dialog renders the fallback
//     "sign-in failed" copy.
//   * Duplicate: `outlook_sources_add` rejects with
//     `IPC_OUTLOOK_SOURCE_ALREADY_EXISTS` → the dialog renders the
//     "this calendar is already connected" guidance.
//
// Pattern mirrors `AddGithubSourceDialog.test.tsx` — uses the
// in-process `tauri-mock`, no MSW, no real runtime.

import {
  render,
  screen,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  OAuthSessionView,
  Source,
} from "@dayseam/ipc-types";
import { AddOutlookSourceDialog } from "../AddOutlookSourceDialog";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
} from "../../../__tests__/tauri-mock";

const SESSION_ID = "session-1";
const TENANT_ID = "00000000-1111-2222-3333-444444444444";
const UPN = "alice@contoso.com";
const DISPLAY_NAME = "Alice Smith";

const PENDING_VIEW: OAuthSessionView = {
  id: SESSION_ID,
  provider_id: "microsoft-outlook",
  created_at: "2026-04-26T12:00:00Z",
  status: { kind: "pending" },
};

const COMPLETED_VIEW: OAuthSessionView = {
  ...PENDING_VIEW,
  status: { kind: "completed" },
};

const NEW_OUTLOOK_SOURCE: Source = {
  id: "outlook-new",
  kind: "Outlook",
  label: UPN,
  config: {
    Outlook: { tenant_id: TENANT_ID, user_principal_name: UPN },
  },
  secret_ref: null,
  created_at: "2026-04-26T12:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
};

const FAST_TIMERS = { pollIntervalMs: 5, signInTimeoutMs: 150 };

describe("AddOutlookSourceDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
  });
  afterEach(() => resetTauriMocks());

  it("happy path: begin → complete → validate → add fires onAdded", async () => {
    // The driver returns Pending on begin, then transitions to
    // Completed. We script the poll handler to return Pending on the
    // first tick, Completed afterwards so the state machine has to
    // observe the transition to move forward — matches production
    // where `oauth_begin_login` always hands back Pending.
    let callCount = 0;
    registerInvokeHandler("oauth_begin_login", async () => PENDING_VIEW);
    registerInvokeHandler("oauth_session_status", async () => {
      callCount += 1;
      return callCount === 1 ? PENDING_VIEW : COMPLETED_VIEW;
    });
    registerInvokeHandler("outlook_validate_credentials", async () => ({
      tenant_id: TENANT_ID,
      user_principal_name: UPN,
      display_name: DISPLAY_NAME,
      user_object_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    }));
    registerInvokeHandler(
      "outlook_sources_add",
      async () => NEW_OUTLOOK_SOURCE,
    );

    const onAdded = vi.fn();
    render(
      <AddOutlookSourceDialog
        open
        onClose={() => {}}
        onAdded={onAdded}
        testing={FAST_TIMERS}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /sign in with microsoft/i }));

    // The "Signed in as …" ribbon is the validated-state tell; wait
    // for it instead of poking the status interval by hand.
    await waitFor(() =>
      expect(screen.getByTestId("add-outlook-signed-in")).toBeInTheDocument(),
    );
    expect(
      screen.getByTestId("add-outlook-signed-in").textContent,
    ).toContain(DISPLAY_NAME);
    expect(
      screen.getByTestId("add-outlook-signed-in").textContent,
    ).toContain(UPN);
    expect(
      screen.getByTestId("add-outlook-signed-in").textContent,
    ).toContain(TENANT_ID);

    // Validate doesn't fire `outlook_sources_add`; clicking the
    // primary (now "Add source") must be what triggers the persist.
    fireEvent.click(screen.getByRole("button", { name: /add source/i }));

    await waitFor(() =>
      expect(onAdded).toHaveBeenCalledWith(NEW_OUTLOOK_SOURCE),
    );

    // And the command went over the wire with the right args —
    // including the label the UI defaulted to the validated display
    // name.
    expect(mockInvoke).toHaveBeenCalledWith("outlook_sources_add", {
      sessionId: SESSION_ID,
      label: DISPLAY_NAME,
    });
  });

  it("consent_required: renders admin-routing copy with deep link", async () => {
    registerInvokeHandler("oauth_begin_login", async () => PENDING_VIEW);
    // The driver's polled status carries the admin-consent code. We
    // don't let validate run — the `Failed` transition short-circuits
    // straight to the error block.
    registerInvokeHandler("oauth_session_status", async () => ({
      ...PENDING_VIEW,
      status: {
        kind: "failed",
        code: "outlook.consent_required",
        message: "Admin consent required for this tenant.",
      },
    }));

    render(
      <AddOutlookSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        testing={FAST_TIMERS}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /sign in with microsoft/i }),
    );

    // Admin-consent deep link is the signature affordance here — the
    // copy body alone could match the fallback. The dialog exposes
    // the deep link with a stable testid so the assertion doesn't
    // depend on exact copy wording.
    const link = await screen.findByTestId("add-outlook-admin-consent-link");
    expect(link).toHaveAttribute(
      "href",
      expect.stringContaining("login.microsoftonline.com"),
    );
    expect(link).toHaveAttribute(
      "href",
      expect.stringContaining("adminconsent"),
    );
  });

  it("timeout: sign-in stuck on Pending surfaces the timeout copy", async () => {
    // Real timers, tight poll interval + tight sign-in timeout.
    // Mixing `vi.useFakeTimers` with RTL's `findBy*` is fiddly
    // because jsdom's React-18 act environment toggles between the
    // fake + real timer queues in ways that trip the "not configured
    // to support act(...)" warning mid-suite; the cheapest fix is to
    // lean on a short real-timer timeout and the default `findBy*`
    // 1s polling ceiling.
    registerInvokeHandler("oauth_begin_login", async () => PENDING_VIEW);
    registerInvokeHandler("oauth_session_status", async () => PENDING_VIEW);

    render(
      <AddOutlookSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        testing={{ pollIntervalMs: 5, signInTimeoutMs: 50 }}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /sign in with microsoft/i }),
    );

    const error = await screen.findByTestId("add-outlook-error");
    expect(error.textContent).toMatch(/timed out/i);
  });

  it("user cancel: Cancel button fires oauth_cancel_login and resets", async () => {
    const cancelSpy = vi.fn(async () => PENDING_VIEW);
    registerInvokeHandler("oauth_begin_login", async () => PENDING_VIEW);
    registerInvokeHandler("oauth_session_status", async () => PENDING_VIEW);
    registerInvokeHandler("oauth_cancel_login", cancelSpy);

    render(
      <AddOutlookSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        testing={FAST_TIMERS}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /sign in with microsoft/i }),
    );

    // Wait for the signingIn status hint to render so we know the
    // primary button has flipped to its Cancel variant.
    await screen.findByTestId("add-outlook-signing-in");
    fireEvent.click(screen.getByRole("button", { name: /cancel sign-in/i }));

    await waitFor(() =>
      expect(cancelSpy).toHaveBeenCalledWith({ sessionId: SESSION_ID }),
    );
    // Back to idle — the primary reads "Sign in with Microsoft" again
    // and the signingIn hint is gone.
    await waitFor(() =>
      expect(
        screen.queryByTestId("add-outlook-signing-in"),
      ).not.toBeInTheDocument(),
    );
    expect(
      screen.getByRole("button", { name: /sign in with microsoft/i }),
    ).toBeInTheDocument();
  });

  it("state-mismatch: oauth_begin_login Failed surfaces the error", async () => {
    registerInvokeHandler("oauth_begin_login", async () => ({
      ...PENDING_VIEW,
      status: {
        kind: "failed",
        code: "oauth.state_mismatch",
        message: "PKCE state mismatch.",
      },
    }));

    render(
      <AddOutlookSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        testing={FAST_TIMERS}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /sign in with microsoft/i }),
    );

    const error = await screen.findByTestId("add-outlook-error");
    // No outlookErrorCopy entry for `oauth.state_mismatch`, so the
    // dialog falls back to a generic "Sign-in failed" title + the
    // raw IPC error message. We assert the latter because it's the
    // unambiguous signal the fallback path took.
    expect(error.textContent).toMatch(/PKCE state mismatch|Sign-in failed/i);
  });

  it("duplicate: outlook_sources_add rejection surfaces the guidance", async () => {
    registerInvokeHandler("oauth_begin_login", async () => PENDING_VIEW);
    registerInvokeHandler("oauth_session_status", async () => COMPLETED_VIEW);
    registerInvokeHandler("outlook_validate_credentials", async () => ({
      tenant_id: TENANT_ID,
      user_principal_name: UPN,
      display_name: DISPLAY_NAME,
      user_object_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    }));
    // Tauri surfaces `DayseamError` as `{ data: { code, message, … } }`
    // when the externally-tagged serde shape survives — mirror that
    // here so the dialog's `coerceError` hits the code branch.
    registerInvokeHandler("outlook_sources_add", async () => {
      throw {
        variant: "InvalidConfig",
        data: {
          code: "ipc.outlook.source_already_exists",
          message: "This Outlook calendar is already connected.",
        },
      };
    });

    render(
      <AddOutlookSourceDialog
        open
        onClose={() => {}}
        onAdded={() => {}}
        testing={FAST_TIMERS}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /sign in with microsoft/i }),
    );
    await screen.findByTestId("add-outlook-signed-in");
    fireEvent.click(screen.getByRole("button", { name: /add source/i }));

    const error = await screen.findByTestId("add-outlook-error");
    expect(error.textContent).toMatch(/already/i);
  });
});
