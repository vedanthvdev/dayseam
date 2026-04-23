// Uniform "rename this source" dialog, reached from every source
// chip's action cluster.
//
// Prior to DAY-121 there were four different edit surfaces — one per
// connector kind — and only two of them (LocalGit, GitLab) exposed
// the label at all. GitHub and Atlassian "edit" opened in reconnect
// mode with the label pinned, and GitLab's dialog technically
// rendered a label input but its `handleSubmit` always forwarded
// `pat.trim()` (an empty string for a label-only edit) which the
// Rust-side `validate_pat_arg` rejects with `ipc.gitlab.pat.missing`.
// Net effect: users could not rename a GitHub, Atlassian, or GitLab
// source from the UI.
//
// Rather than grow four separate label inputs with four separate
// save paths, this dialog calls the existing `sources_update` IPC
// with a label-only patch (`config: null`, `pat: null`) which the
// backend has supported since DAY-70. One dialog, one code path, one
// test surface, works identically for every connector kind.
//
// The dialog deliberately does not let the user clear the label —
// the DB schema pins `label` to `NOT NULL` (`0001_initial.sql:9`) and
// an empty chip would be invisible in the sidebar. Trimmed-empty
// input disables the Save button instead of erroring on submit.

import { useCallback, useEffect, useState } from "react";
import type { Source } from "@dayseam/ipc-types";
import { Dialog, DialogButton } from "../../components/Dialog";
import { useSources } from "../../ipc";

interface RenameSourceDialogProps {
  /** Non-null while the dialog should be open. The source is captured
   *  on open so the label reset in `useEffect` picks up the latest
   *  value if the caller swaps sources without remounting. */
  source: Source | null;
  onClose: () => void;
  /** Fired after `sources_update` resolves successfully. The caller
   *  is responsible for closing the dialog (`setSource(null)`) and
   *  refreshing any local state that needs the new label. */
  onRenamed: (updated: Source) => void;
}

export function RenameSourceDialog({
  source,
  onClose,
  onRenamed,
}: RenameSourceDialogProps) {
  const { update } = useSources();
  const [label, setLabel] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  useEffect(() => {
    if (!source) return;
    setLabel(source.label);
    setSubmitError(null);
    setSubmitting(false);
  }, [source]);

  const trimmed = label.trim();
  const unchanged = source !== null && trimmed === source.label.trim();
  // Save is disabled when the input is whitespace-only OR equals the
  // current label. The latter is a small UX nicety: the Save button
  // only "lights up" when the user has actually changed the label,
  // so clicking it always produces a visible chip update.
  const canSubmit =
    source !== null && trimmed.length > 0 && !unchanged && !submitting;

  const handleSubmit = useCallback(async () => {
    if (!source || !canSubmit) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      // A label-only patch: `config: null` leaves the existing
      // per-kind config untouched, and `pat: null` tells
      // `validate_pat_arg` to leave the stored secret alone. The
      // Rust command re-reads the row and returns the updated
      // `Source`, which we forward so the caller can update its
      // own cached view without a full list refresh.
      const updated = await update(source.id, { label: trimmed, config: null }, null);
      onRenamed(updated);
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : JSON.stringify(err));
      setSubmitting(false);
    }
  }, [source, canSubmit, trimmed, update, onRenamed]);

  const handleClose = useCallback(() => {
    if (submitting) return;
    onClose();
  }, [submitting, onClose]);

  return (
    <Dialog
      open={source !== null}
      onClose={handleClose}
      title="Rename source"
      description={
        source
          ? `Change the label shown in the Sources strip. The underlying ${connectorName(source)} connection is left untouched.`
          : undefined
      }
      testId="rename-source-dialog"
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
            {submitting ? "Saving…" : "Save"}
          </DialogButton>
        </>
      }
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void handleSubmit();
        }}
      >
        <label className="flex flex-col gap-1">
          <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
            Label
          </span>
          <input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="Work repos"
            data-testid="rename-source-label"
            autoFocus
            spellCheck={false}
            autoComplete="off"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </label>
        {submitError ? (
          <p
            role="alert"
            data-testid="rename-source-error"
            className="rounded border border-red-300 bg-red-50 px-2 py-1 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
          >
            {submitError}
          </p>
        ) : null}
      </form>
    </Dialog>
  );
}

/** Human-readable connector name for the dialog description. Kept
 *  local because nothing else in the codebase needs a purely
 *  cosmetic label for a `Source` variant. */
function connectorName(source: Source): string {
  if ("LocalGit" in source.config) return "local-git";
  if ("GitLab" in source.config) return "GitLab";
  if ("GitHub" in source.config) return "GitHub";
  if ("Jira" in source.config) return "Jira";
  if ("Confluence" in source.config) return "Confluence";
  return "source";
}
