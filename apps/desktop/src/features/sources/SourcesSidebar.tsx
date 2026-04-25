// Sources row — the configured-sources strip directly under the
// app's title bar. Replaces the Phase-1 static `SOURCE_PLACEHOLDERS`
// chips with live rows fed by `useSources()`.
//
// DAY-170: this component now also hosts the report-generation
// action surface that used to live in the separate `ActionRow`. A
// date picker sits on the left, every configured source renders as
// a chip that both *shows* the connection state and *toggles*
// whether the source is included in the next Generate, and the
// primary Generate / Cancel button anchors the right-hand edge
// alongside the Add-source menu. Collapsing the old two-row layout
// into one row keeps the header lean, removes the duplicate "pick
// your sources" checklist, and surfaces every piece of state the
// user has to touch to start a run in a single sentence-shaped
// strip. When the parent doesn't pass `reportActions` (for example
// in unit tests that cover source management in isolation), the
// date + generate half disappears and the row collapses back to
// just the configured-sources list.
//
// Each chip renders the brand mark (coloured — Dayseam's singular
// splash of branded colour, keyed to `ConnectorLogo colored`), a
// health dot (green = `ok`, amber = never checked, red = last probe
// failed), the source label, and — for `LocalGit` sources — the
// number of `.git` repos that were actually discovered under the
// configured scan roots. That count comes from `local_repos_list`
// so it reflects what sync would actually walk. The chip's outer
// frame doubles as the "include in next report" toggle: filled when
// selected, dashed when excluded.
//
// Hovering (or keyboard-focusing) a chip reveals three affordances:
// Rescan (↻), Edit (✎), and Delete (✕). Rescan fires
// `sources_healthcheck(id)`; Edit reopens the source's add dialog
// (different per kind) in edit mode where the user can rename the
// source and/or rotate its credentials; Delete asks for
// confirmation before calling `sources_delete(id)`. DAY-126 folded
// the standalone Rename dialog into Edit because carrying two
// near-identical surfaces for every connector was confusing and
// left GitHub + Atlassian without a label field at all.
//
// When a source's `last_health.ok` is false and the error code is a
// known `gitlab.*` code, the chip also renders a `SourceErrorCard`
// directly below it. Auth-flavoured codes (`gitlab.auth.invalid_token`,
// `gitlab.auth.missing_scope`) expose a "Reconnect" button that
// re-opens `AddGitlabSourceDialog` in edit mode pre-seeded with the
// existing base URL and identity so the user can paste a fresh PAT
// without losing the attached identities.
//
// The Add source menu is flat (no nested sub-menus) and renders
// each connector's coloured brand logo alongside its label so the
// reader's eye lands on "GitLab" / "Jira" / "GitHub" before they
// read a single letter — the same affordance that now lives on
// each configured chip, applied to the "what can I add?" surface.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  DayseamError,
  Source,
  SourceHealth,
  SourceKind,
} from "@dayseam/ipc-types";
import { useLocalRepos, useSources } from "../../ipc";
import type { ReportStatus } from "../../ipc";
import { ConnectorLogo } from "../../components/ConnectorLogo";
import { AddLocalGitSourceDialog } from "./AddLocalGitSourceDialog";
import { AddGitlabSourceDialog } from "./AddGitlabSourceDialog";
import { AddAtlassianSourceDialog } from "./AddAtlassianSourceDialog";
import { AddGithubSourceDialog } from "./AddGithubSourceDialog";
import { ApproveReposDialog } from "./ApproveReposDialog";
import { SourceErrorCard } from "./SourceErrorCard";

/** The Generate / Cancel surface the merged row hosts. Kept as its
 *  own object so the prop is optional without requiring any of the
 *  three leaves individually — if the parent doesn't want the
 *  action UI at all it omits the whole thing. */
export interface SourcesSidebarReportActions {
  status: ReportStatus;
  /** Fired when the user clicks Generate. The parent wires this
   *  into `useReport().generate`. The source-id list is the set of
   *  chip toggles currently in the "included" state. */
  onGenerate: (date: string, sourceIds: string[]) => void;
  /** Fired when the user clicks Cancel during an in-flight run. */
  onCancel: () => void;
}

export interface SourcesSidebarProps {
  /** When provided, the strip also renders the date picker and the
   *  Generate / Cancel button. Absent in unit tests that cover
   *  source management in isolation. */
  reportActions?: SourcesSidebarReportActions;
}

/** YYYY-MM-DD for the user's local today. Mirrors the helper that
 *  used to live in `ActionRow` — the `<input type="date">` element
 *  formats in local tz, so using the ISO UTC date would cause a
 *  near-midnight user in UTC-05:00 to see "yesterday" selected. */
function localTodayIso(): string {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function isRunning(status: ReportStatus): boolean {
  return status === "starting" || status === "running";
}

/** Which `SourceKind` the Add-source menu entry opens. Kept as a
 *  discriminated-union tag in one place so we can drive the menu's
 *  coloured-mark rendering from the same `SourceKind` keys the
 *  chips already use, and so the future "add Slack" landing needs
 *  only one new entry. */
interface AddMenuEntry {
  kind: SourceKind;
  label: string;
  testId: string;
  onClick: () => void;
}

function healthDotClass(health: SourceHealth): string {
  if (!health.checked_at) return "bg-neutral-300 dark:bg-neutral-600";
  return health.ok
    ? "bg-emerald-500 dark:bg-emerald-400"
    : "bg-red-500 dark:bg-red-400";
}

function healthTitle(health: SourceHealth): string {
  if (!health.checked_at) return "Not yet probed";
  if (health.ok) return `Healthy · last checked ${formatWhen(health.checked_at)}`;
  // Every `DayseamError` variant carries a `code` in its `data` blob;
  // the discriminated-union shape means we read it through the nested
  // `.data` rather than a flat `.code`.
  const code = health.last_error?.data.code ?? "unknown";
  return `Error (${code}) at ${formatWhen(health.checked_at)}`;
}

function formatWhen(ts: string): string {
  try {
    const d = new Date(ts);
    if (Number.isNaN(d.getTime())) return ts;
    return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  } catch {
    return ts;
  }
}

function isGitlab(source: Source): boolean {
  return "GitLab" in source.config;
}

function isLocalGit(source: Source): boolean {
  return "LocalGit" in source.config;
}

function isAtlassian(source: Source): boolean {
  return "Jira" in source.config || "Confluence" in source.config;
}

function isGithub(source: Source): boolean {
  return "GitHub" in source.config;
}


export function SourcesSidebar({ reportActions }: SourcesSidebarProps = {}) {
  const { sources, loading, error, refresh, healthcheck, remove } = useSources();

  // ── Report-action state (only used when `reportActions` is
  // provided). Hoisted here from the former `ActionRow` so the
  // merged strip owns everything a user needs to start a run.
  const [date, setDate] = useState<string>(() => localTodayIso());
  const [selected, setSelected] = useState<Set<string>>(() => new Set());

  // First-arrival policy: auto-select every configured source the
  // first time the list arrives. Subsequent additions leave the
  // user's curated selection alone so a power user who toggled
  // `Jira` off doesn't see it silently switched back on when the
  // list refetches (e.g. after a healthcheck). Identical to the
  // policy `ActionRow` carried pre-merge; the tests in
  // `ActionRow.test.tsx` still cover the boundary condition.
  const [hasSeenInitialSources, setHasSeenInitialSources] = useState(false);
  useEffect(() => {
    if (hasSeenInitialSources) return;
    if (loading) return;
    setSelected(new Set(sources.map((s) => s.id)));
    setHasSeenInitialSources(true);
  }, [sources, loading, hasSeenInitialSources]);

  const running = reportActions ? isRunning(reportActions.status) : false;
  const toggleSelected = useCallback((id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const selectedIds = useMemo(() => Array.from(selected), [selected]);
  const canGenerate =
    !!reportActions &&
    !running &&
    sources.length > 0 &&
    selectedIds.length > 0 &&
    Boolean(date);

  // DAY-127 #7 carry-over: the native `<input type="date">` popover
  // needs a blur() nudge after a pointer pick on some Chromium
  // builds to close reliably. But `onChange` also fires for every
  // keyboard value change, so an unconditional blur would boot
  // keyboard users out of the field between every arrow press. We
  // track the last interaction modality via `pointerdown` /
  // `keydown` and only blur when the change came from a pointer.
  const pointerDrivenRef = useRef(false);
  const handleDatePointerDown = useCallback(() => {
    pointerDrivenRef.current = true;
  }, []);
  const handleDateKeyDown = useCallback(() => {
    pointerDrivenRef.current = false;
  }, []);
  const handleDateChange = useCallback(
    (event: React.ChangeEvent<HTMLInputElement>) => {
      setDate(event.target.value);
      if (pointerDrivenRef.current) {
        event.target.blur();
        pointerDrivenRef.current = false;
      }
    },
    [],
  );

  const handleGenerate = useCallback(() => {
    if (!reportActions || !canGenerate) return;
    reportActions.onGenerate(date, selectedIds);
  }, [reportActions, canGenerate, date, selectedIds]);
  // One dialog per kind; tracked separately so menu choice and edit
  // choice can each pick the right one. `addGitlabOpen` + `editing`
  // with a GitLab source share `AddGitlabSourceDialog` but through
  // different props.
  const [addLocalGitOpen, setAddLocalGitOpen] = useState(false);
  const [addGitlabOpen, setAddGitlabOpen] = useState(false);
  // Atlassian dialog is one dialog regardless of whether the user
  // already has one Atlassian product configured — the dialog
  // detects that state itself and steps into Journey C.
  const [addAtlassianOpen, setAddAtlassianOpen] = useState(false);
  const [addGithubOpen, setAddGithubOpen] = useState(false);
  // The two-option "Add source" menu. Closed by default; a click on
  // either item opens the relevant dialog and closes the menu.
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  // When `AddLocalGitSourceDialog` resolves successfully it hands the
  // newly-created source here so we can open ApproveReposDialog on
  // top; keeping the two dialogs coordinated at the parent avoids
  // callback choreography across siblings.
  const [approving, setApproving] = useState<Source | null>(null);
  // Non-null while the edit dialog is open. `editingKind` decides
  // which dialog renders; mixing LocalGit + GitLab in the same
  // component would over-generalise both. DAY-126: all four
  // connector edit dialogs now surface the label field, so rename
  // is part of edit and does not need its own separate state slot.
  const [editing, setEditing] = useState<Source | null>(null);
  // Non-null while the delete confirmation is showing.
  const [deleting, setDeleting] = useState<Source | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [deleteInFlight, setDeleteInFlight] = useState(false);

  const editingLocalGit = editing !== null && isLocalGit(editing) ? editing : null;
  const editingGitlab = editing !== null && isGitlab(editing) ? editing : null;
  // Atlassian "edit" currently only means token rotation — URL and
  // account email are pinned to the bound `SourceIdentity` and a
  // change there would require re-seeding the identity row, which
  // DAY-87 deliberately does not take on. Reuse the same dialog
  // for both the ✎ chip affordance and the Reconnect chip; the
  // copy in reconnect mode is accurate for both entry points.
  const editingAtlassian =
    editing !== null && isAtlassian(editing) ? editing : null;
  // GitHub "edit" follows the Atlassian pattern: URL and label are
  // pinned to the bound `SourceIdentity` (rotating the GitHub account
  // would require re-seeding the identity row, which DAY-99 does not
  // take on). The same dialog handles both the ✎ chip affordance and
  // the Reconnect chip; the copy in reconnect mode is accurate for
  // both entry points.
  const editingGithub =
    editing !== null && isGithub(editing) ? editing : null;

  const addMenuRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!addMenuOpen) return;
    const handler = (e: MouseEvent) => {
      if (!addMenuRef.current) return;
      if (addMenuRef.current.contains(e.target as Node)) return;
      setAddMenuOpen(false);
    };
    window.addEventListener("mousedown", handler);
    return () => window.removeEventListener("mousedown", handler);
  }, [addMenuOpen]);

  // DAY-127 #1: the rescan (↻) button used to fire `healthcheck(id)`
  // and return silently — nothing on the chip changed until the
  // IPC resolved and `sources_list` refetched, so a user clicking
  // ↻ had no idea whether the click registered. We now track the
  // set of in-flight source ids and spin the arrow for that chip
  // while the healthcheck is pending, giving the click an
  // immediate visual acknowledgement.
  const [checkingIds, setCheckingIds] = useState<Set<string>>(
    () => new Set(),
  );
  const handleHealthcheck = useCallback(
    (id: string) => {
      setCheckingIds((prev) => {
        if (prev.has(id)) return prev;
        const next = new Set(prev);
        next.add(id);
        return next;
      });
      void healthcheck(id).finally(() => {
        setCheckingIds((prev) => {
          if (!prev.has(id)) return prev;
          const next = new Set(prev);
          next.delete(id);
          return next;
        });
      });
    },
    [healthcheck],
  );

  const handleReconnect = useCallback((source: Source) => {
    // Reconnect is semantically an edit on the existing row, so we
    // reuse the edit path — `AddGitlabSourceDialog` handles the
    // reconnect copy internally when it detects `editing != null`,
    // and `AddAtlassianSourceDialog`'s DAY-87 reconnect mode
    // activates when we pass the source via its `reconnect` prop
    // (wired below alongside the other edit dialogs).
    setEditing(source);
  }, []);

  const handleAtlassianReconnected = useCallback(
    (affectedIds: string[]) => {
      // `atlassian_sources_reconnect` returns every source id whose
      // keychain slot was rotated (two for shared-PAT sources). Fire
      // `sources_healthcheck` for each so the red chips clear
      // immediately instead of waiting for the next poll; `refresh`
      // on its own would re-read the stale `last_health` snapshot.
      for (const id of affectedIds) {
        void healthcheck(id);
      }
      setEditing(null);
      void refresh();
    },
    [healthcheck, refresh],
  );

  const handleGithubReconnected = useCallback(
    (affectedId: string) => {
      // `github_sources_reconnect` rotates exactly one keychain slot
      // (GitHub is single-source-per-PAT, no Atlassian-style shared-
      // token flow), so the single affected id is all we need to
      // fire healthcheck against to clear the red chip without
      // waiting for the next poll.
      void healthcheck(affectedId);
      setEditing(null);
      void refresh();
    },
    [healthcheck, refresh],
  );

  const handleConfirmDelete = useCallback(async () => {
    if (!deleting) return;
    setDeleteInFlight(true);
    setDeleteError(null);
    try {
      await remove(deleting.id);
      setDeleting(null);
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : JSON.stringify(err));
    } finally {
      setDeleteInFlight(false);
    }
  }, [deleting, remove]);

  // Add-source menu entries. Each carries the canonical `SourceKind`
  // so the menu can render the same coloured brand mark the chip
  // does, keeping the "what will I get if I click this?" signal
  // consistent across surfaces.
  //
  // Note the awkward pairing for Atlassian: one menu entry opens the
  // single `AddAtlassianSourceDialog`, which internally branches
  // between Jira + Confluence flows. We render the Jira mark next to
  // the entry label because it's the more-recognised Atlassian
  // product and the dialog itself shows both names. If DAY-170 review
  // flags this as misleading we can split it into two entries and
  // wire the dialog's `initialProduct` prop; the cost is one extra
  // menu row and a slightly narrower dialog.
  const addMenuEntries: AddMenuEntry[] = [
    {
      kind: "LocalGit",
      label: "Add local git source",
      testId: "sources-add-menu-localgit",
      onClick: () => {
        setAddMenuOpen(false);
        setAddLocalGitOpen(true);
      },
    },
    {
      kind: "GitLab",
      label: "Add GitLab source",
      testId: "sources-add-menu-gitlab",
      onClick: () => {
        setAddMenuOpen(false);
        setAddGitlabOpen(true);
      },
    },
    {
      kind: "Jira",
      label: "Add Atlassian source",
      testId: "sources-add-menu-atlassian",
      onClick: () => {
        setAddMenuOpen(false);
        setAddAtlassianOpen(true);
      },
    },
    {
      kind: "GitHub",
      label: "Add GitHub source",
      testId: "sources-add-menu-github",
      onClick: () => {
        setAddMenuOpen(false);
        setAddGithubOpen(true);
      },
    },
  ];

  return (
    <section
      aria-label={
        reportActions
          ? "Connected sources and report actions"
          : "Connected sources"
      }
      className="flex flex-wrap items-center gap-2 border-b border-neutral-200 px-6 py-3 dark:border-neutral-800"
    >
      {reportActions ? (
        <label className="flex items-center gap-2 text-xs text-neutral-700 dark:text-neutral-200">
          <span>Date</span>
          {/* Shrunk to text-xs + py-0.5 so the date input's baseline
              matches the chip row it now shares — previously the
              input rode a hair taller than the chips when the two
              rows were separate and the mismatch didn't matter. */}
          <input
            type="date"
            value={date}
            onPointerDown={handleDatePointerDown}
            onKeyDown={handleDateKeyDown}
            onChange={handleDateChange}
            disabled={running}
            aria-disabled={running ? "true" : undefined}
            aria-label="Report date"
            data-testid="action-row-date"
            className="rounded border border-neutral-300 bg-white px-2 py-0.5 text-xs text-neutral-800 disabled:cursor-not-allowed disabled:opacity-60 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200"
          />
        </label>
      ) : null}

      {loading && sources.length === 0 ? (
        <span className="text-xs text-neutral-400 dark:text-neutral-500">
          Loading…
        </span>
      ) : null}

      {error ? (
        <span
          role="alert"
          className="text-xs text-red-600 dark:text-red-400"
          title={error}
        >
          Failed to load sources
        </span>
      ) : null}

      {sources.map((source) => (
        <SourceChip
          key={source.id}
          source={source}
          onHealthcheck={handleHealthcheck}
          isChecking={checkingIds.has(source.id)}
          onEdit={() => setEditing(source)}
          onRequestDelete={() => {
            setDeleteError(null);
            setDeleting(source);
          }}
          onReconnect={handleReconnect}
          selectable={reportActions !== undefined}
          selected={selected.has(source.id)}
          onToggleSelected={() => toggleSelected(source.id)}
          running={running}
        />
      ))}

      {sources.length === 0 && !loading && !error ? (
        <span className="text-xs text-neutral-400 dark:text-neutral-500">
          No sources connected
        </span>
      ) : null}

      <div className="relative ml-auto" ref={addMenuRef}>
        <button
          type="button"
          onClick={() => setAddMenuOpen((prev) => !prev)}
          aria-haspopup="menu"
          aria-expanded={addMenuOpen}
          data-testid="sources-add-menu-trigger"
          className="rounded border border-neutral-300 px-2 py-0.5 text-xs text-neutral-700 hover:bg-neutral-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-900"
        >
          Add source
        </button>
        {addMenuOpen ? (
          <div
            role="menu"
            className="absolute right-0 z-40 mt-1 w-56 rounded border border-neutral-200 bg-white py-1 text-xs shadow-lg dark:border-neutral-800 dark:bg-neutral-950"
          >
            {addMenuEntries.map((entry) => (
              <button
                key={entry.testId}
                type="button"
                role="menuitem"
                onClick={entry.onClick}
                data-testid={entry.testId}
                className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-700 hover:bg-neutral-50 dark:text-neutral-200 dark:hover:bg-neutral-900"
              >
                <ConnectorLogo kind={entry.kind} size={14} colored />
                <span>{entry.label}</span>
              </button>
            ))}
          </div>
        ) : null}
      </div>

      {reportActions ? (
        // Pinned-width slot so the Cancel / Generate swap doesn't
        // reflow the row during status transitions — carries over
        // the DAY-127 #2 fix from the old `ActionRow`.
        <div className="flex min-w-[140px] items-center justify-end">
          {running ? (
            <button
              type="button"
              onClick={reportActions.onCancel}
              data-testid="action-row-cancel"
              className="w-full rounded border border-red-300 bg-red-50 px-3 py-1 text-xs font-medium text-red-800 hover:bg-red-100 dark:border-red-800 dark:bg-red-950 dark:text-red-200 dark:hover:bg-red-900"
            >
              Cancel
            </button>
          ) : (
            <button
              type="button"
              onClick={handleGenerate}
              disabled={!canGenerate}
              data-testid="action-row-generate"
              className="w-full rounded border border-transparent bg-neutral-900 px-3 py-1 text-xs font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
            >
              Generate report
            </button>
          )}
        </div>
      ) : null}

      <AddLocalGitSourceDialog
        open={addLocalGitOpen}
        onClose={() => setAddLocalGitOpen(false)}
        onAdded={(source) => {
          setAddLocalGitOpen(false);
          setApproving(source);
          // Refresh in the background so the chip appears even if
          // the user dismisses the approve dialog without approving.
          void refresh();
        }}
      />

      <AddLocalGitSourceDialog
        open={editingLocalGit !== null}
        editing={editingLocalGit}
        onClose={() => setEditing(null)}
        onAdded={() => {
          // Edit mode never reaches onAdded, but the prop is required
          // by the dialog's create-mode contract, so we provide a
          // harmless no-op.
        }}
        onSaved={() => {
          setEditing(null);
          void refresh();
        }}
      />

      <AddGitlabSourceDialog
        open={addGitlabOpen}
        onClose={() => setAddGitlabOpen(false)}
        onAdded={() => {
          setAddGitlabOpen(false);
          void refresh();
        }}
      />

      <AddGitlabSourceDialog
        open={editingGitlab !== null}
        editing={editingGitlab}
        onClose={() => setEditing(null)}
        onAdded={() => {
          // Reconnect never reaches onAdded (edit mode calls onSaved
          // instead), but the create-mode contract requires the prop.
        }}
        onSaved={() => {
          setEditing(null);
          void refresh();
        }}
      />

      <AddAtlassianSourceDialog
        open={addAtlassianOpen}
        onClose={() => setAddAtlassianOpen(false)}
        existingSources={sources}
        onAdded={() => {
          setAddAtlassianOpen(false);
          void refresh();
        }}
      />

      <AddAtlassianSourceDialog
        open={editingAtlassian !== null}
        onClose={() => setEditing(null)}
        // In reconnect mode the dialog ignores `existingSources` and
        // `onAdded`; both props are still required by the current
        // type, so we pass the same values as the add-flow instance
        // (the empty callback never fires in reconnect mode).
        existingSources={sources}
        reconnect={editingAtlassian ? { source: editingAtlassian } : null}
        onAdded={() => {
          // Reconnect mode resolves through `onReconnected` instead.
        }}
        onReconnected={handleAtlassianReconnected}
      />

      <AddGithubSourceDialog
        open={addGithubOpen}
        onClose={() => setAddGithubOpen(false)}
        onAdded={() => {
          setAddGithubOpen(false);
          void refresh();
        }}
      />

      <AddGithubSourceDialog
        open={editingGithub !== null}
        onClose={() => setEditing(null)}
        reconnect={editingGithub ? { source: editingGithub } : null}
        onAdded={() => {
          // Reconnect mode resolves through `onReconnected`.
        }}
        onReconnected={handleGithubReconnected}
      />

      {approving ? (
        <ApproveReposDialog
          source={approving}
          onClose={() => {
            setApproving(null);
            void refresh();
          }}
        />
      ) : null}

      {deleting ? (
        <DeleteSourceConfirmDialog
          source={deleting}
          inFlight={deleteInFlight}
          error={deleteError}
          onCancel={() => {
            if (deleteInFlight) return;
            setDeleting(null);
            setDeleteError(null);
          }}
          onConfirm={() => void handleConfirmDelete()}
        />
      ) : null}
    </section>
  );
}

interface SourceChipProps {
  source: Source;
  onHealthcheck: (id: string) => void;
  /** DAY-127 #1: set true while `sources_healthcheck` for this row
   *  is in flight so the ↻ button can rotate as a visual receipt
   *  for the click. Cleared on promise settlement (success or
   *  failure). */
  isChecking: boolean;
  /** Opens the connector-specific edit dialog. As of DAY-126 every
   *  edit dialog (LocalGit, GitLab, GitHub, Atlassian) surfaces a
   *  Label field, so renaming is part of this one flow instead of
   *  a separate Rename dialog. */
  onEdit: () => void;
  onRequestDelete: () => void;
  onReconnect: (source: Source) => void;
  /** DAY-170: when true the chip body doubles as a toggle for "is
   *  this source included in the next Generate?". Off in unit tests
   *  that don't mount the merged action row, so the chip stays a
   *  passive status readout. */
  selectable: boolean;
  /** DAY-170: whether this source is currently included in the
   *  next Generate. Ignored when `selectable` is false. */
  selected: boolean;
  /** DAY-170: fires when the user toggles inclusion. Invoked via a
   *  checkbox inside the chip to keep the click behaviour obvious
   *  for pointer + keyboard users. */
  onToggleSelected: () => void;
  /** DAY-170: true while a run is in flight. Disables the
   *  inclusion toggle so the user can't accidentally change the
   *  set of sources mid-run (the run already captured the list at
   *  start; re-toggling would create a "checked but not actually
   *  in the run" false impression). */
  running: boolean;
}

/**
 * One row in the sources strip. Factored out of `SourcesSidebar` so
 * each chip can own its own `useLocalRepos(sourceId)` call and show
 * the count of discovered `.git` repos under the source's scan
 * roots. That number — rather than the raw scan-roots count — is
 * what sync actually walks, so it's the useful thing to surface.
 *
 * Per-chip fetch keeps the wiring simple; `useLocalRepos` already
 * listens to the sources bus, so repo counts stay accurate after an
 * edit that changes scan roots. For non-LocalGit sources
 * (GitLab from Phase 3 onward) we skip the query and omit the
 * secondary label until that source kind has its own count model.
 *
 * When the chip's `last_health` surfaces an error, the chip wraps
 * itself + `SourceErrorCard` in a vertical stack. The card lives
 * outside the hover-collapsible affordance cluster so auth errors
 * are always visible — the whole point of the card is to short-
 * circuit "sync failed, I have no idea why".
 */
function SourceChip({
  source,
  onHealthcheck,
  isChecking,
  onEdit,
  onRequestDelete,
  onReconnect,
  selectable,
  selected,
  onToggleSelected,
  running,
}: SourceChipProps) {
  const localGit = isLocalGit(source);
  // `useLocalRepos(null)` short-circuits inside the hook, so we can
  // always call it unconditionally and still avoid an IPC round-trip
  // for non-LocalGit sources — keeping the rules-of-hooks happy.
  const { repos, loading, error } = useLocalRepos(
    localGit ? source.id : null,
  );
  const showCount = localGit;
  const count = repos.length;

  const health = source.last_health;
  const hasHealthError =
    !health.ok && health.checked_at !== null && health.last_error !== null;

  // A GitLab row with no `secret_ref` is definitionally in the
  // "reconnect needed" state — either it was created before
  // sources_add persisted PATs, or the keychain slot was wiped out
  // from under us (OS keychain reset, restored DB on a new machine,
  // etc.). Without this synthesis the row would look healthy until
  // the user either ran a healthcheck or tried to generate a report,
  // so the only clue would be a red toast with no pointer back to
  // the source that caused it. Surfacing the same Reconnect card
  // here closes that discovery gap; the synthesised error mirrors
  // exactly what `build_source_auth` returns on the Rust side so the
  // copy, action token, and testid are all unchanged.
  const needsReconnect =
    isGitlab(source) && source.secret_ref === null && !hasHealthError;
  const syntheticError: DayseamError | null = needsReconnect
    ? {
        variant: "Auth",
        data: {
          code: "gitlab.auth.invalid_token",
          message:
            "No PAT on file for this GitLab source — reconnect to add one.",
          retryable: false,
          action_hint: "reconnect",
        },
      }
    : null;
  const displayedError: DayseamError | null = hasHealthError
    ? health.last_error
    : syntheticError;

  // DAY-170: chip frame flips between "dashed outline, neutral ink"
  // (excluded from next Generate) and "solid outline, neutral fill"
  // (included) so the toggle state is legible without hunting for a
  // checkbox. The brand logo stays coloured either way — the one
  // deliberate splash of colour in the app — so the "which service
  // is this?" signal is preserved regardless of selection state.
  const frameClass = selectable
    ? selected
      ? "border-neutral-700 bg-neutral-100 text-neutral-900 dark:border-neutral-300 dark:bg-neutral-900 dark:text-neutral-100"
      : "border-dashed border-neutral-300 text-neutral-500 dark:border-neutral-700 dark:text-neutral-400"
    : "border-neutral-300 text-neutral-700 dark:border-neutral-700 dark:text-neutral-200";

  // Shared visual payload — logo, health dot, label, optional repo
  // count, and the hover-revealed action cluster. Rendered inside
  // either a `<label>` (when the chip is selectable, so the native
  // label-click semantics toggle the hidden checkbox) or a plain
  // `<span>` (tests and non-report contexts). Splitting the visual
  // from the wrapper keeps the JSX flat and avoids duplicating the
  // inner markup across both branches.
  const chipInner = (
    <>
      <ConnectorLogo
        kind={source.kind}
        size={12}
        colored
        className="shrink-0"
      />
      <span
        aria-hidden="true"
        className={`h-1.5 w-1.5 rounded-full ${healthDotClass(health)}`}
      />
      <span>{source.label}</span>
      {showCount ? (
        <span
          className="text-neutral-500 dark:text-neutral-400"
          // Error surfaces through the title, not the chip body, so
          // a hiccup in `local_repos_list` doesn't scream at the
          // user when the sync path itself is unaffected.
          title={
            error
              ? `Could not read repos under this source: ${error}`
              : undefined
          }
        >
          · {loading && count === 0 ? "…" : `${count} repo${count === 1 ? "" : "s"}`}
        </span>
      ) : null}
      <span
        className="inline-flex w-0 items-center gap-0.5 overflow-hidden opacity-0 transition-all duration-150 group-hover:ml-1 group-hover:w-auto group-hover:opacity-100 group-focus-within:ml-1 group-focus-within:w-auto group-focus-within:opacity-100"
      >
        <button
          type="button"
          onClick={() => onHealthcheck(source.id)}
          disabled={isChecking}
          data-testid={`source-chip-rescan-${source.id}`}
          data-checking={isChecking ? "true" : undefined}
          className="rounded px-1 text-[11px] text-neutral-500 hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-60 dark:text-neutral-400 dark:hover:bg-neutral-800"
          aria-label={`Rescan ${source.label}`}
          aria-busy={isChecking ? "true" : undefined}
          title={isChecking ? "Rescanning…" : "Rescan"}
        >
          <span
            aria-hidden="true"
            className={
              isChecking
                ? "inline-block animate-spin motion-reduce:animate-none"
                : "inline-block"
            }
          >
            ↻
          </span>
        </button>
        <button
          type="button"
          onClick={onEdit}
          className="rounded px-1 text-[11px] text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800"
          aria-label={`Edit ${source.label}`}
          title="Edit"
          data-testid={`source-chip-edit-${source.id}`}
        >
          ✎
        </button>
        <button
          type="button"
          onClick={onRequestDelete}
          className="rounded px-1 text-[11px] text-neutral-500 hover:bg-red-50 hover:text-red-700 dark:text-neutral-400 dark:hover:bg-red-950/40 dark:hover:text-red-300"
          aria-label={`Delete ${source.label}`}
          title="Delete"
          data-testid={`source-chip-delete-${source.id}`}
        >
          ✕
        </button>
      </span>
    </>
  );

  // When selectable we wrap the whole chip in a `<label>` whose
  // `htmlFor` binds the visually-hidden checkbox. Native label
  // semantics then:
  //  - toggle the checkbox on click of any non-interactive
  //    descendant (the logo / health-dot / text / count),
  //  - leave clicks on the action buttons alone (buttons are
  //    interactive elements that the browser excludes from label
  //    click forwarding),
  //  - forward pointer cursor + focus ring to the checkbox so
  //    screen readers hear "Include GitLab in next report,
  //    checked / not checked".
  // The action-row-era testid `action-row-source-<id>` is kept on
  // the checkbox so the existing selection tests still find it.
  const checkboxId = `source-chip-toggle-${source.id}`;
  const bodyClass = `group inline-flex items-center gap-1.5 self-start rounded border px-2 py-0.5 text-xs ${frameClass} ${selectable ? "cursor-pointer" : ""}`;

  return (
    <div className="flex flex-col items-stretch">
      {selectable ? (
        <>
          <input
            id={checkboxId}
            type="checkbox"
            checked={selected}
            disabled={running}
            onChange={onToggleSelected}
            aria-label={`Include ${source.label} in next report`}
            data-testid={`action-row-source-${source.id}`}
            className="sr-only"
          />
          <label
            htmlFor={checkboxId}
            title={healthTitle(health)}
            className={bodyClass}
            data-testid={`source-chip-${source.id}`}
            data-selected={selected ? "true" : "false"}
          >
            {chipInner}
          </label>
        </>
      ) : (
        <span
          title={healthTitle(health)}
          className={bodyClass}
          data-testid={`source-chip-${source.id}`}
        >
          {chipInner}
        </span>
      )}

      {displayedError ? (
        <SourceErrorCard
          source={source}
          error={displayedError}
          onReconnect={onReconnect}
        />
      ) : null}
    </div>
  );
}

interface DeleteSourceConfirmDialogProps {
  source: Source;
  inFlight: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

// Local to this file because the confirm flow is specific to the
// source-chip strip. If a second surface (e.g. sinks row) grows the
// same "confirm + inline error + in-flight spinner" shape we promote
// this into `components/ConfirmDialog.tsx` rather than speculatively
// abstracting now.
function DeleteSourceConfirmDialog({
  source,
  inFlight,
  error,
  onCancel,
  onConfirm,
}: DeleteSourceConfirmDialogProps) {
  // Inlined here (not reusing `Dialog`) because the confirmation is
  // tiny and should not get the larger dialog's size / focus trap
  // defaults. If the product grows more confirm dialogs we revisit.
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="delete-source-confirm-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4"
      data-testid={`source-chip-delete-confirm-${source.id}`}
    >
      <div className="w-full max-w-sm rounded-md border border-neutral-200 bg-white p-4 shadow-lg dark:border-neutral-800 dark:bg-neutral-950">
        <h2
          id="delete-source-confirm-title"
          className="text-sm font-semibold text-neutral-900 dark:text-neutral-50"
        >
          Delete source?
        </h2>
        <p className="mt-2 text-xs text-neutral-600 dark:text-neutral-400">
          This removes <span className="font-medium">{source.label}</span> and
          every approved repo under it from Dayseam. The folders on disk are
          untouched.
        </p>
        {error ? (
          <p
            role="alert"
            className="mt-2 rounded border border-red-300 bg-red-50 px-2 py-1 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
          >
            {error}
          </p>
        ) : null}
        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            disabled={inFlight}
            className="rounded border border-neutral-300 bg-white px-3 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-50 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={inFlight}
            data-testid={`source-chip-delete-confirm-${source.id}-submit`}
            className="rounded bg-red-600 px-3 py-1 text-xs font-medium text-white hover:bg-red-700 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-red-700 dark:hover:bg-red-600"
          >
            {inFlight ? "Deleting…" : "Delete"}
          </button>
        </div>
      </div>
    </div>
  );
}
