// Live preview of a report generation — renders a progress bar while
// the run is in-flight and the finished draft's sections/bullets once
// it completes. The preview is purely a view over `useReport()`
// state; no IPC lives here.
//
// Progress rendering rule: when the last `ProgressEvent.phase.status`
// is `in_progress` and `total` is a known number, render a
// determinate bar; otherwise render an indeterminate pulse. This
// matches the semantics `ProgressPhase` documents and keeps the UI
// honest about "we don't know how long this will take yet".

import { useMemo, useState } from "react";
import type {
  ProgressEvent,
  ReportDraft,
  RenderedBullet,
  RenderedSection,
} from "@dayseam/ipc-types";
import type { ReportStatus } from "../../ipc";
import { BulletEvidencePopover } from "./BulletEvidencePopover";

export interface StreamingPreviewProps {
  status: ReportStatus;
  progress: ProgressEvent[];
  draft: ReportDraft | null;
  error: string | null;
}

function lastMessage(events: ProgressEvent[]): string | null {
  const last = events[events.length - 1];
  if (!last) return null;
  const phase = last.phase;
  if ("message" in phase) return phase.message;
  return null;
}

/** Extract `(completed, total)` from the last `in_progress` event, if
 *  the run is currently in `in_progress`. Anything else (Starting,
 *  Completed, Cancelled, Failed) renders as indeterminate / steady. */
function determinateProgress(
  events: ProgressEvent[],
): { completed: number; total: number } | null {
  const last = events[events.length - 1];
  if (!last) return null;
  if (last.phase.status !== "in_progress") return null;
  const total = last.phase.total;
  if (total == null) return null;
  return { completed: last.phase.completed, total };
}

export function StreamingPreview({
  status,
  progress,
  draft,
  error,
}: StreamingPreviewProps) {
  const progressMessage = useMemo(() => lastMessage(progress), [progress]);
  const determinate = useMemo(() => determinateProgress(progress), [progress]);

  // Which bullet (if any) has the evidence popover open. The id is
  // globally unique within a `ReportDraft` so a single piece of state
  // is enough.
  const [activeBulletId, setActiveBulletId] = useState<string | null>(null);

  const isIdle = status === "idle";
  const isRunning = status === "starting" || status === "running";
  const isFailed = status === "failed";
  const isCancelled = status === "cancelled";
  const isCompleted = status === "completed" && draft !== null;

  if (isIdle && !draft) {
    return (
      <section
        aria-label="Report preview"
        className="flex flex-1 flex-col items-center justify-center gap-2 px-6 py-10 text-center"
      >
        <div
          aria-hidden="true"
          className="h-10 w-10 rounded-full border-2 border-dashed border-neutral-300 dark:border-neutral-700"
        />
        <h2 className="text-base font-medium text-neutral-700 dark:text-neutral-200">
          No report yet
        </h2>
        <p className="max-w-sm text-sm text-neutral-500 dark:text-neutral-400">
          Pick a date, choose your sources, and hit Generate. The draft and its
          evidence will stream in here as it's produced.
        </p>
      </section>
    );
  }

  return (
    <section
      aria-label="Report preview"
      className="flex flex-1 flex-col gap-3 overflow-y-auto px-6 py-4"
      data-testid="streaming-preview"
    >
      {isRunning ? (
        <ProgressBar
          determinate={determinate}
          message={progressMessage}
          data-testid="streaming-preview-progress"
        />
      ) : null}

      {isFailed ? (
        <div
          role="alert"
          className="rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-800 dark:bg-red-950 dark:text-red-200"
        >
          Generation failed: {error ?? "unknown error"}
        </div>
      ) : null}

      {isCancelled ? (
        <div
          role="status"
          className="rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200"
        >
          Run cancelled.
        </div>
      ) : null}

      {draft ? (
        <article
          className="flex flex-col gap-4"
          data-testid="streaming-preview-draft"
        >
          <header className="flex items-center justify-between">
            <h2 className="text-base font-semibold text-neutral-900 dark:text-neutral-50">
              {draft.date}
            </h2>
            <span className="text-xs text-neutral-500 dark:text-neutral-400">
              {draft.template_id} · {draft.template_version}
            </span>
          </header>
          {draft.sections.length === 0 ? (
            <p className="text-sm text-neutral-500 dark:text-neutral-400">
              No activity found for this date. Try widening the date range or
              rescanning your sources.
            </p>
          ) : (
            draft.sections.map((section) => (
              <SectionView
                key={section.id}
                section={section}
                draft={draft}
                activeBulletId={activeBulletId}
                onOpenBullet={setActiveBulletId}
                onCloseBullet={() => setActiveBulletId(null)}
              />
            ))
          )}
        </article>
      ) : isCompleted ? (
        <p className="text-sm text-neutral-500 dark:text-neutral-400">
          Report ready, loading draft…
        </p>
      ) : null}
    </section>
  );
}

function ProgressBar({
  determinate,
  message,
}: {
  determinate: { completed: number; total: number } | null;
  message: string | null;
}) {
  // Clamp ratio to [0, 1] so a misbehaving producer (total lower than
  // completed) still renders a sensible bar.
  const ratio = determinate
    ? Math.min(1, Math.max(0, determinate.completed / determinate.total))
    : null;

  return (
    <div
      className="flex flex-col gap-1"
      role="status"
      aria-live="polite"
      aria-label="Report generation progress"
      data-testid="streaming-preview-progress"
    >
      <div
        className="h-1.5 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-800"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={determinate?.total ?? undefined}
        aria-valuenow={determinate?.completed ?? undefined}
      >
        {ratio !== null ? (
          <div
            className="h-full bg-neutral-900 transition-[width] duration-150 ease-out dark:bg-neutral-100"
            style={{ width: `${Math.round(ratio * 100)}%` }}
            data-testid="streaming-preview-progress-fill"
          />
        ) : (
          <div
            aria-hidden="true"
            className="h-full w-1/3 animate-pulse bg-neutral-900 dark:bg-neutral-100"
          />
        )}
      </div>
      {message ? (
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          {message}
        </p>
      ) : null}
    </div>
  );
}

function SectionView({
  section,
  draft,
  activeBulletId,
  onOpenBullet,
  onCloseBullet,
}: {
  section: RenderedSection;
  draft: ReportDraft;
  activeBulletId: string | null;
  onOpenBullet: (id: string) => void;
  onCloseBullet: () => void;
}) {
  return (
    <section className="flex flex-col gap-1">
      <h3 className="text-sm font-semibold uppercase tracking-wide text-neutral-600 dark:text-neutral-300">
        {section.title}
      </h3>
      {section.bullets.length === 0 ? (
        <p className="text-sm text-neutral-500 dark:text-neutral-400">
          Nothing here.
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {section.bullets.map((bullet) => (
            <BulletRow
              key={bullet.id}
              bullet={bullet}
              draft={draft}
              isOpen={activeBulletId === bullet.id}
              onOpen={() => onOpenBullet(bullet.id)}
              onClose={onCloseBullet}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

function BulletRow({
  bullet,
  draft,
  isOpen,
  onOpen,
  onClose,
}: {
  bullet: RenderedBullet;
  draft: ReportDraft;
  isOpen: boolean;
  onOpen: () => void;
  onClose: () => void;
}) {
  const evidence = draft.evidence.find((e) => e.bullet_id === bullet.id);
  const hasEvidence = !!evidence && evidence.event_ids.length > 0;

  return (
    <li className="relative flex items-start gap-2">
      <span
        aria-hidden="true"
        className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-neutral-400 dark:bg-neutral-500"
      />
      <button
        type="button"
        onClick={hasEvidence ? onOpen : undefined}
        disabled={!hasEvidence}
        className={`text-left text-sm text-neutral-800 dark:text-neutral-200 ${
          hasEvidence
            ? "cursor-pointer hover:underline"
            : "cursor-default opacity-80"
        }`}
        title={hasEvidence ? evidence?.reason ?? "Show evidence" : undefined}
        data-testid={`bullet-${bullet.id}`}
      >
        {bullet.text}
      </button>
      {isOpen && evidence ? (
        <BulletEvidencePopover
          bullet={bullet}
          eventIds={evidence.event_ids}
          reason={evidence.reason}
          onClose={onClose}
        />
      ) : null}
    </li>
  );
}
