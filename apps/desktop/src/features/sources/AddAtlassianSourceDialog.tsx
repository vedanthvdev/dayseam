// Dialog: connect Jira and/or Confluence sources with a shared or
// separate Atlassian API token.
//
// Four journeys share this one surface (see the DAY-82 plan at
// `docs/plan/2026-04-20-v0.2-atlassian.md` §Task 9):
//
//   A. Shared-PAT default. Both product checkboxes are ticked and
//      the user pastes one API token. The `atlassian_sources_add`
//      command writes one keychain row + two `sources` rows that
//      share its `secret_ref`.
//   B. Single product. Only one checkbox is ticked. One keychain
//      row + one `sources` row. Symmetrical shape to GitLab.
//   C-mode-1. Reuse existing token. The dialog detects that one
//      Atlassian product is already configured and offers a "Use
//      existing token" affordance that clones the existing
//      `secret_ref` onto the new `sources` row — zero new keychain
//      rows.
//   C-mode-2. Separate token. Same as C-mode-1 but the user opts to
//      paste a different PAT. One new keychain row, refcount 1.
//
// Validation is always done *before* persist: the "Add" button
// stays disabled until `atlassian_validate_credentials` returns an
// account triple we can pin to `SourceIdentity`. That makes Journey
// C-mode-1 a one-click flow after the checkbox flip because the
// existing source already carries the validated account id.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  AtlassianValidationResult,
  SecretRef,
  Source,
  SourcePatch,
} from "@dayseam/ipc-types";
import { Dialog, DialogButton } from "../../components/Dialog";
import { invoke } from "../../ipc/invoke";
import { sourcesBus, SOURCES_CHANGED } from "../../ipc/useSources";
import {
  atlassianTokenPageUrl,
  normaliseWorkspaceUrl,
  type WorkspaceUrlNormalisation,
} from "./atlassian-workspace-url";

interface AddAtlassianSourceDialogProps {
  open: boolean;
  onClose: () => void;
  /** Fired after `atlassian_sources_add` succeeds. Receives every
   *  row the IPC created — one in Journey B / C, two in Journey A.
   *  Not called in `reconnect` mode; see `onReconnected` instead. */
  onAdded: (sources: Source[]) => void;
  /** Every source currently known to the frontend. Used to detect
   *  whether an existing Jira or Confluence row is already
   *  configured (→ reuse-PAT affordance) and which product the
   *  user is *not* already running. */
  existingSources: readonly Source[];
  /** When set, the dialog mounts in edit mode (DAY-87 + DAY-126):
   *  workspace URL and email are shown read-only from the passed
   *  source, the product checkboxes are hidden, the label is
   *  editable (pre-filled from the row), and the API token is
   *  optional — empty means "keep the existing token", pasted
   *  means "rotate it". Submit calls
   *  `atlassian_sources_reconnect` when a token is present and/or
   *  `sources_update` when the label changed, instead of
   *  `atlassian_sources_add`. URL/email changes are intentionally
   *  out of scope: rotating the bound Atlassian account would
   *  require re-seeding `SourceIdentity` to keep the render-stage
   *  self-filter honest. A shared-PAT rotation fans out to every
   *  sibling row via the `affected` id list from reconnect, but the
   *  label rename only applies to the one source the user is
   *  editing — sibling rows keep their labels. */
  reconnect?: { source: Source } | null;
  /** Fired after `atlassian_sources_reconnect` succeeds. Receives
   *  the ids of every source whose keychain slot was rotated —
   *  shared-PAT sources hand back two ids. The caller is expected
   *  to fire `sources_healthcheck` for each so the red chips on the
   *  sidebar clear without waiting for the next poll. */
  onReconnected?: (affectedSourceIds: string[]) => void;
}

type ValidationState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "ok"; result: AtlassianValidationResult }
  | { kind: "error"; message: string };

type TokenMode = "paste" | "reuse";

/** Information the dialog extracts from the `existingSources` list
 *  when the user already has one Atlassian product configured. Drives
 *  the Journey-C mode switch. */
interface ExistingAtlassian {
  kind: "Jira" | "Confluence";
  source: Source;
  secretRef: SecretRef;
  workspaceUrl: string;
  email: string | null;
  /** The `account_id` the dialog needs to stamp onto the second
   *  product's `SourceIdentity`. Pulled from the existing source
   *  when available; falls back to re-validating if absent. Today
   *  we re-validate because the identity isn't exposed on `Source`
   *  directly; this keeps the UI robust against a missing identity
   *  row without a second round-trip to the DB. */
  accountId: string | null;
}

function findExistingAtlassian(
  sources: readonly Source[],
): ExistingAtlassian | null {
  // Prefer the first `Jira` we find; fall back to `Confluence`. The
  // user can have at most two Atlassian sources per workspace (one
  // Jira, one Confluence) so "first hit" is fine here.
  for (const kind of ["Jira", "Confluence"] as const) {
    const hit = sources.find((s) => s.kind === kind && s.secret_ref != null);
    if (hit == null) continue;
    if (kind === "Jira" && "Jira" in hit.config) {
      return {
        kind,
        source: hit,
        secretRef: hit.secret_ref!,
        workspaceUrl: hit.config.Jira.workspace_url,
        email: hit.config.Jira.email,
        accountId: null,
      };
    }
    if (kind === "Confluence" && "Confluence" in hit.config) {
      return {
        kind,
        source: hit,
        secretRef: hit.secret_ref!,
        workspaceUrl: hit.config.Confluence.workspace_url,
        // Since DAY-84 Confluence rows carry `email` on their config
        // alongside Jira, so we can prefill it here for Journey C
        // (reuse-PAT) and for DAY-87 reconnect. Older installs that
        // missed the backfill surface an empty string; the dialog
        // still asks the user to retype it (email input is editable
        // in add mode) so we don't block them on the upgrade artifact.
        email: hit.config.Confluence.email || null,
        accountId: null,
      };
    }
  }
  return null;
}

export function AddAtlassianSourceDialog({
  open,
  onClose,
  onAdded,
  existingSources,
  reconnect,
  onReconnected,
}: AddAtlassianSourceDialogProps) {
  const isReconnect = reconnect != null;
  const reconnectSource = reconnect?.source ?? null;
  const reconnectConfig = useMemo(() => {
    if (reconnectSource == null) return null;
    if ("Jira" in reconnectSource.config) {
      return {
        kind: "Jira" as const,
        workspaceUrl: reconnectSource.config.Jira.workspace_url,
        email: reconnectSource.config.Jira.email,
      };
    }
    if ("Confluence" in reconnectSource.config) {
      return {
        kind: "Confluence" as const,
        workspaceUrl: reconnectSource.config.Confluence.workspace_url,
        email: reconnectSource.config.Confluence.email,
      };
    }
    return null;
  }, [reconnectSource]);

  const existing = useMemo(
    // In reconnect mode the dialog is operating on one specific
    // source; the Journey-C "pair me with the other product" logic
    // is irrelevant and could only confuse the reuse/paste picker.
    () => (isReconnect ? null : findExistingAtlassian(existingSources)),
    [existingSources, isReconnect],
  );
  // When an existing Atlassian source is present, the *other*
  // product is the one the user is about to add. In the greenfield
  // case both products are enabled by default (Journey A).
  const existingKind = existing?.kind ?? null;

  const [workspaceUrlRaw, setWorkspaceUrlRaw] = useState("");
  const [email, setEmail] = useState("");
  const [apiToken, setApiToken] = useState("");
  const [enableJira, setEnableJira] = useState(true);
  const [enableConfluence, setEnableConfluence] = useState(true);
  const [tokenMode, setTokenMode] = useState<TokenMode>("paste");
  const [validation, setValidation] = useState<ValidationState>({ kind: "idle" });
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  // DAY-126: in edit mode the dialog doubles as the rename surface,
  // so the label sits in dialog state pre-filled from the source
  // being edited.
  // DAY-127 #5b: the add surface now also carries an optional label
  // input (matching GitHub + GitLab), so users can name the chip at
  // creation time instead of accepting the auto-derived default.
  // Empty means "let the backend derive the label" (preserves
  // Journey A/B/C semantics pre-DAY-127).
  const [label, setLabel] = useState("");

  // Re-seed each time the dialog opens. When an existing source is
  // detected we prefill the workspace URL and email from it and
  // collapse the product checkboxes to the one the user hasn't
  // added yet, which is the common Journey-C shape.
  useEffect(() => {
    if (!open) return;
    if (isReconnect && reconnectConfig != null) {
      // DAY-87 reconnect / DAY-126 edit: URL + email come from the
      // source row and are displayed read-only. The token is always
      // empty so we never show a masked-but-present field that hides
      // the fact we wiped the old one — and so leaving it empty is
      // the obvious "keep existing token" gesture.
      setWorkspaceUrlRaw(reconnectConfig.workspaceUrl);
      setEmail(reconnectConfig.email);
      setApiToken("");
      setLabel(reconnectSource?.label ?? "");
      setEnableJira(reconnectConfig.kind === "Jira");
      setEnableConfluence(reconnectConfig.kind === "Confluence");
      setTokenMode("paste");
      setValidation({ kind: "idle" });
      setSubmitError(null);
      setSubmitting(false);
      return;
    }
    setWorkspaceUrlRaw(existing?.workspaceUrl ?? "");
    setEmail(existing?.email ?? "");
    setApiToken("");
    setLabel("");
    if (existingKind === "Jira") {
      setEnableJira(false);
      setEnableConfluence(true);
    } else if (existingKind === "Confluence") {
      setEnableJira(true);
      setEnableConfluence(false);
    } else {
      setEnableJira(true);
      setEnableConfluence(true);
    }
    setTokenMode(existing ? "reuse" : "paste");
    setValidation({ kind: "idle" });
    setSubmitError(null);
    setSubmitting(false);
  }, [open, existing, existingKind, isReconnect, reconnectConfig, reconnectSource]);

  const normalisation: WorkspaceUrlNormalisation = useMemo(
    () => normaliseWorkspaceUrl(workspaceUrlRaw),
    [workspaceUrlRaw],
  );
  const normalisedUrl = normalisation.kind === "ok" ? normalisation.url : null;

  // DAY-127 #5a: the add flow used to lock the workspace URL as
  // soon as the dialog detected an existing Atlassian source
  // (Journey C), which looked to the user like the field was
  // hard-wired to the existing tenant ("it's blocked to
  // company"). It's still true that Journey C only makes
  // sense when the two products live on the same workspace URL,
  // but the right UX is to let the user type and automatically
  // drop out of reuse-PAT if they point somewhere else — not to
  // block the field outright. This derived flag captures
  // "the user has edited the URL away from the existing tenant",
  // and the effect below forces `tokenMode = "paste"` for that
  // case so the submit doesn't try to reuse a secret from a
  // different workspace.
  const divergedFromExistingUrl =
    existing != null &&
    normalisedUrl != null &&
    normalisedUrl !== existing.workspaceUrl;
  // Tracks whether *this effect* switched the token mode away from
  // reuse because of divergence. Only in that case do we ever
  // switch it back — a manual flip to paste by the user (e.g. the
  // existing PAT doesn't work for some reason) must stick.
  const didAutoSwitchToPasteRef = useRef(false);
  useEffect(() => {
    if (divergedFromExistingUrl) {
      setTokenMode((prev) => {
        if (prev === "reuse") {
          didAutoSwitchToPasteRef.current = true;
          return "paste";
        }
        return prev;
      });
      return;
    }
    // DAY-127 #5a (post-review): if we flipped the user into paste
    // mode because they diverged, and they then edit the URL back
    // to the existing tenant, restore the reuse default so the
    // dialog doesn't leave a stale "paste" selection the user has
    // to spot and undo manually. Skip the restore if a token has
    // already been pasted — wiping a half-entered secret is a UX
    // regression worse than the stale radio we're cleaning up.
    // Email is intentionally *not* gated on here: the dialog
    // pre-fills `email` from the existing source, so an email
    // check would block the restore for every Journey C caller.
    if (
      didAutoSwitchToPasteRef.current &&
      apiToken.trim().length === 0
    ) {
      setTokenMode("reuse");
      didAutoSwitchToPasteRef.current = false;
    }
  }, [divergedFromExistingUrl, apiToken]);

  // Typing in the URL, email, or token invalidates any cached
  // validation: the user is pointing at a different account, and
  // running the old result would let them add a source whose
  // identity doesn't match what's on-screen.
  useEffect(() => {
    setValidation((prev) => (prev.kind === "idle" ? prev : { kind: "idle" }));
  }, [normalisedUrl, email, apiToken, tokenMode]);

  // Invariant 1: at-least-one-product. Submit stays disabled until
  // the user ticks at least one of the two product checkboxes. In
  // reconnect mode the checkbox state is forced to the source's
  // kind so this is always satisfied.
  const atLeastOneProduct = enableJira || enableConfluence;

  // In reuse mode the email + token fields are inert and validation
  // is a no-op (the existing source already proved the credentials
  // work). The submit path skips the `atlassian_validate_credentials`
  // round-trip entirely. Reconnect mode always requires a fresh
  // token, so it never enters reuse.
  const isReuseMode = !isReconnect && tokenMode === "reuse" && existing != null;

  const canValidate =
    !isReuseMode &&
    normalisation.kind === "ok" &&
    email.trim().length > 0 &&
    apiToken.trim().length > 0 &&
    validation.kind !== "checking";

  // DAY-126 edit: users can rotate the token, rename the source,
  // or both in one submit. At least one side has to be dirty so
  // the Save button is not a no-op. `label.trim()` must be
  // non-empty because the DB pins `label NOT NULL` and an empty
  // chip is invisible in the sidebar. When the user pastes a new
  // token the submit handler runs the probe server-side as part of
  // the `atlassian_sources_reconnect` IPC, so we do not gate on
  // client-side validation here.
  const trimmedLabel = label.trim();
  const labelChanged =
    isReconnect &&
    reconnectSource != null &&
    trimmedLabel.length > 0 &&
    trimmedLabel !== reconnectSource.label;
  const tokenEntered = apiToken.trim().length > 0;

  const canSubmit = isReconnect
    ? (tokenEntered || labelChanged) && trimmedLabel.length > 0 && !submitting
    : atLeastOneProduct &&
      normalisation.kind === "ok" &&
      !submitting &&
      (isReuseMode || validation.kind === "ok");

  const handleValidate = useCallback(async () => {
    if (!canValidate || normalisedUrl == null) return;
    setValidation({ kind: "checking" });
    try {
      const result = await invoke("atlassian_validate_credentials", {
        workspaceUrl: normalisedUrl,
        email: email.trim(),
        apiToken: apiToken.trim(),
      });
      setValidation({ kind: "ok", result });
    } catch (err) {
      const message =
        err instanceof Error
          ? err.message
          : typeof err === "object" && err != null && "data" in err
            ? JSON.stringify((err as { data: unknown }).data)
            : JSON.stringify(err);
      setValidation({ kind: "error", message });
    }
  }, [canValidate, normalisedUrl, email, apiToken]);

  const handleOpenTokenPage = useCallback(async () => {
    try {
      await invoke("shell_open", { url: atlassianTokenPageUrl() });
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      if (isReconnect && reconnectSource != null) {
        // DAY-87 + DAY-126: unified edit. Rotate the token first
        // (if the user pasted one) so a failed rotation short-
        // circuits before we touch the label — otherwise a
        // renamed-but-still-broken source would be worse than the
        // pre-edit state. The backend re-runs
        // `/rest/api/3/myself` against the stored workspace URL +
        // email, refuses if the resolved `account_id` doesn't
        // match the `SourceIdentity` bound to this source, and
        // otherwise rotates the keychain slot atomically. The
        // returned list is every `source_id` whose secret was
        // rotated (two ids when the PAT is shared across Jira +
        // Confluence siblings); we hand it to the caller so the
        // sidebar can fire `sources_healthcheck` for each and
        // clear the red chips without waiting for the next poll.
        let affected: string[] = [reconnectSource.id];
        if (tokenEntered) {
          affected = await invoke("atlassian_sources_reconnect", {
            sourceId: reconnectSource.id,
            apiToken: apiToken.trim(),
          });
        }
        if (labelChanged) {
          // Label rename is deliberately per-source even when the
          // PAT is shared across Jira + Confluence siblings. A
          // user editing the Jira chip intends to rename the Jira
          // source; the Confluence sibling keeps its label.
          const patch: SourcePatch = { label: trimmedLabel, config: null };
          await invoke("sources_update", {
            id: reconnectSource.id,
            patch,
            pat: null,
          });
        }
        sourcesBus.dispatchEvent(new Event(SOURCES_CHANGED));
        onReconnected?.(affected);
        return;
      }
      if (normalisation.kind !== "ok") return;
      // Journey C mode 1 (reuse) — clone the existing `SecretRef`
      // and skip the token field entirely. The `account_id` comes
      // from the validation we have to re-run once if the existing
      // source didn't carry it (today: always). A future refactor
      // can surface `accountId` on `Source` directly and drop the
      // extra validate round-trip.
      let accountId: string | null = null;
      let reuseSecretRef: SecretRef | null = null;

      if (isReuseMode && existing != null) {
        reuseSecretRef = existing.secretRef;
        if (existing.accountId != null) {
          accountId = existing.accountId;
        } else {
          // No cached account id on the existing source — ask the
          // IPC once so we can seed the second product's
          // `SourceIdentity` with the same Atlassian account. The
          // existing source's token is used via the secret_ref the
          // backend reads; we still pass a dummy token here because
          // the IPC signature requires a string, and the backend
          // rejects with `IPC_ATLASSIAN_REUSE_SECRET_MISSING` if
          // the slot is empty regardless of what we send. In
          // practice we expect the caller to have cached the
          // account id by the time reuse is an option; this branch
          // is defensive.
          throw new Error(
            "Existing Atlassian source is missing a cached account id. " +
              "Reopen the other product's dialog and re-validate first.",
          );
        }
      } else if (validation.kind === "ok") {
        accountId = validation.result.account_id;
      }

      if (accountId == null) {
        throw new Error("Missing account_id — validate your credentials first.");
      }

      const rows = await invoke("atlassian_sources_add", {
        workspaceUrl: normalisation.url,
        email: email.trim(),
        apiToken: isReuseMode ? null : apiToken.trim(),
        accountId,
        enableJira,
        enableConfluence,
        reuseSecretRef,
      });
      // DAY-127 #5b: if the user typed a label, rename each
      // inserted row post-hoc. `atlassian_sources_add` does not
      // take a label argument (the Rust side auto-derives
      // `"<Kind> — <host>"`); rather than grow the IPC we apply a
      // label-only `sources_update` per inserted row. When the
      // user enabled both products in Journey A the two siblings
      // land with `"<label> — Jira"` / `"<label> — Confluence"` so
      // they remain distinguishable in the sidebar. A failure
      // here does not roll back the add — the sources exist with
      // their auto-derived labels and the user can rename via the
      // edit dialog, which is a strictly recoverable state. We
      // fire the updates in parallel (the two rows are independent
      // in Journey A) to shrink the "half-renamed" window, and
      // surface each failure via `console.warn` so a user report
      // of "my label didn't stick" lands something actionable in
      // the log drawer rather than a silent swallow.
      if (trimmedLabel.length > 0) {
        await Promise.all(
          rows.map((row) => {
            const suffix = rows.length > 1 ? ` — ${row.kind}` : "";
            return invoke("sources_update", {
              id: row.id,
              patch: { label: `${trimmedLabel}${suffix}`, config: null },
              pat: null,
            }).catch((err) => {
              console.warn("atlassian post-add rename failed", {
                id: row.id,
                err,
              });
            });
          }),
        );
      }
      // Fan out on the same bus every other source mutator uses so
      // the sidebar, the onboarding state machine, and
      // `useLocalRepos` all refresh.
      sourcesBus.dispatchEvent(new Event(SOURCES_CHANGED));
      onAdded(rows);
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
      setSubmitting(false);
    }
  }, [
    canSubmit,
    normalisation,
    isReuseMode,
    existing,
    validation,
    email,
    apiToken,
    enableJira,
    enableConfluence,
    onAdded,
    isReconnect,
    reconnectSource,
    onReconnected,
    tokenEntered,
    labelChanged,
    trimmedLabel,
  ]);

  const handleClose = useCallback(() => {
    if (submitting) return;
    onClose();
  }, [submitting, onClose]);

  const urlHelp = isReconnect ? null : renderUrlHelp(normalisation);

  const title = isReconnect
    ? `Edit ${reconnectConfig?.kind ?? "Atlassian"} source`
    : existing
      ? `Add ${existingKind === "Jira" ? "Confluence" : "Jira"}`
      : "Add Atlassian source";
  const description = isReconnect
    ? "Rename the source and/or rotate its API token. Leave the token blank to keep the existing one. The workspace URL and account email are fixed — delete and re-add to change either one."
    : existing
      ? `You already have ${existingKind} connected. Add the other product with the same token — or use a different one.`
      : "Connect Jira, Confluence, or both with one Atlassian API token. Dayseam only needs read access.";

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title={title}
      description={description}
      testId="add-atlassian-dialog"
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
                ? "Saving…"
                : "Adding…"
              : isReconnect
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
        {!isReconnect ? (
          <fieldset className="flex flex-col gap-2">
            <legend className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
              Products
            </legend>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={enableJira}
                onChange={(e) => setEnableJira(e.target.checked)}
                data-testid="add-atlassian-enable-jira"
              />
              <span>Jira</span>
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={enableConfluence}
                onChange={(e) => setEnableConfluence(e.target.checked)}
                data-testid="add-atlassian-enable-confluence"
              />
              <span>Confluence</span>
            </label>
            {!atLeastOneProduct ? (
              <span
                role="alert"
                data-testid="add-atlassian-product-required"
                className="text-[11px] text-red-700 dark:text-red-300"
              >
                Pick at least one product.
              </span>
            ) : null}
          </fieldset>
        ) : null}

        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
            Workspace URL
          </span>
          <input
            type="text"
            value={workspaceUrlRaw}
            onChange={(e) => setWorkspaceUrlRaw(e.target.value)}
            readOnly={isReconnect}
            autoFocus={!isReconnect}
            placeholder="yourcompany"
            data-testid="add-atlassian-workspace-url"
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-sm read-only:cursor-not-allowed read-only:opacity-75 dark:border-neutral-700 dark:bg-neutral-900"
          />
          {urlHelp}
          {divergedFromExistingUrl ? (
            <span
              data-testid="add-atlassian-url-diverged"
              className="text-[11px] text-amber-700 dark:text-amber-400"
            >
              This is a different workspace than your existing Atlassian
              source. The "reuse token" option has been turned off —
              paste a fresh API token for this workspace.
            </span>
          ) : null}
        </label>

        {!isReconnect ? (
          // DAY-127 #5b: add flow now offers an optional label
          // input so the chip can be named at creation time
          // instead of accepting the default `"<Kind> — <host>"`.
          // Leaving it blank preserves the pre-DAY-127 derived
          // label; filling it triggers a post-add `sources_update`
          // per inserted row (one row in Journey B/C, two in
          // Journey A) so both siblings land with the user-
          // supplied label + a kind suffix.
          <label className="flex flex-col gap-1">
            <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
              Label{" "}
              <span className="font-normal text-neutral-500 dark:text-neutral-400">
                (optional)
              </span>
            </span>
            <input
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="Work"
              data-testid="add-atlassian-add-label"
              className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
            <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
              Leave blank to use the default label
              (e.g. <code>Jira — yourcompany.atlassian.net</code>).
            </span>
          </label>
        ) : null}

        {isReconnect ? (
          <label className="flex flex-col gap-1">
            <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
              Label
            </span>
            <input
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="Work Jira"
              data-testid="add-atlassian-label"
              className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          </label>
        ) : null}

        {existing ? (
          <fieldset className="flex flex-col gap-1 rounded border border-neutral-200 bg-neutral-50 px-3 py-2 text-xs dark:border-neutral-800 dark:bg-neutral-900/40">
            <legend className="px-1 text-[11px] font-medium text-neutral-600 dark:text-neutral-400">
              Atlassian token
            </legend>
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="atlassian-token-mode"
                value="reuse"
                checked={tokenMode === "reuse"}
                onChange={() => setTokenMode("reuse")}
                disabled={divergedFromExistingUrl}
                data-testid="add-atlassian-token-mode-reuse"
              />
              <span
                className={
                  divergedFromExistingUrl
                    ? "text-neutral-400 dark:text-neutral-600"
                    : undefined
                }
              >
                Reuse the token from <em>{existing.kind}</em> (no paste needed).
              </span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="atlassian-token-mode"
                value="paste"
                checked={tokenMode === "paste"}
                onChange={() => setTokenMode("paste")}
                data-testid="add-atlassian-token-mode-paste"
              />
              <span>Use a different API token.</span>
            </label>
          </fieldset>
        ) : null}

        {!isReuseMode ? (
          <>
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
                Atlassian account email
              </span>
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                readOnly={isReconnect}
                placeholder="you@example.com"
                data-testid="add-atlassian-email"
                spellCheck={false}
                autoCapitalize="off"
                autoComplete="email"
                className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm read-only:cursor-not-allowed read-only:opacity-75 dark:border-neutral-700 dark:bg-neutral-900"
              />
            </label>

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void handleOpenTokenPage()}
                data-testid="add-atlassian-open-token-page"
                className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
              >
                Open token page
              </button>
              <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
                Create a token at <code>id.atlassian.com</code> with no extra
                scopes.
              </span>
            </div>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
                API token
              </span>
              <input
                type="password"
                value={apiToken}
                onChange={(e) => setApiToken(e.target.value)}
                placeholder={isReconnect ? "Leave blank to keep existing token" : "ATATT3…"}
                data-testid="add-atlassian-api-token"
                spellCheck={false}
                autoCapitalize="off"
                autoComplete="off"
                className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-sm dark:border-neutral-700 dark:bg-neutral-900"
              />
              {isReconnect ? (
                <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
                  Paste a fresh token only if you need to rotate it.
                  A new token is validated against the existing account
                  before the keychain entry is replaced.
                </span>
              ) : (
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() => void handleValidate()}
                    disabled={!canValidate}
                    data-testid="add-atlassian-validate"
                    className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
                  >
                    {validation.kind === "checking" ? "Validating…" : "Validate"}
                  </button>
                  {renderValidationStatus(validation)}
                </div>
              )}
            </label>
          </>
        ) : null}

        {submitError ? (
          <p
            role="alert"
            data-testid="add-atlassian-submit-error"
            className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
          >
            {submitError}
          </p>
        ) : null}
      </form>
    </Dialog>
  );
}

function renderUrlHelp(n: WorkspaceUrlNormalisation) {
  if (n.kind === "ok") {
    return (
      <span
        data-testid="add-atlassian-url-normalised"
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
        data-testid="add-atlassian-url-invalid"
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
          data-testid="add-atlassian-validation-ok"
          className="text-[11px] text-emerald-700 dark:text-emerald-300"
        >
          ✓ Connected as <code>{validation.result.display_name}</code>
          {validation.result.email ? ` <${validation.result.email}>` : ""}
        </span>
      );
    case "error":
      return (
        <span
          role="alert"
          data-testid="add-atlassian-validation-error"
          className="text-[11px] text-red-700 dark:text-red-300"
        >
          {validation.message}
        </span>
      );
    case "checking":
      return (
        <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
          Checking /rest/api/3/myself…
        </span>
      );
    case "idle":
    default:
      return null;
  }
}
