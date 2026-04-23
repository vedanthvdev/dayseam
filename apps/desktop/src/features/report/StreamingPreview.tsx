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
  SourceKind,
} from "@dayseam/ipc-types";
import type { ReportStatus } from "../../ipc";
import { BulletEvidencePopover } from "./BulletEvidencePopover";

// DAY-104. Keep this ordering (and the emoji / label pair) in
// lockstep with `dayseam_core::SourceKind::render_order` +
// `display_emoji` / `display_label` — the Rust markdown sink and
// this preview MUST emit identical per-kind groupings so users
// see the same `### 🐙 GitHub` etc. in both surfaces. Changes
// here without a Rust-side mirror are a rendering divergence and
// will break the shared-ordering assertion in
// `sink-markdown-file` tests.
const SOURCE_KIND_ORDER: SourceKind[] = [
  "LocalGit",
  "GitHub",
  "GitLab",
  "Jira",
  "Confluence",
];

const SOURCE_KIND_LABEL: Record<SourceKind, string> = {
  LocalGit: "Local git",
  GitHub: "GitHub",
  GitLab: "GitLab",
  Jira: "Jira",
  Confluence: "Confluence",
};

const SOURCE_KIND_EMOJI: Record<SourceKind, string> = {
  LocalGit: "💻",
  GitHub: "🐙",
  GitLab: "🦊",
  Jira: "📋",
  Confluence: "📄",
};

/** Group a section's bullets by `source_kind`, preserving the
 *  bullet order inside each group and ordering the groups by
 *  `SOURCE_KIND_ORDER`. Bullets with `source_kind == null`
 *  (legacy drafts from <v0.5, see `RenderedBullet` doc) land in
 *  a trailing `null` group so they still render but without a
 *  subheading. */
function groupBulletsByKind(
  bullets: RenderedBullet[],
): { kind: SourceKind | null; bullets: RenderedBullet[] }[] {
  const groups = new Map<SourceKind | "__none__", RenderedBullet[]>();
  for (const bullet of bullets) {
    const key: SourceKind | "__none__" = bullet.source_kind ?? "__none__";
    const list = groups.get(key);
    if (list) {
      list.push(bullet);
    } else {
      groups.set(key, [bullet]);
    }
  }

  const ordered: { kind: SourceKind | null; bullets: RenderedBullet[] }[] = [];
  for (const kind of SOURCE_KIND_ORDER) {
    const list = groups.get(kind);
    if (list && list.length > 0) {
      ordered.push({ kind, bullets: list });
    }
  }
  const noneList = groups.get("__none__");
  if (noneList && noneList.length > 0) {
    ordered.push({ kind: null, bullets: noneList });
  }
  return ordered;
}

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
        // DOGFOOD-v0.4-06: `min-h-0` + `overflow-y-auto` lets this
        // flex child shrink below its intrinsic content height so
        // the parent's `h-dvh` bound is honored. Without it, `flex-1`
        // still grows without limit and the window scrollbar migrates
        // to `<body>`, pushing the footer off-screen. We keep the
        // same invariants on the idle branch as on the drafted branch
        // so the App-level layout regression test covers both.
        className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 overflow-y-auto px-6 py-10 text-center"
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
      // DOGFOOD-v0.4-06: `min-h-0` is the pairing with `flex-1` that
      // keeps `overflow-y-auto` on *this* element (not on the body)
      // — so only the preview scrolls, and the shell's footer stays
      // pinned at the bottom of the viewport for long reports.
      className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-6 py-4"
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
            {/*
              DAY-68 Phase 3 Task 8: the visible label used to read
              `${template_id} · template v${template_version}`, but
              `template_version` is a YYYY-MM-DD string (see
              `dayseam-report::DEV_EOD_TEMPLATE_VERSION`). Side-by-side
              with `{draft.date}` in the same header, users kept
              reading the version as "the report date is wrong". We
              now render only `template_id` and tuck the revision into
              the tooltip, where the word "schema" makes it clear the
              date-shaped string is a format identifier, not content.
            */}
            <span
              className="text-xs text-neutral-500 dark:text-neutral-400"
              title={`Template "${draft.template_id}" · schema revision ${draft.template_version} (bumped when the rendered output would change for the same input; unrelated to the report date)`}
              data-template-version={draft.template_version}
            >
              {draft.template_id}
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
  // DAY-90 TST-v0.2-02. `data-section` and `data-bullet-count`
  // are stable DOM hooks the E2E suite uses to make count-aware
  // assertions — `cy.contains("COMMITS")` passes on a heading
  // that merely exists, not on one that reflects the expected
  // event count, which is the class of silent failure
  // DOG-v0.2-04 caught. Values live on the section's outer
  // element so a Playwright locator can scope a `data-bullet`
  // count query to exactly one section without walking the tree.
  //
  // DAY-104. Inside each section, bullets are grouped by
  // `source_kind` so the preview matches the markdown sink
  // byte-for-byte in structure (`### <emoji> <Label>` per group).
  // `data-kind="<kind>"` (or `"none"` for legacy un-attributed
  // bullets) gives the RTL + E2E tests a stable anchor to scope
  // per-group queries, mirroring how `data-section` scopes
  // per-section ones.
  const groups = useMemo(() => groupBulletsByKind(section.bullets), [
    section.bullets,
  ]);

  return (
    <section
      className="flex flex-col gap-1"
      data-section={section.id}
      data-bullet-count={section.bullets.length}
    >
      <h3 className="text-sm font-semibold uppercase tracking-wide text-neutral-600 dark:text-neutral-300">
        {section.title}
      </h3>
      {section.bullets.length === 0 ? (
        <p className="text-sm text-neutral-500 dark:text-neutral-400">
          Nothing here.
        </p>
      ) : (
        <div className="flex flex-col gap-2">
          {groups.map(({ kind, bullets }) => (
            <div
              key={kind ?? "__none__"}
              className="flex flex-col gap-1"
              data-kind={kind ?? "none"}
              data-kind-bullet-count={bullets.length}
            >
              {kind !== null ? (
                <h4 className="flex items-center gap-1.5 text-xs font-medium text-neutral-500 dark:text-neutral-400">
                  <span aria-hidden="true">{SOURCE_KIND_EMOJI[kind]}</span>
                  <span>{SOURCE_KIND_LABEL[kind]}</span>
                </h4>
              ) : null}
              <ul className="flex flex-col gap-1">
                {bullets.map((bullet) => (
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
            </div>
          ))}
        </div>
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
    // DAY-90 TST-v0.2-02. `data-bullet` makes the bullet
    // individually addressable by a section-scoped count query
    // (`[data-section='commits'] [data-bullet]`). `data-testid`
    // stays on the button because that's what BulletRow
    // interactivity tests and the evidence popover key on.
    <li
      className="relative flex items-start gap-2"
      data-bullet={bullet.id}
    >
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
