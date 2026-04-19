// Save dialog — lets the user pick which configured sink the current
// `ReportDraft` should be written to and surfaces the
// `WriteReceipt`s that came back. Interactive-only; scheduled runs
// (Task 8) live on a different code path.
//
// The sink list is produced by `filterSinksForSave({ unattended:
// false })` so the capability check documented in
// `sink-capabilities.ts` actually runs. The Phase-2 markdown sink
// has `interactive_only = false`, so for the moment the filter
// doesn't drop anything — that's intentional, the invariant is what
// matters.

import { useMemo, useState } from "react";
import type { Sink, WriteReceipt } from "@dayseam/ipc-types";
import { useSinks, invoke } from "../../ipc";
import { Dialog, DialogButton } from "../../components/Dialog";
import { filterSinksForSave } from "./sink-capabilities";

export interface SaveReportDialogProps {
  open: boolean;
  onClose: () => void;
  /** `true` once a draft exists to save. When `false` the dialog
   *  still renders (so the caller can keep it mounted) but the save
   *  button is disabled with an explanatory hint. */
  hasDraft: boolean;
  /** Fired with the sink id the user picked. The caller wires this
   *  through `useReport().save`; the dialog itself does not hold the
   *  draft id. */
  onSave: (sinkId: string) => Promise<WriteReceipt[]>;
}

type DialogStatus = "idle" | "saving" | "saved" | "error";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function SaveReportDialog({
  open,
  onClose,
  hasDraft,
  onSave,
}: SaveReportDialogProps) {
  const { sinks, loading: sinksLoading, error: sinksError } = useSinks();
  const eligibleSinks = useMemo(
    () => filterSinksForSave(sinks, { unattended: false }),
    [sinks],
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [status, setStatus] = useState<DialogStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [receipts, setReceipts] = useState<WriteReceipt[]>([]);

  const reset = () => {
    setSelectedId(null);
    setStatus("idle");
    setError(null);
    setReceipts([]);
  };

  const handleClose = () => {
    reset();
    onClose();
  };

  const handleSave = async () => {
    if (!selectedId || !hasDraft) return;
    setStatus("saving");
    setError(null);
    try {
      const out = await onSave(selectedId);
      setReceipts(out);
      setStatus("saved");
    } catch (err) {
      setStatus("error");
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  };

  const openReceiptPath = async (path: string) => {
    try {
      await invoke("shell_open", { url: `file://${encodeURI(path)}` });
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  };

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title="Save report"
      description={
        hasDraft
          ? "Choose a sink to write the current draft to."
          : "Generate a report first, then come back here to save it."
      }
      size="md"
      testId="save-report-dialog"
      footer={
        <>
          <DialogButton kind="secondary" onClick={handleClose}>
            {status === "saved" ? "Done" : "Cancel"}
          </DialogButton>
          {status !== "saved" ? (
            <DialogButton
              kind="primary"
              onClick={handleSave}
              disabled={
                !hasDraft || !selectedId || status === "saving" || sinksLoading
              }
            >
              {status === "saving" ? "Saving…" : "Save"}
            </DialogButton>
          ) : null}
        </>
      }
    >
      {sinksError ? (
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          Failed to load sinks: {sinksError}
        </p>
      ) : null}

      {sinksLoading && sinks.length === 0 ? (
        <p className="text-sm text-neutral-500 dark:text-neutral-400">
          Loading sinks…
        </p>
      ) : null}

      {!sinksLoading && eligibleSinks.length === 0 ? (
        <p className="text-sm text-neutral-500 dark:text-neutral-400">
          No sinks configured yet. Use the Sinks dialog in the status bar to
          add one.
        </p>
      ) : null}

      {status !== "saved" ? (
        <ul className="flex flex-col gap-1.5">
          {eligibleSinks.map((sink) => (
            <li key={sink.id}>
              <label className="flex cursor-pointer items-start gap-2 rounded border border-neutral-200 px-2 py-1.5 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-900">
                <input
                  type="radio"
                  name="save-report-sink"
                  value={sink.id}
                  checked={selectedId === sink.id}
                  onChange={() => setSelectedId(sink.id)}
                  data-testid={`save-sink-${sink.id}`}
                  className="mt-0.5"
                />
                <div className="flex flex-col">
                  <span className="text-sm font-medium text-neutral-800 dark:text-neutral-100">
                    {sink.label}
                  </span>
                  <span className="text-xs text-neutral-500 dark:text-neutral-400">
                    {describeSink(sink)}
                  </span>
                </div>
              </label>
            </li>
          ))}
        </ul>
      ) : null}

      {error ? (
        <p
          role="alert"
          className="mt-2 text-sm text-red-600 dark:text-red-400"
          data-testid="save-report-error"
        >
          {error}
        </p>
      ) : null}

      {status === "saved" && receipts.length > 0 ? (
        <ul
          className="mt-2 flex flex-col gap-1.5"
          data-testid="save-report-receipts"
        >
          {receipts.map((receipt, idx) => (
            <li
              key={`${receipt.written_at}-${idx}`}
              className="rounded border border-emerald-300 bg-emerald-50 px-2 py-1.5 text-xs text-emerald-900 dark:border-emerald-800 dark:bg-emerald-950 dark:text-emerald-100"
            >
              <p className="font-medium">
                Wrote {formatBytes(receipt.bytes_written)}
              </p>
              {receipt.destinations_written.map((path) => (
                <button
                  key={path}
                  type="button"
                  onClick={() => void openReceiptPath(path)}
                  className="mt-0.5 block w-full truncate text-left text-[11px] font-mono underline-offset-2 hover:underline"
                  title="Open in default app"
                >
                  {path}
                </button>
              ))}
            </li>
          ))}
        </ul>
      ) : null}
    </Dialog>
  );
}

function describeSink(sink: Sink): string {
  if ("MarkdownFile" in sink.config) {
    const cfg = sink.config.MarkdownFile;
    const dirs = cfg.dest_dirs.join(", ");
    return `markdown · ${dirs}${cfg.frontmatter ? " · frontmatter" : ""}`;
  }
  return sink.kind;
}
