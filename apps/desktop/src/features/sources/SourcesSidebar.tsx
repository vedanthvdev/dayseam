// Sources row — the configured-sources strip directly under the
// action bar. This replaces the Phase-1 static `SOURCE_PLACEHOLDERS`
// chips with live rows fed by `useSources()`.
//
// Each row renders a dashed-border chip plus a health dot (green =
// `ok`, amber = never checked, red = last probe failed) and, for
// `LocalGit` sources, the number of `.git` repos that were actually
// discovered under the configured scan roots. That count comes from
// `local_repos_list`, so it reflects what sync would actually walk —
// more useful than surfacing the raw scan-roots count which told
// the user nothing about whether the roots contained any repos.
//
// Hovering (or keyboard-focusing) a chip reveals three affordances:
// Rescan (↻), Edit (✎), and Delete (✕). Rescan fires
// `sources_healthcheck(id)`; Edit reopens the source's add dialog
// (different per kind) in edit mode; Delete asks for confirmation
// before calling `sources_delete(id)`.
//
// When a source's `last_health.ok` is false and the error code is a
// known `gitlab.*` code, the chip also renders a `SourceErrorCard`
// directly below it. Auth-flavoured codes (`gitlab.auth.invalid_token`,
// `gitlab.auth.missing_scope`) expose a "Reconnect" button that
// re-opens `AddGitlabSourceDialog` in edit mode pre-seeded with the
// existing base URL and identity so the user can paste a fresh PAT
// without losing the attached identities.
//
// Adding a source is a two-item menu: "Local git" reopens the
// long-standing `AddLocalGitSourceDialog`; "GitLab" opens
// `AddGitlabSourceDialog`. Keeping the menu flat (no nested sub-
// menus) matches the rest of the action bar.

import { useCallback, useEffect, useRef, useState } from "react";
import type { DayseamError, Source, SourceHealth } from "@dayseam/ipc-types";
import { useLocalRepos, useSources } from "../../ipc";
import { AddLocalGitSourceDialog } from "./AddLocalGitSourceDialog";
import { AddGitlabSourceDialog } from "./AddGitlabSourceDialog";
import { AddAtlassianSourceDialog } from "./AddAtlassianSourceDialog";
import { AddGithubSourceDialog } from "./AddGithubSourceDialog";
import { ApproveReposDialog } from "./ApproveReposDialog";
import { SourceErrorCard } from "./SourceErrorCard";

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


export function SourcesSidebar() {
  const { sources, loading, error, refresh, healthcheck, remove } = useSources();
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
  // component would over-generalise both.
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

  const handleHealthcheck = useCallback(
    (id: string) => {
      void healthcheck(id);
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

  return (
    <section
      aria-label="Connected sources"
      className="flex flex-wrap items-center gap-2 border-b border-neutral-200 px-6 py-3 dark:border-neutral-800"
    >
      <span className="text-xs uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
        Sources
      </span>

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
          onEdit={() => setEditing(source)}
          onRequestDelete={() => {
            setDeleteError(null);
            setDeleting(source);
          }}
          onReconnect={handleReconnect}
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
            className="absolute right-0 z-40 mt-1 w-44 rounded border border-neutral-200 bg-white py-1 text-xs shadow-lg dark:border-neutral-800 dark:bg-neutral-950"
          >
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setAddMenuOpen(false);
                setAddLocalGitOpen(true);
              }}
              className="block w-full px-3 py-1 text-left text-neutral-700 hover:bg-neutral-50 dark:text-neutral-200 dark:hover:bg-neutral-900"
            >
              Add local git source
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setAddMenuOpen(false);
                setAddGitlabOpen(true);
              }}
              data-testid="sources-add-menu-gitlab"
              className="block w-full px-3 py-1 text-left text-neutral-700 hover:bg-neutral-50 dark:text-neutral-200 dark:hover:bg-neutral-900"
            >
              Add GitLab source
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setAddMenuOpen(false);
                setAddAtlassianOpen(true);
              }}
              data-testid="sources-add-menu-atlassian"
              className="block w-full px-3 py-1 text-left text-neutral-700 hover:bg-neutral-50 dark:text-neutral-200 dark:hover:bg-neutral-900"
            >
              Add Atlassian source
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setAddMenuOpen(false);
                setAddGithubOpen(true);
              }}
              data-testid="sources-add-menu-github"
              className="block w-full px-3 py-1 text-left text-neutral-700 hover:bg-neutral-50 dark:text-neutral-200 dark:hover:bg-neutral-900"
            >
              Add GitHub source
            </button>
          </div>
        ) : null}
      </div>

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
  onEdit: () => void;
  onRequestDelete: () => void;
  onReconnect: (source: Source) => void;
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
  onEdit,
  onRequestDelete,
  onReconnect,
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

  return (
    <div className="flex flex-col items-stretch">
      <span
        title={healthTitle(health)}
        className="group inline-flex cursor-pointer items-center gap-1.5 self-start rounded border border-neutral-300 px-2 py-0.5 text-xs text-neutral-700 dark:border-neutral-700 dark:text-neutral-200"
        data-testid={`source-chip-${source.id}`}
      >
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
            className="rounded px-1 text-[11px] text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800"
            aria-label={`Rescan ${source.label}`}
            title="Rescan"
          >
            ↻
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
      </span>

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
