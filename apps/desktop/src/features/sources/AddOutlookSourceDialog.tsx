// Dialog: connect an Outlook calendar via the PKCE loopback OAuth
// flow introduced in DAY-201.
//
// Shape differs from every other connector dialog because Outlook
// doesn't use a PAT. The user clicks "Sign in with Microsoft", the
// OS opens Azure AD's consent screen in their default browser, and
// Dayseam's loopback listener captures the authorization code that
// comes back. The UI state machine tracks the four stages that
// cross-cut that flow:
//
//   idle       — nothing started yet; primary button reads
//                "Sign in with Microsoft" and fires `oauth_begin_login`.
//   signingIn  — `oauth_begin_login` returned; the UI polls
//                `oauth_session_status` (and listens to the
//                `oauth://session-updated` event when the runtime is
//                available). Primary button swaps to "Cancel", which
//                fires `oauth_cancel_login` and drops back to `idle`.
//   validated  — session reached `Completed`, `outlook_validate_credentials`
//                returned a user principal. We render "Signed in as
//                <display_name> (<upn>) — <tenant_id>" and unlock the
//                "Add source" primary.
//   submitting — `outlook_sources_add` in flight. Disables every
//                button except Cancel (which is disabled while the
//                commit runs because rolling back a partial commit is
//                the Rust side's responsibility, not the UI's).
//
// Terminal outcomes:
//
//   * success — `onAdded(source)` fires, the dialog closes.
//   * error   — `state` flips to `{ kind: "error", code, message }`
//               where `code` is resolved through `outlookErrorCopy`
//               for known `outlook.*` / `ipc.outlook.*` codes, and
//               falls back to the raw IPC error message for
//               everything else.
//
// Poll cadence is 750ms — fast enough that a quick consent feels
// snappy (the browser tab closes right after consent, so the first
// poll after that is usually the Completed transition) but slow
// enough not to drown the background runtime in status IPC calls
// when the user has stepped away from the browser tab for a minute.
// The 60s timeout mirrors the server-side `OAUTH_SESSION_TIMEOUT` so
// the dialog's "sign-in timed out" copy aligns with the Rust side
// tearing the session down.
//
// This dialog deliberately does not implement reconnect mode — for
// Outlook, "reconnect" means re-running the whole sign-in, which
// reuses the add flow verbatim (just with a pre-filled label). The
// parent (`SourcesSidebar`) mounts two instances of this dialog: one
// for add and one for reconnect; the latter pre-fills `initialLabel`
// from the existing source's label and surfaces "Reauthorize" copy.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  OAuthSessionId,
  OAuthSessionView,
  OutlookErrorCode,
  OutlookValidationResult,
  Source,
} from "@dayseam/ipc-types";
import { OUTLOOK_ERROR_CODES } from "@dayseam/ipc-types";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Dialog, DialogButton } from "../../components/Dialog";
import { invoke } from "../../ipc/invoke";
import { sourcesBus, SOURCES_CHANGED } from "../../ipc/useSources";
import { outlookErrorCopy } from "./outlookErrorCopy";

/** Provider identifier baked into the Rust `PROVIDER_MICROSOFT_OUTLOOK`
 *  constant. Duplicated here rather than re-exported from `ipc-types`
 *  because the TS-generated command map already refers to it as an
 *  opaque string; a typo fails at runtime either way, and surfacing
 *  it as a dedicated type would bloat the generated surface. */
const PROVIDER_ID = "microsoft-outlook";

/** Polling interval for `oauth_session_status`, in ms. Gated behind
 *  `globalThis` so tests can override it via Vitest's `vi.setConfig`
 *  without monkey-patching module-scope state. */
const DEFAULT_POLL_INTERVAL_MS = 750;

/** How long to keep polling before calling the session timed out.
 *  Mirrors the server-side `OAUTH_SESSION_TIMEOUT` so the user-facing
 *  copy aligns with the Rust side tearing the session down. */
const DEFAULT_SIGN_IN_TIMEOUT_MS = 60_000;

/** Name of the Tauri event emitted on every status transition. Must
 *  match the `SESSION_EVENT` constant in `ipc/oauth.rs`. */
const SESSION_EVENT = "oauth://session-updated";

interface AddOutlookSourceDialogProps {
  open: boolean;
  onClose: () => void;
  /** Fired after `outlook_sources_add` succeeds with the freshly
   *  inserted `Source` row. */
  onAdded: (source: Source) => void;
  /** When set, the dialog mounts in reconnect mode: the same sign-in
   *  flow, but the label input pre-fills from the reconnecting row
   *  and the dialog title/primary-button copy switches to
   *  "Reauthorize". The underlying IPC is still
   *  `outlook_sources_add`; reconnect for Outlook is "delete + add"
   *  semantically, and the Rust side detects the
   *  `IPC_OUTLOOK_SOURCE_ALREADY_EXISTS` collision so the user
   *  explicitly hits the duplicate path. */
  reconnect?: { source: Source } | null;
  /** Fired after a successful reconnect. In Outlook's add-is-reconnect
   *  model this is effectively the `onAdded` callback under a
   *  different name so the parent sidebar can wire the "clear the
   *  red chip" healthcheck it already has for GitHub/Atlassian. */
  onReconnected?: (sourceId: string) => void;
  /** Test hook. Lets a vitest suite collapse the poll interval / sign-in
   *  timeout so a fake-timer advance can traverse the state machine
   *  in tens of milliseconds instead of tens of seconds. Production
   *  callers leave this unset. */
  testing?: {
    pollIntervalMs?: number;
    signInTimeoutMs?: number;
  };
}

type DialogState =
  | { kind: "idle" }
  | {
      kind: "signingIn";
      sessionId: OAuthSessionId;
      startedAt: number;
    }
  | {
      kind: "validated";
      sessionId: OAuthSessionId;
      result: OutlookValidationResult;
    }
  | {
      kind: "submitting";
      sessionId: OAuthSessionId;
      result: OutlookValidationResult;
    }
  | {
      kind: "error";
      code: string;
      message: string;
      // Keep the session id around so the user can retry validate
      // without restarting sign-in when the failure is transient
      // (Graph 5xx / rate-limit). Absent for pre-session errors
      // (e.g. `oauth_begin_login` itself failed).
      sessionId: OAuthSessionId | null;
    };

const KNOWN_OUTLOOK_CODES: ReadonlySet<string> = new Set<string>(
  OUTLOOK_ERROR_CODES,
);

/** Extract the stable `code` string out of whatever a Tauri
 *  `#[tauri::command]` failure looks like on the TS side. Tauri
 *  surfaces `DayseamError` as `{ data: { code, message, … } }` when
 *  serde's externally-tagged shape survives the round-trip; when the
 *  command rejects with a plain `Error` (no DayseamError mapping)
 *  the caller sees a raw `Error` instance. Both are handled here so
 *  one call site can branch on a stable `code` without repeating the
 *  unwrap dance at every callsite. */
function coerceError(err: unknown): { code: string; message: string } {
  if (err && typeof err === "object") {
    const obj = err as { data?: unknown; message?: unknown };
    if (obj.data && typeof obj.data === "object") {
      const data = obj.data as { code?: unknown; message?: unknown };
      if (typeof data.code === "string") {
        const message =
          typeof data.message === "string" ? data.message : String(err);
        return { code: data.code, message };
      }
    }
    if (typeof obj.message === "string") {
      return { code: "unknown", message: obj.message };
    }
  }
  if (err instanceof Error) {
    return { code: "unknown", message: err.message };
  }
  return { code: "unknown", message: JSON.stringify(err) };
}

export function AddOutlookSourceDialog({
  open,
  onClose,
  onAdded,
  reconnect,
  onReconnected,
  testing,
}: AddOutlookSourceDialogProps) {
  const reconnectSource = reconnect?.source ?? null;
  const isReconnect = reconnectSource != null;

  const pollIntervalMs =
    testing?.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const signInTimeoutMs =
    testing?.signInTimeoutMs ?? DEFAULT_SIGN_IN_TIMEOUT_MS;

  const [label, setLabel] = useState("");
  const [state, setState] = useState<DialogState>({ kind: "idle" });

  // Latest state value for async callbacks that would otherwise
  // capture a stale closure. Using a ref instead of threading the
  // value through every dependency list keeps the reducer-like
  // state transitions self-contained without the extra ceremony of
  // actually reaching for `useReducer`.
  const stateRef = useRef<DialogState>(state);
  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  // Extracted so the polling + listener effects below can carry a
  // stable primitive on their dep arrays instead of the ternary the
  // `react-hooks/exhaustive-deps` rule can't statically verify.
  const signingInSessionId =
    state.kind === "signingIn" ? state.sessionId : null;

  // Re-seed when the dialog opens or the reconnect source changes.
  useEffect(() => {
    if (!open) return;
    setLabel(reconnectSource?.label ?? "");
    setState({ kind: "idle" });
  }, [open, reconnectSource]);

  // ── Event listener for `oauth://session-updated` ───────────────
  //
  // The Rust background driver emits this on every status transition
  // so the UI can react without waiting for the next poll tick. We
  // still keep polling as a safety net because the listener isn't
  // guaranteed to attach before the session completes (a fast path
  // that returns tokens before the `listen` promise resolves would
  // leave us blind to the transition).
  useEffect(() => {
    if (state.kind !== "signingIn") return;
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    void listen<OAuthSessionView>(SESSION_EVENT, (event) => {
      const s = stateRef.current;
      if (s.kind !== "signingIn") return;
      if (event.payload.id !== s.sessionId) return;
      void handleStatusTransition(event.payload);
    }).then((u) => {
      if (cancelled) {
        u();
        return;
      }
      unlisten = u;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
    // `handleStatusTransition` is a stable closure; including it
    // would make this effect re-subscribe on every render which
    // would double-fire the listener. The state-guard above reads
    // through `stateRef` so staleness isn't a concern.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.kind, signingInSessionId]);

  // ── Polling loop ─────────────────────────────────────────────────
  useEffect(() => {
    if (state.kind !== "signingIn") return;
    const sessionId = state.sessionId;
    const startedAt = state.startedAt;
    let cancelled = false;

    const tick = async () => {
      if (cancelled) return;
      if (Date.now() - startedAt >= signInTimeoutMs) {
        setState({
          kind: "error",
          code: "oauth.timeout",
          message:
            "Sign-in timed out. Open the browser tab Dayseam launched and complete the Microsoft consent, or start again.",
          sessionId,
        });
        return;
      }
      try {
        const view = await invoke("oauth_session_status", { sessionId });
        if (cancelled) return;
        if (view === null) {
          setState({
            kind: "error",
            code: "ipc.outlook.session_not_found",
            message:
              "The sign-in session disappeared before completing. Start again.",
            sessionId: null,
          });
          return;
        }
        await handleStatusTransition(view);
      } catch (err) {
        if (cancelled) return;
        const { code, message } = coerceError(err);
        setState({ kind: "error", code, message, sessionId });
      }
    };

    const handle = setInterval(() => {
      void tick();
    }, pollIntervalMs);
    // First tick without waiting a full interval so a fast-completing
    // browser round-trip doesn't sit on a stale Pending status.
    void tick();
    return () => {
      cancelled = true;
      clearInterval(handle);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.kind, signingInSessionId, pollIntervalMs, signInTimeoutMs]);

  // ── State transitions ────────────────────────────────────────────
  const handleStatusTransition = useCallback(
    async (view: OAuthSessionView) => {
      const s = stateRef.current;
      if (s.kind !== "signingIn") return;
      if (view.id !== s.sessionId) return;
      const status = view.status;
      if (status.kind === "pending") return;
      if (status.kind === "cancelled") {
        setState({ kind: "idle" });
        return;
      }
      if (status.kind === "failed") {
        setState({
          kind: "error",
          code: status.code,
          message: status.message,
          sessionId: s.sessionId,
        });
        return;
      }
      // kind === "completed" — probe Graph for the "Signed in as …"
      // ribbon. Don't consume the session here; `outlook_sources_add`
      // does that.
      try {
        const result = await invoke("outlook_validate_credentials", {
          sessionId: s.sessionId,
        });
        setState({
          kind: "validated",
          sessionId: s.sessionId,
          result,
        });
        if (label.trim().length === 0) {
          const fallback =
            result.display_name ?? result.user_principal_name;
          setLabel(fallback);
        }
      } catch (err) {
        const { code, message } = coerceError(err);
        setState({
          kind: "error",
          code,
          message,
          sessionId: s.sessionId,
        });
      }
    },
    [label],
  );

  const handleSignIn = useCallback(async () => {
    if (stateRef.current.kind !== "idle" && stateRef.current.kind !== "error")
      return;
    try {
      const view = await invoke("oauth_begin_login", {
        providerId: PROVIDER_ID,
      });
      if (view.status.kind === "failed") {
        setState({
          kind: "error",
          code: view.status.code,
          message: view.status.message,
          sessionId: view.id,
        });
        return;
      }
      setState({
        kind: "signingIn",
        sessionId: view.id,
        startedAt: Date.now(),
      });
    } catch (err) {
      const { code, message } = coerceError(err);
      setState({ kind: "error", code, message, sessionId: null });
    }
  }, []);

  const handleCancelSignIn = useCallback(async () => {
    const s = stateRef.current;
    if (s.kind !== "signingIn") return;
    const sessionId = s.sessionId;
    try {
      await invoke("oauth_cancel_login", { sessionId });
    } catch {
      // Cancel is best-effort: if the session already terminated
      // (Completed / Failed) the IPC returns `null` and we reset
      // anyway. A network failure on cancel also drops us back to
      // idle — the background driver tears down on timeout.
    }
    setState({ kind: "idle" });
  }, []);

  const handleSubmit = useCallback(async () => {
    const s = stateRef.current;
    if (s.kind !== "validated") return;
    setState({
      kind: "submitting",
      sessionId: s.sessionId,
      result: s.result,
    });
    try {
      const source = await invoke("outlook_sources_add", {
        sessionId: s.sessionId,
        label: label.trim(),
      });
      sourcesBus.dispatchEvent(new Event(SOURCES_CHANGED));
      if (isReconnect) {
        onReconnected?.(source.id);
      } else {
        onAdded(source);
      }
    } catch (err) {
      const { code, message } = coerceError(err);
      setState({
        kind: "error",
        code,
        message,
        sessionId: s.sessionId,
      });
    }
  }, [label, isReconnect, onAdded, onReconnected]);

  const handleRetryAfterError = useCallback(() => {
    setState({ kind: "idle" });
  }, []);

  const canCloseFromOutside =
    state.kind !== "signingIn" && state.kind !== "submitting";
  const handleClose = useCallback(() => {
    if (!canCloseFromOutside) return;
    onClose();
  }, [canCloseFromOutside, onClose]);

  // ── Render helpers ───────────────────────────────────────────────
  const errorCopy = useMemo(() => {
    if (state.kind !== "error") return null;
    if (KNOWN_OUTLOOK_CODES.has(state.code)) {
      return outlookErrorCopy[state.code as OutlookErrorCode];
    }
    if (state.code === "oauth.timeout") {
      return {
        title: "Sign-in timed out",
        body: state.message,
        action: "retry" as const,
      };
    }
    return null;
  }, [state]);

  const trimmedLabel = label.trim();
  const canSubmit =
    state.kind === "validated" && trimmedLabel.length > 0;

  const primaryButtonLabel = (() => {
    switch (state.kind) {
      case "idle":
      case "error":
        return isReconnect ? "Reauthorize with Microsoft" : "Sign in with Microsoft";
      case "signingIn":
        return "Cancel sign-in";
      case "validated":
        return isReconnect ? "Reauthorize source" : "Add source";
      case "submitting":
        return isReconnect ? "Reauthorizing…" : "Adding…";
    }
  })();

  const primaryButtonOnClick = (() => {
    switch (state.kind) {
      case "idle":
      case "error":
        return () => void handleSignIn();
      case "signingIn":
        return () => void handleCancelSignIn();
      case "validated":
        return () => void handleSubmit();
      case "submitting":
        return () => undefined;
    }
  })();

  const primaryButtonDisabled = (() => {
    switch (state.kind) {
      case "idle":
      case "error":
        return false;
      case "signingIn":
        return false;
      case "validated":
        return !canSubmit;
      case "submitting":
        return true;
    }
  })();

  const primaryButtonKind =
    state.kind === "signingIn" ? "secondary" : "primary";

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title={isReconnect ? "Reauthorize Outlook source" : "Add Outlook calendar"}
      description={
        isReconnect
          ? "Microsoft will walk you through the sign-in again and Dayseam will rotate the stored tokens. The existing source row stays the same."
          : "Connect a Microsoft 365 account so Dayseam can read your calendar for the Meetings section of the end-of-day report."
      }
      testId="add-outlook-dialog"
      footer={
        <>
          <DialogButton
            kind="secondary"
            onClick={handleClose}
            disabled={!canCloseFromOutside}
          >
            Close
          </DialogButton>
          <DialogButton
            kind={primaryButtonKind}
            onClick={primaryButtonOnClick}
            disabled={primaryButtonDisabled}
          >
            {primaryButtonLabel}
          </DialogButton>
        </>
      }
    >
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (canSubmit) void handleSubmit();
        }}
      >
        <section
          data-testid="add-outlook-status"
          className="rounded border border-neutral-200 bg-neutral-50 px-3 py-2 text-xs text-neutral-700 dark:border-neutral-800 dark:bg-neutral-900 dark:text-neutral-200"
        >
          {state.kind === "idle" ? (
            <p>
              Click <strong>{primaryButtonLabel}</strong> to open Microsoft's
              consent screen in your default browser. Dayseam needs
              <code className="mx-1 rounded bg-neutral-200 px-1 text-[10px] dark:bg-neutral-800">
                Calendars.Read
              </code>
              and
              <code className="mx-1 rounded bg-neutral-200 px-1 text-[10px] dark:bg-neutral-800">
                User.Read
              </code>
              and will request
              <code className="mx-1 rounded bg-neutral-200 px-1 text-[10px] dark:bg-neutral-800">
                offline_access
              </code>
              so refreshing the token doesn't require another consent.
            </p>
          ) : null}
          {state.kind === "signingIn" ? (
            <p data-testid="add-outlook-signing-in">
              Waiting for Microsoft… finish the consent in your browser, then
              come back to this window. Cancel and try again if the browser
              tab never opened.
            </p>
          ) : null}
          {state.kind === "validated" || state.kind === "submitting" ? (
            <p data-testid="add-outlook-signed-in">
              Signed in as{" "}
              <strong>
                {state.result.display_name ?? state.result.user_principal_name}
              </strong>{" "}
              <span className="text-neutral-500 dark:text-neutral-400">
                ({state.result.user_principal_name})
              </span>
              <br />
              <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
                Tenant <code>{state.result.tenant_id}</code>
              </span>
            </p>
          ) : null}
          {state.kind === "error" ? (
            <ErrorBlock
              title={errorCopy?.title ?? "Sign-in failed"}
              body={errorCopy?.body ?? state.message}
              adminConsentUrl={
                errorCopy && "adminConsentUrl" in errorCopy
                  ? errorCopy.adminConsentUrl
                  : undefined
              }
              rawMessage={errorCopy ? null : state.message}
              onRetry={handleRetryAfterError}
            />
          ) : null}
        </section>

        {(state.kind === "validated" || state.kind === "submitting") ? (
          <label className="flex flex-col gap-1">
            <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
              Label
            </span>
            <input
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={
                state.kind === "validated" || state.kind === "submitting"
                  ? state.result.user_principal_name
                  : ""
              }
              data-testid="add-outlook-label"
              disabled={state.kind === "submitting"}
              className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900"
            />
            <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
              Shown on the sources strip. Defaults to the display name if left
              blank.
            </span>
          </label>
        ) : null}
      </form>
    </Dialog>
  );
}

interface ErrorBlockProps {
  title: string;
  body: string;
  adminConsentUrl: string | undefined;
  rawMessage: string | null;
  onRetry: () => void;
}

function ErrorBlock({
  title,
  body,
  adminConsentUrl,
  rawMessage,
  onRetry,
}: ErrorBlockProps) {
  return (
    <div
      role="alert"
      data-testid="add-outlook-error"
      className="flex flex-col gap-2 rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
    >
      <strong>{title}</strong>
      <span>
        {body}
        {adminConsentUrl ? (
          <a
            data-testid="add-outlook-admin-consent-link"
            href={adminConsentUrl}
            target="_blank"
            rel="noreferrer noopener"
            className="ml-1 underline"
          >
            {adminConsentUrl}
          </a>
        ) : null}
      </span>
      {rawMessage ? (
        <span className="font-mono text-[10px] text-red-700 dark:text-red-300">
          {rawMessage}
        </span>
      ) : null}
      <div>
        <button
          type="button"
          onClick={onRetry}
          data-testid="add-outlook-retry"
          className="rounded border border-red-300 bg-white px-2 py-0.5 text-[11px] font-medium text-red-800 hover:bg-red-100 dark:border-red-700 dark:bg-red-950 dark:text-red-200 dark:hover:bg-red-900"
        >
          Try again
        </button>
      </div>
    </div>
  );
}
