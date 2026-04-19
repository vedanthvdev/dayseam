// Dialog: capture a label + one or more scan roots for a new
// `LocalGit` source and call `sources_add`, OR edit the label /
// scan-roots of an existing source via `sources_update`.
//
// The same dialog drives both create and edit because the form
// shape is identical — only the commit action and the title vary.
// In create mode, on success the parent (SourcesSidebar) takes the
// returned `Source` and opens `ApproveReposDialog` so the user can
// toggle `is_private` on the freshly-discovered repos before they're
// ever scanned. In edit mode, the approve dialog doesn't open —
// repos discovered via new scan roots surface through the normal
// rescan flow (↻ button on the chip) to keep this dialog's remit
// small.
//
// The user can either type/paste absolute paths (one per line) or
// use the "Browse…" button to pick a folder through the OS's native
// directory chooser (`@tauri-apps/plugin-dialog`). The picked path
// is appended to the textarea, so power users retain full edit
// control and the parser stays the single source of truth.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openFolderPicker } from "@tauri-apps/plugin-dialog";
import type { Source } from "@dayseam/ipc-types";
import { useSources } from "../../ipc";
import { Dialog, DialogButton } from "../../components/Dialog";

interface AddLocalGitSourceDialogProps {
  open: boolean;
  onClose: () => void;
  onAdded: (source: Source) => void;
  /**
   * When present, the dialog opens in edit mode: the label and scan
   * roots prefill from this source, the submit button calls
   * `sources_update` instead of `sources_add`, and the parent is
   * notified via `onSaved` rather than `onAdded` so it can skip the
   * approve-repos dialog that only makes sense right after creation.
   */
  editing?: Source | null;
  onSaved?: (source: Source) => void;
}

function parseScanRoots(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function initialRootsForEdit(source: Source | null | undefined): string {
  if (!source) return "";
  if ("LocalGit" in source.config) {
    return source.config.LocalGit.scan_roots.join("\n");
  }
  return "";
}

export function AddLocalGitSourceDialog({
  open,
  onClose,
  onAdded,
  editing,
  onSaved,
}: AddLocalGitSourceDialogProps) {
  const { add, update } = useSources();
  const isEdit = editing != null;
  const [label, setLabel] = useState(editing?.label ?? "");
  const [rootsRaw, setRootsRaw] = useState(() => initialRootsForEdit(editing));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const labelRef = useRef<HTMLInputElement>(null);

  // Re-seed form state whenever the caller swaps the `editing` source
  // (e.g. closes the dialog, picks a different chip, reopens). Without
  // this the dialog would keep the previous source's fields.
  useEffect(() => {
    if (!open) return;
    setLabel(editing?.label ?? "");
    setRootsRaw(initialRootsForEdit(editing));
    setError(null);
  }, [open, editing]);

  const scanRoots = useMemo(() => parseScanRoots(rootsRaw), [rootsRaw]);
  const canSubmit = label.trim().length > 0 && scanRoots.length > 0 && !submitting;

  const reset = useCallback(() => {
    setLabel("");
    setRootsRaw("");
    setError(null);
    setSubmitting(false);
  }, []);

  const handleClose = useCallback(() => {
    if (submitting) return;
    reset();
    onClose();
  }, [submitting, reset, onClose]);

  const handleBrowse = useCallback(async () => {
    // `open({ directory: true })` returns the absolute path string,
    // or `null` if the user cancelled. We don't surface cancellation
    // as an error; we just leave the textarea as-is. Picker errors
    // (sandbox denial, missing permission grant) land in `error` so
    // the user sees why Browse… did nothing.
    try {
      const picked = await openFolderPicker({
        directory: true,
        multiple: false,
        title: "Select a folder to scan for git repos",
      });
      if (typeof picked !== "string" || picked.length === 0) return;
      setRootsRaw((prev) => {
        const existing = parseScanRoots(prev);
        if (existing.includes(picked)) return prev;
        if (prev.length === 0) return picked;
        return prev.endsWith("\n") ? `${prev}${picked}` : `${prev}\n${picked}`;
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    try {
      if (isEdit && editing) {
        const saved = await update(editing.id, {
          label: label.trim(),
          config: { LocalGit: { scan_roots: scanRoots } },
        });
        reset();
        onSaved?.(saved);
      } else {
        const source = await add("LocalGit", label.trim(), {
          LocalGit: { scan_roots: scanRoots },
        });
        reset();
        onAdded(source);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
      setSubmitting(false);
    }
  }, [add, update, isEdit, editing, label, scanRoots, canSubmit, reset, onAdded, onSaved]);

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title={isEdit ? "Edit local git source" : "Add local git source"}
      description={
        isEdit
          ? "Update the label or scan roots for this source. Existing approved repos stay approved; new scan roots surface new repos the next time you rescan."
          : "Dayseam scans each root for `.git` directories and creates one repo row per discovery. Everything stays local."
      }
      testId="add-local-git-dialog"
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
                : "Scanning…"
              : isEdit
                ? "Save"
                : "Add and scan"}
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
            Label
          </span>
          <input
            ref={labelRef}
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            autoFocus
            placeholder="Work repos"
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </label>

        <div className="flex flex-col gap-1">
          <div className="flex items-center justify-between">
            <label
              htmlFor="add-local-git-scan-roots"
              className="text-xs font-medium text-neutral-700 dark:text-neutral-300"
            >
              Scan roots (one folder per line)
            </label>
            <button
              type="button"
              onClick={() => void handleBrowse()}
              disabled={submitting}
              data-testid="add-local-git-browse"
              className="rounded border border-neutral-300 bg-white px-2 py-0.5 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
            >
              Browse…
            </button>
          </div>
          <textarea
            id="add-local-git-scan-roots"
            rows={4}
            value={rootsRaw}
            onChange={(e) => setRootsRaw(e.target.value)}
            placeholder={"/Users/me/code\n/Users/me/work"}
            className="rounded border border-neutral-300 bg-white px-2 py-1.5 font-mono text-xs dark:border-neutral-700 dark:bg-neutral-900"
          />
          <span className="text-[11px] text-neutral-500 dark:text-neutral-400">
            {scanRoots.length} root{scanRoots.length === 1 ? "" : "s"} · each
            root is walked recursively for `.git` directories. Use
            Browse… to pick a folder, or paste absolute paths directly.
          </span>
        </div>

        {error ? (
          <p
            role="alert"
            className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
          >
            {error}
          </p>
        ) : null}
      </form>
    </Dialog>
  );
}
