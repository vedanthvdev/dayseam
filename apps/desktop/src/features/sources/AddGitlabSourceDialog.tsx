// Dialog: connect (or reconnect) a self-hosted GitLab instance with a
// Personal Access Token.
//
// The flow is three small steps, presented sequentially rather than
// wizarded because the whole thing fits in one viewport:
//
//   1. Base URL field (with normalisation — see `./base-url.ts`).
//      The field is always first because every downstream action
//      (opening the token page, validating a PAT) depends on it.
//   2. "Open token page" button that shells out via `shell_open` to
//      `https://<host>/-/user_settings/personal_access_tokens?...` so
//      the user lands on GitLab's token page with our suggested name
//      (`Dayseam`) and scopes (`read_api,read_user`) prefilled.
//   3. PAT paste field + "Validate" button that calls the
//      `gitlab_validate_pat` IPC command (added in Task 3.1). On
//      success we capture the `user_id` + `username` GitLab echoed
//      back and enable the final "Add source" submit.
//
// Edit / reconnect mode: when `editing` is passed in, the base URL
// prefills from the existing `SourceConfig::GitLab.base_url` and the
// submit path calls `sources_update` instead of `sources_add`,
// stamping the new `user_id` + `username` if GitLab returned different
// values. That's the "Reconnect" deep link from `SourceErrorCard`:
// same dialog, different entry point.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { GitlabValidationResult, Source } from "@dayseam/ipc-types";
import { Dialog, DialogButton } from "../../components/Dialog";
import { useSources } from "../../ipc";
import { invoke } from "../../ipc/invoke";
import {
  normaliseBaseUrl,
  tokenPageUrl,
  type BaseUrlNormalisation,
} from "./base-url";

interface AddGitlabSourceDialogProps {
  open: boolean;
  onClose: () => void;
  /** Fired after `sources_add` succeeds in create mode. */
  onAdded: (source: Source) => void;
  /** When present the dialog opens in reconnect/edit mode: the base
   *  URL is prefilled and read-only (a reconnect never changes the
   *  host), and the submit path is `sources_update`. */
  editing?: Source | null;
  /** Fired after `sources_update` succeeds in edit mode. */
  onSaved?: (source: Source) => void;
}

function initialBaseUrlForEdit(source: Source | null | undefined): string {
  if (!source) return "";
  if ("GitLab" in source.config) return source.config.GitLab.base_url;
  return "";
}

function initialLabelForEdit(source: Source | null | undefined): string {
  if (!source) return "";
  return source.label;
}

type ValidationState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "ok"; result: GitlabValidationResult }
  | { kind: "error"; message: string };

export function AddGitlabSourceDialog({
  open,
  onClose,
  onAdded,
  editing,
  onSaved,
}: AddGitlabSourceDialogProps) {
  const { add, update } = useSources();
  const isEdit = editing != null;

  const [baseUrlRaw, setBaseUrlRaw] = useState("");
  const [label, setLabel] = useState("");
  const [pat, setPat] = useState("");
  const [validation, setValidation] = useState<ValidationState>({ kind: "idle" });
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  // Re-seed when the dialog opens (or when the caller swaps `editing`
  // under us). We intentionally do NOT prefill the PAT in edit mode —
  // the whole point of reconnect is that the existing PAT no longer
  // works, and we never read it back out of the keychain for display.
  useEffect(() => {
    if (!open) return;
    setBaseUrlRaw(initialBaseUrlForEdit(editing));
    setLabel(initialLabelForEdit(editing));
    setPat("");
    setValidation({ kind: "idle" });
    setSubmitError(null);
    setSubmitting(false);
  }, [open, editing]);

  const normalisation: BaseUrlNormalisation = useMemo(
    () => normaliseBaseUrl(baseUrlRaw),
    [baseUrlRaw],
  );

  const normalisedUrl = normalisation.kind === "ok" ? normalisation.url : null;
  const insecure = normalisation.kind === "ok" && normalisation.insecure;

  // Typing in the URL or PAT invalidates any cached validation: the
  // user is pointing at a different host, or pasted a different
  // token. Running the old result would let them add a source whose
  // identity doesn't match their current input.
  useEffect(() => {
    setValidation((prev) => (prev.kind === "idle" ? prev : { kind: "idle" }));
  }, [normalisedUrl, pat]);

  const canValidate =
    normalisation.kind === "ok" &&
    pat.trim().length > 0 &&
    validation.kind !== "checking";

  const canSubmit =
    normalisation.kind === "ok" &&
    label.trim().length > 0 &&
    validation.kind === "ok" &&
    !submitting;

  const handleValidate = useCallback(async () => {
    if (!canValidate || normalisedUrl == null) return;
    setValidation({ kind: "checking" });
    try {
      const result = await invoke("gitlab_validate_pat", {
        host: normalisedUrl,
        pat: pat.trim(),
      });
      setValidation({ kind: "ok", result });
      if (!isEdit && label.trim().length === 0) {
        // Default the label to the host part of the normalised URL
        // so the common case (one self-hosted instance) has nothing
        // to type beyond the URL + PAT.
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
  }, [canValidate, normalisedUrl, pat, isEdit, label]);

  const handleOpenTokenPage = useCallback(async () => {
    if (normalisation.kind !== "ok") return;
    try {
      await invoke("shell_open", { url: tokenPageUrl(normalisation.url) });
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  }, [normalisation]);

  const handleSubmit = useCallback(async () => {
    if (!canSubmit || validation.kind !== "ok" || normalisation.kind !== "ok") {
      return;
    }
    setSubmitting(true);
    setSubmitError(null);
    try {
      const config = {
        GitLab: {
          base_url: normalisation.url,
          user_id: validation.result.user_id,
          username: validation.result.username,
        },
      } as const;
      if (isEdit && editing) {
        const saved = await update(editing.id, {
          label: label.trim(),
          config,
        });
        onSaved?.(saved);
      } else {
        const added = await add("GitLab", label.trim(), config);
        onAdded(added);
      }
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
      setSubmitting(false);
    }
  }, [
    canSubmit,
    validation,
    normalisation,
    isEdit,
    editing,
    label,
    add,
    update,
    onAdded,
    onSaved,
  ]);

  const handleClose = useCallback(() => {
    if (submitting) return;
    onClose();
  }, [submitting, onClose]);

  const urlHelp = renderUrlHelp(normalisation);

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title={isEdit ? "Reconnect GitLab" : "Add GitLab source"}
      description={
        isEdit
          ? "Paste a fresh Personal Access Token to restore this source. The host stays the same; the existing identity is preserved."
          : "Connect a self-hosted or cloud GitLab instance with a Personal Access Token. Dayseam only needs read access."
      }
      testId="add-gitlab-dialog"
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
              ? isEdit
                ? "Saving…"
                : "Adding…"
              : isEdit
                ? "Save"
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
            GitLab base URL
          </span>
          <input
            type="text"
            value={baseUrlRaw}
            onChange={(e) => setBaseUrlRaw(e.target.value)}
            readOnly={isEdit}
            autoFocus={!isEdit}
            placeholder="gitlab.example.com"
            data-testid="add-gitlab-base-url"
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-sm read-only:cursor-not-allowed read-only:opacity-75 dark:border-neutral-700 dark:bg-neutral-900"
          />
          {urlHelp}
        </label>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => void handleOpenTokenPage()}
            disabled={normalisation.kind !== "ok"}
            data-testid="add-gitlab-open-token-page"
            className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
          >
            Open token page
          </button>
          <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
            Scopes: <code>read_api</code>, <code>read_user</code>.
          </span>
        </div>

        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
            Personal Access Token
          </span>
          <input
            type="password"
            value={pat}
            onChange={(e) => setPat(e.target.value)}
            placeholder="glpat-…"
            data-testid="add-gitlab-pat"
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleValidate()}
              disabled={!canValidate}
              data-testid="add-gitlab-validate"
              className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
            >
              {validation.kind === "checking" ? "Validating…" : "Validate"}
            </button>
            {renderValidationStatus(validation)}
          </div>
        </label>

        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
            Label
          </span>
          <input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="gitlab.example.com"
            data-testid="add-gitlab-label"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </label>

        {insecure ? (
          <p
            role="alert"
            data-testid="add-gitlab-insecure-warning"
            className="rounded border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200"
          >
            Heads up — <code>http://</code> sends your token in cleartext.
            Use <code>https://</code> unless you're on a trusted local
            network.
          </p>
        ) : null}

        {submitError ? (
          <p
            role="alert"
            data-testid="add-gitlab-submit-error"
            className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
          >
            {submitError}
          </p>
        ) : null}
      </form>
    </Dialog>
  );
}

function renderUrlHelp(n: BaseUrlNormalisation) {
  if (n.kind === "ok") {
    return (
      <span
        data-testid="add-gitlab-url-normalised"
        className="text-[11px] text-neutral-500 dark:text-neutral-400"
      >
        Will connect to <code>{n.url}</code>.
      </span>
    );
  }
  if (n.kind === "invalid") {
    return (
      <span
        role="alert"
        data-testid="add-gitlab-url-invalid"
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
          data-testid="add-gitlab-validation-ok"
          className="text-[11px] text-emerald-700 dark:text-emerald-300"
        >
          ✓ Connected as <code>{validation.result.username}</code>
        </span>
      );
    case "error":
      return (
        <span
          role="alert"
          data-testid="add-gitlab-validation-error"
          className="text-[11px] text-red-700 dark:text-red-300"
        >
          {validation.message}
        </span>
      );
    case "checking":
      return (
        <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
          Checking against /api/v4/user…
        </span>
      );
    case "idle":
    default:
      return null;
  }
}

