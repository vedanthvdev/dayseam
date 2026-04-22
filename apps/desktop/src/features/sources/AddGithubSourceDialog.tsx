// Dialog: connect (or reconnect) a GitHub account with a Personal
// Access Token.
//
// Mirrors the shape of `AddGitlabSourceDialog` — one URL, one PAT,
// one label — but hits dedicated GitHub IPC commands
// (`github_validate_credentials`, `github_sources_add`,
// `github_sources_reconnect`) instead of the generic sources mutators
// the GitLab dialog uses. The dedicated commands exist because
// GitHub's `SourceConfig::GitHub` is small and the add flow gets to
// skip the `sources_update` round-trip after validate: a single IPC
// writes the row + keychain + identity atomically (see
// `apps/desktop/src-tauri/src/ipc/github.rs`).
//
// The flow is:
//
//   1. API base URL field — prefilled with
//      `https://api.github.com/` so the common "one-click cloud
//      GitHub" case needs no typing. For GitHub Enterprise the user
//      pastes `https://ghe.example.com/api/v3/`; `github-api-base-
//      url.ts` normalises both shapes into the trailing-slash form
//      the connector joins against. Read-only in reconnect mode.
//   2. "Open token page" button — shells out to `shell_open` on
//      `https://<host>/settings/tokens/new?...` so the user lands on
//      GitHub's token creation page with the name and scopes prefilled.
//   3. PAT field + "Validate" button — fires
//      `github_validate_credentials` and, on success, captures the
//      `{ user_id, login, name }` triple for the "Connected as …"
//      ribbon. Typing in any field resets validation back to `idle`
//      so the submit button cannot round-trip a stale `user_id`.
//   4. Label field — defaults to the host portion of the URL after
//      a successful validate.
//
// Reconnect mode: when `reconnect.source` is supplied the dialog
// operates on that source — URL and label are shown read-only from
// the existing row, the validate button is hidden, and submit calls
// `github_sources_reconnect` directly. The backend re-runs `/user`
// against the stored URL, refuses if the resolved numeric `user_id`
// does not match the `GitHubUserId` identity already bound to the
// source (silent-rebind guard), and rotates the keychain entry in
// place. The caller then fires `sources_healthcheck` to clear the
// red chip. Rotating the GitHub account off a source is deliberately
// out of scope here — deleting and re-adding is the supported path.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { GithubValidationResult, Source } from "@dayseam/ipc-types";
import { Dialog, DialogButton } from "../../components/Dialog";
import { invoke } from "../../ipc/invoke";
import { sourcesBus, SOURCES_CHANGED } from "../../ipc/useSources";
import {
  GITHUB_CLOUD_API_BASE_URL,
  normaliseGithubApiBaseUrl,
  tokenPageUrl,
  type GithubApiBaseUrlNormalisation,
} from "./github-api-base-url";

interface AddGithubSourceDialogProps {
  open: boolean;
  onClose: () => void;
  /** Fired after `github_sources_add` succeeds with the freshly
   *  inserted `Source` row. Not called in reconnect mode; see
   *  `onReconnected`. */
  onAdded: (source: Source) => void;
  /** When set, the dialog mounts in reconnect mode: the API base
   *  URL + label are shown read-only from the passed source, the
   *  validate button is hidden, and submit calls
   *  `github_sources_reconnect` instead of `github_sources_add`. */
  reconnect?: { source: Source } | null;
  /** Fired after `github_sources_reconnect` succeeds with the source
   *  id whose keychain slot was rotated. The caller is expected to
   *  fire `sources_healthcheck` against that id so the red chip on
   *  the sidebar clears immediately without waiting for the next
   *  poll. */
  onReconnected?: (sourceId: string) => void;
}

type ValidationState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "ok"; result: GithubValidationResult }
  | { kind: "error"; message: string };

function initialApiBaseUrlForReconnect(source: Source | null): string {
  if (!source) return GITHUB_CLOUD_API_BASE_URL;
  if ("GitHub" in source.config) return source.config.GitHub.api_base_url;
  return GITHUB_CLOUD_API_BASE_URL;
}

function initialLabelForReconnect(source: Source | null): string {
  return source?.label ?? "";
}

export function AddGithubSourceDialog({
  open,
  onClose,
  onAdded,
  reconnect,
  onReconnected,
}: AddGithubSourceDialogProps) {
  const reconnectSource = reconnect?.source ?? null;
  const isReconnect = reconnectSource != null;

  const [apiBaseUrlRaw, setApiBaseUrlRaw] = useState(GITHUB_CLOUD_API_BASE_URL);
  const [label, setLabel] = useState("");
  const [pat, setPat] = useState("");
  const [validation, setValidation] = useState<ValidationState>({ kind: "idle" });
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  // Re-seed each time the dialog opens (or when the caller swaps the
  // `reconnect` source under us). The PAT field is never prefilled —
  // the whole point of reconnect is that the existing token no
  // longer works, and we never read it back out of the keychain for
  // display.
  useEffect(() => {
    if (!open) return;
    setApiBaseUrlRaw(initialApiBaseUrlForReconnect(reconnectSource));
    setLabel(initialLabelForReconnect(reconnectSource));
    setPat("");
    setValidation({ kind: "idle" });
    setSubmitError(null);
    setSubmitting(false);
  }, [open, reconnectSource]);

  const normalisation: GithubApiBaseUrlNormalisation = useMemo(
    () => normaliseGithubApiBaseUrl(apiBaseUrlRaw),
    [apiBaseUrlRaw],
  );
  const normalisedUrl = normalisation.kind === "ok" ? normalisation.url : null;

  // Typing in the URL or PAT invalidates any cached validation: the
  // user is pointing at a different host or pasted a different
  // token. Running the old result would let them add a source whose
  // identity doesn't match what's on-screen — the DAY-99 invariant
  // the RTL regression test (`AddGithubSourceDialog.validate-edit`)
  // pins.
  useEffect(() => {
    setValidation((prev) => (prev.kind === "idle" ? prev : { kind: "idle" }));
  }, [normalisedUrl, pat]);

  const canValidate =
    !isReconnect &&
    normalisation.kind === "ok" &&
    pat.trim().length > 0 &&
    validation.kind !== "checking";

  const canSubmit = isReconnect
    ? // Reconnect path skips the explicit Validate button — the
      // backend re-runs `/user` as part of
      // `github_sources_reconnect`, so we only need a non-empty PAT
      // client-side.
      pat.trim().length > 0 && !submitting
    : normalisation.kind === "ok" &&
      label.trim().length > 0 &&
      validation.kind === "ok" &&
      !submitting;

  const handleValidate = useCallback(async () => {
    if (!canValidate || normalisedUrl == null) return;
    setValidation({ kind: "checking" });
    try {
      const result = await invoke("github_validate_credentials", {
        apiBaseUrl: normalisedUrl,
        pat: pat.trim(),
      });
      setValidation({ kind: "ok", result });
      if (label.trim().length === 0) {
        try {
          setLabel(new URL(normalisedUrl).hostname);
        } catch {
          /* URL was already validated above; ignore */
        }
      }
    } catch (err) {
      const message =
        err instanceof Error
          ? err.message
          : typeof err === "object" && err != null && "data" in err
            ? JSON.stringify((err as { data: unknown }).data)
            : JSON.stringify(err);
      setValidation({ kind: "error", message });
    }
  }, [canValidate, normalisedUrl, pat, label]);

  const handleOpenTokenPage = useCallback(async () => {
    if (normalisation.kind !== "ok") return;
    try {
      await invoke("shell_open", { url: tokenPageUrl(normalisation.url) });
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  }, [normalisation]);

  const handleSubmit = useCallback(async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      if (isReconnect && reconnectSource != null) {
        const affectedId = await invoke("github_sources_reconnect", {
          sourceId: reconnectSource.id,
          pat: pat.trim(),
        });
        sourcesBus.dispatchEvent(new Event(SOURCES_CHANGED));
        onReconnected?.(affectedId);
        return;
      }
      if (validation.kind !== "ok" || normalisation.kind !== "ok") return;
      const added = await invoke("github_sources_add", {
        apiBaseUrl: normalisation.url,
        label: label.trim(),
        pat: pat.trim(),
        userId: validation.result.user_id,
      });
      sourcesBus.dispatchEvent(new Event(SOURCES_CHANGED));
      onAdded(added);
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
      setSubmitting(false);
    }
  }, [
    canSubmit,
    isReconnect,
    reconnectSource,
    validation,
    normalisation,
    label,
    pat,
    onAdded,
    onReconnected,
  ]);

  const handleClose = useCallback(() => {
    if (submitting) return;
    onClose();
  }, [submitting, onClose]);

  const urlHelp = isReconnect ? null : renderUrlHelp(normalisation);

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title={isReconnect ? "Reconnect GitHub" : "Add GitHub source"}
      description={
        isReconnect
          ? "Paste a fresh Personal Access Token to restore this source. The API base URL stays the same; the bound GitHub account is preserved."
          : "Connect a GitHub (or GitHub Enterprise) account with a Personal Access Token. Dayseam only needs read access."
      }
      testId="add-github-dialog"
      footer={
        <>
          <DialogButton kind="secondary" onClick={handleClose} disabled={submitting}>
            Cancel
          </DialogButton>
          <DialogButton
            kind="primary"
            type="submit"
            disabled={!canSubmit}
            onClick={() => void handleSubmit()}
          >
            {submitting
              ? isReconnect
                ? "Reconnecting…"
                : "Adding…"
              : isReconnect
                ? "Reconnect"
                : "Add source"}
          </DialogButton>
        </>
      }
    >
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          void handleSubmit();
        }}
      >
        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
            GitHub API base URL
          </span>
          <input
            type="text"
            value={apiBaseUrlRaw}
            onChange={(e) => setApiBaseUrlRaw(e.target.value)}
            readOnly={isReconnect}
            autoFocus={!isReconnect}
            placeholder="https://api.github.com/"
            data-testid="add-github-api-base-url"
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-sm read-only:cursor-not-allowed read-only:opacity-75 dark:border-neutral-700 dark:bg-neutral-900"
          />
          {urlHelp}
        </label>

        {!isReconnect ? (
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleOpenTokenPage()}
              disabled={normalisation.kind !== "ok"}
              data-testid="add-github-open-token-page"
              className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
            >
              Open token page
            </button>
            <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
              Scopes: <code>repo</code>, <code>read:org</code>, <code>read:user</code>.
            </span>
          </div>
        ) : null}

        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
            Personal Access Token
          </span>
          <input
            type="password"
            value={pat}
            onChange={(e) => setPat(e.target.value)}
            placeholder="ghp_…"
            data-testid="add-github-pat"
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
          {isReconnect ? (
            <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
              The new token is validated against the bound GitHub
              account when you click Reconnect.
            </span>
          ) : (
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void handleValidate()}
                disabled={!canValidate}
                data-testid="add-github-validate"
                className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
              >
                {validation.kind === "checking" ? "Validating…" : "Validate"}
              </button>
              {renderValidationStatus(validation)}
            </div>
          )}
        </label>

        {!isReconnect ? (
          <label className="flex flex-col gap-1">
            <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
              Label
            </span>
            <input
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="api.github.com"
              data-testid="add-github-label"
              className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          </label>
        ) : null}

        {submitError ? (
          <p
            role="alert"
            data-testid="add-github-submit-error"
            className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
          >
            {submitError}
          </p>
        ) : null}
      </form>
    </Dialog>
  );
}

function renderUrlHelp(n: GithubApiBaseUrlNormalisation) {
  if (n.kind === "ok") {
    return (
      <span
        data-testid="add-github-url-normalised"
        className="text-[11px] text-neutral-500 dark:text-neutral-400"
      >
        Will connect to <code>{n.url}</code>
        {n.isCloud ? " (GitHub cloud)" : " (GitHub Enterprise)"}.
      </span>
    );
  }
  if (n.kind === "invalid") {
    return (
      <span
        role="alert"
        data-testid="add-github-url-invalid"
        className="text-[11px] text-red-700 dark:text-red-300"
      >
        {n.reason}
      </span>
    );
  }
  return null;
}

function renderValidationStatus(validation: ValidationState) {
  switch (validation.kind) {
    case "ok":
      return (
        <span
          data-testid="add-github-validation-ok"
          className="text-[11px] text-emerald-700 dark:text-emerald-300"
        >
          ✓ Connected as{" "}
          <code>{validation.result.name ?? validation.result.login}</code>
          {validation.result.name ? ` (@${validation.result.login})` : ""}
        </span>
      );
    case "error":
      return (
        <span
          role="alert"
          data-testid="add-github-validation-error"
          className="text-[11px] text-red-700 dark:text-red-300"
        >
          {validation.message}
        </span>
      );
    case "checking":
      return (
        <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
          Checking /user…
        </span>
      );
    case "idle":
    default:
      return null;
  }
}
