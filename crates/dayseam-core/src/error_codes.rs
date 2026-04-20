//! Registry of stable machine-readable error codes.
//!
//! Every new code added here is a minor-version bump at worst; renaming or
//! removing a code is a **breaking change** because the frontend and any
//! external tooling (log parsers, support playbooks) key off these
//! literal strings. The `error_codes_registry_snapshot` test in
//! `lib.rs` guards against accidental renames.

// -------- GitLab connector --------------------------------------------------

pub const GITLAB_AUTH_INVALID_TOKEN: &str = "gitlab.auth.invalid_token";
pub const GITLAB_AUTH_MISSING_SCOPE: &str = "gitlab.auth.missing_scope";
pub const GITLAB_URL_DNS: &str = "gitlab.url.dns";
pub const GITLAB_URL_TLS: &str = "gitlab.url.tls";
pub const GITLAB_RATE_LIMITED: &str = "gitlab.rate_limited";
pub const GITLAB_UPSTREAM_5XX: &str = "gitlab.upstream_5xx";
pub const GITLAB_UPSTREAM_SHAPE_CHANGED: &str = "gitlab.upstream_shape_changed";

// -------- Local-git connector ----------------------------------------------

pub const LOCAL_GIT_REPO_LOCKED: &str = "local_git.repo_locked";
pub const LOCAL_GIT_REPO_UNREADABLE: &str = "local_git.repo_unreadable";
/// The repository exists but its object database is corrupt and
/// `git2::Repository::open` / `walk` returned an error that does not
/// fit the "locked" or "unreadable" buckets. Surfaced as a
/// `LogEvent::Error` so the run continues across other repos; the
/// orchestrator still produces a report, just without this repo's
/// commits.
pub const LOCAL_GIT_REPO_CORRUPT: &str = "local_git.repo_corrupt";
/// A configured scan root does not exist on disk. Fatal for the
/// connector's `sync` call (the user asked us to scan a path that
/// isn't there); surfaced as `DayseamError::Io` with a
/// `path = Some(root)` so the UI can render the exact missing path.
pub const LOCAL_GIT_REPO_NOT_FOUND: &str = "local_git.repo_not_found";
/// The repo has no author/committer signature configured and every
/// commit we tried to read came back without a usable email. Emitted
/// as a warning log; we still emit the `CommitAuthored` event because
/// "someone committed here today" is still signal, just without
/// identity attribution.
pub const LOCAL_GIT_NO_SIGNATURE: &str = "local_git.no_signature";
/// Discovery hit the configured `max_roots` cap before finishing
/// walking the scan tree. Surfaced as a warning log with the cap's
/// value and the first N roots so the user can either raise the cap
/// or narrow their scan roots.
pub const LOCAL_GIT_TOO_MANY_ROOTS: &str = "local_git.too_many_roots";
/// One or more commits on the selected day were skipped because the
/// author **and** committer emails are both absent from the user's
/// [`crate::types::source::SourceIdentity`] list for this source.
/// Surfaced as a warning log at the end of a sync so the user can
/// see (a) that there was activity, (b) roughly how many commits
/// got filtered, and (c) a hint pointing at the most common cause —
/// merge commits authored through GitHub / GitLab's web UI, which
/// use the platform's `NNNN+user@users.noreply.github.com` alias
/// instead of the user's real email. DAY-52.
pub const LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY: &str = "local_git.commits_filtered_by_identity";

// -------- Sinks -------------------------------------------------------------

pub const SINK_FS_NOT_WRITABLE: &str = "sink.fs.not_writable";
pub const SINK_FS_DESTINATION_MISSING: &str = "sink.fs.destination_missing";
pub const SINK_MALFORMED_MARKER: &str = "sink.malformed_marker";
/// A concurrent `MarkdownFileSink::write` is already in flight for the
/// same target path. The second caller observes the lock sentinel
/// (`<file>.dayseam.lock`) and refuses to write rather than risk
/// interleaving atomic renames. Surfaced as `DayseamError::Io` with the
/// target path so the UI can offer a "retry in a moment" action.
pub const SINK_FS_CONCURRENT_WRITE: &str = "sink.fs.concurrent_write";

// -------- Connector SDK ----------------------------------------------------

/// Run cancelled by the user (e.g. they clicked Cancel in the UI).
pub const RUN_CANCELLED_BY_USER: &str = "run.cancelled.by_user";
/// Run cancelled because a newer run for the same source/date superseded
/// it.
///
/// A `run.cancelled.by_shutdown` code existed in Phase 1 alongside
/// a `SyncRunCancelReason::Shutdown` variant, but Phase 2 Task 8
/// removed both (LCY-01): no orchestrator path ever emits them, so
/// keeping them in the registry implied an unshipped graceful-
/// shutdown contract. A future Phase 3 shutdown implementation can
/// reintroduce the code (and the matching variant on
/// [`crate::types::run::SyncRunCancelReason`]) at that time.
pub const RUN_CANCELLED_BY_SUPERSEDED: &str = "run.cancelled.by_superseded";
/// Connector does not support the requested `SyncRequest` variant — most
/// commonly `Since(Checkpoint)` against a connector with no incremental
/// fetch. The orchestrator catches this and falls back.
pub const CONNECTOR_UNSUPPORTED_SYNC_REQUEST: &str = "connector.unsupported.sync_request";
/// Generic retry budget exhausted after multiple 429 / 5xx attempts.
pub const HTTP_RETRY_BUDGET_EXHAUSTED: &str = "http.retry.budget_exhausted";
/// Transport-level HTTP failure (DNS, TLS, connection reset) for an
/// endpoint that isn't bound to a specific connector.
pub const HTTP_TRANSPORT: &str = "http.transport";

// -------- Orchestrator ------------------------------------------------------

/// A run's terminal `Cancelled` state is reached because a newer run for the
/// same `(person_id, date, template_id)` tuple superseded it. The stream's
/// final `ProgressPhase::Failed` carries this code so the UI can render a
/// distinct "superseded" chip instead of a generic cancel.
pub const ORCHESTRATOR_RUN_SUPERSEDED: &str = "orchestrator.run.superseded";
/// The orchestrator tripped cancellation on a run (user clicked Cancel,
/// app is shutting down, …). Distinct from the connector-level
/// `run.cancelled.*` codes so log-parsers can tell the difference
/// between "a connector observed cancel" and "the orchestrator
/// intentionally cancelled the whole run".
pub const ORCHESTRATOR_RUN_CANCELLED: &str = "orchestrator.run.cancelled";
/// The orchestrator terminated a run in a `Failed` state because one
/// or more fan-out steps returned an error the orchestrator could
/// not recover from (e.g. a connector returned `Internal`). The
/// stream's final `ProgressPhase::Failed` carries this code so the
/// UI renders a "failed" row, not a "cancelled" one. Emitted by
/// `terminate_failed`; distinct from `ORCHESTRATOR_RUN_CANCELLED`
/// which is specifically about cancel/supersede termination.
pub const ORCHESTRATOR_RUN_FAILED: &str = "orchestrator.run.failed";
/// The orchestrator's startup sweep found a `sync_runs` row still in
/// `Running` with `finished_at IS NULL` — evidence of an unclean
/// shutdown. The row is rewritten to `Failed` with this code so the
/// next UI render can surface the recovery explicitly.
pub const INTERNAL_PROCESS_RESTARTED: &str = "internal.process_restarted";
/// A [`crate::DayseamError::Internal`] surfaced from the retention
/// sweep path (pruning `raw_payloads` / `log_entries`). Distinct from
/// [`INTERNAL_PROCESS_RESTARTED`] so a log parser can tell the two
/// orchestrator maintenance paths apart.
pub const ORCHESTRATOR_RETENTION_SWEEP_FAILED: &str = "orchestrator.retention.sweep_failed";
/// `save_report(draft_id, sink_id)` was called with a `draft_id` that
/// does not resolve to a `report_drafts` row. The Task 6 save dialog
/// surfaces this as an inline row in the dialog (SEC-03 / STD-01
/// double-visibility rule).
pub const ORCHESTRATOR_SAVE_DRAFT_NOT_FOUND: &str = "orchestrator.save.draft_not_found";
/// `save_report` was called with a sink whose kind is not registered
/// in the orchestrator's [`crate::SinkKind`]-keyed registry. Usually
/// a feature-flag mismatch between the Tauri layer and the
/// orchestrator build.
pub const ORCHESTRATOR_SINK_NOT_REGISTERED: &str = "orchestrator.save.sink_not_registered";

// -------- IPC layer --------------------------------------------------------

/// An IPC command was given an `id` (source, sink, local repo, draft)
/// that has no row in the database. Returned as
/// `DayseamError::InvalidConfig` with the resource name in the message
/// so the UI can render an actionable "not found" toast and refresh
/// its list.
pub const IPC_SOURCE_NOT_FOUND: &str = "ipc.source.not_found";
pub const IPC_SINK_NOT_FOUND: &str = "ipc.sink.not_found";
pub const IPC_LOCAL_REPO_NOT_FOUND: &str = "ipc.local_repo.not_found";
pub const IPC_REPORT_DRAFT_NOT_FOUND: &str = "ipc.report_draft.not_found";
/// `sources_update` was called with a `config` whose `kind` does not
/// match the persisted source's `kind`. Surfaced so the UI never
/// silently widens a `LocalGit` source into a `GitLab` source via a
/// patch.
pub const IPC_SOURCE_CONFIG_KIND_MISMATCH: &str = "ipc.source.config_kind_mismatch";

/// `sources_add` / `sources_update` was called for a GitLab source
/// without a PAT, or `sources_healthcheck` / `report_generate` loaded a
/// GitLab source row whose `secret_ref` points at an empty keychain
/// slot. The UI surfaces this as a "Reconnect" prompt rather than as a
/// generic network error because the fix is always the same: paste a
/// fresh PAT. Introduced in DAY-70 after we discovered that the entire
/// GitLab happy path was silently running unauthenticated on self-
/// hosted hosts, returning HTTP 200 with an empty events array.
pub const IPC_GITLAB_PAT_MISSING: &str = "ipc.gitlab.pat_missing";

/// Writing the GitLab PAT to the OS keychain failed. The PAT did not
/// persist, so the subsequent `report_generate` would silently fall
/// back to unauthenticated requests. We abort `sources_add` /
/// `sources_update` so the caller can retry; no half-written source
/// survives.
pub const IPC_GITLAB_KEYCHAIN_WRITE_FAILED: &str = "ipc.gitlab.keychain_write_failed";

/// Reading the GitLab PAT out of the OS keychain failed at
/// `report_generate` / `sources_healthcheck` time. Rendered as an
/// auth-style error so the UI offers Reconnect; the user re-pasting
/// the PAT overwrites whatever stale/corrupt Keychain row is to blame.
pub const IPC_GITLAB_KEYCHAIN_READ_FAILED: &str = "ipc.gitlab.keychain_read_failed";

/// `sinks_add` was called with a `config` whose body fails the IPC
/// layer's structural check (e.g. a `MarkdownFile` sink with an
/// empty `dest_dirs` list, a non-absolute path, or a path with `..`
/// traversal segments). The guard exists so a buggy or hostile
/// frontend cannot wedge a sink that would later refuse every
/// `report_save`.
pub const IPC_SINK_INVALID_CONFIG: &str = "ipc.sink.invalid_config";

/// `persons_update_self` was called with an empty or whitespace-only
/// `display_name`. The command rejects the update before it touches
/// the DB so the onboarding dialog can re-prompt without a round-trip
/// to SQLite.
pub const IPC_INVALID_DISPLAY_NAME: &str = "ipc.persons.invalid_display_name";

/// `shell_open` was asked to open a URL whose scheme is not in the
/// explicit allow-list (`file`, `http`, `https`, `vscode`, `obsidian`).
/// The guard exists so a malicious or buggy evidence row cannot coax
/// Dayseam into handing a `javascript:` URL to the OS.
pub const IPC_SHELL_URL_DISALLOWED: &str = "ipc.shell.url_disallowed";
/// `shell_open` could not parse the provided string as a URL at all.
/// Distinct from `URL_DISALLOWED` so the UI can tell "looks broken"
/// apart from "looks malicious".
pub const IPC_SHELL_URL_INVALID: &str = "ipc.shell.url_invalid";
/// `shell_open` handed the URL to the OS and the OS refused (missing
/// handler, sandbox denial, etc.). Surfaced as `Internal` so it
/// bubbles into a toast without retry.
pub const IPC_SHELL_OPEN_FAILED: &str = "ipc.shell.open_failed";

// -------- Database ---------------------------------------------------------

/// `sqlx::migrate!` failed to apply a pending migration. Always fatal
/// at startup; the app bails out rather than run against a half-
/// migrated schema.
pub const DB_SCHEMA_MIGRATION_FAILED: &str = "db.schema.migration_failed";

/// All known codes in declaration order. The snapshot test iterates over
/// this slice so a missing entry means either the slice wasn't updated or
/// a constant was renamed — in either case review needs to happen.
pub const ALL: &[&str] = &[
    GITLAB_AUTH_INVALID_TOKEN,
    GITLAB_AUTH_MISSING_SCOPE,
    GITLAB_URL_DNS,
    GITLAB_URL_TLS,
    GITLAB_RATE_LIMITED,
    GITLAB_UPSTREAM_5XX,
    GITLAB_UPSTREAM_SHAPE_CHANGED,
    LOCAL_GIT_REPO_LOCKED,
    LOCAL_GIT_REPO_UNREADABLE,
    LOCAL_GIT_REPO_CORRUPT,
    LOCAL_GIT_REPO_NOT_FOUND,
    LOCAL_GIT_NO_SIGNATURE,
    LOCAL_GIT_TOO_MANY_ROOTS,
    LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY,
    SINK_FS_NOT_WRITABLE,
    SINK_FS_DESTINATION_MISSING,
    SINK_MALFORMED_MARKER,
    SINK_FS_CONCURRENT_WRITE,
    RUN_CANCELLED_BY_USER,
    RUN_CANCELLED_BY_SUPERSEDED,
    CONNECTOR_UNSUPPORTED_SYNC_REQUEST,
    HTTP_RETRY_BUDGET_EXHAUSTED,
    HTTP_TRANSPORT,
    ORCHESTRATOR_RUN_SUPERSEDED,
    ORCHESTRATOR_RUN_CANCELLED,
    ORCHESTRATOR_RUN_FAILED,
    INTERNAL_PROCESS_RESTARTED,
    ORCHESTRATOR_RETENTION_SWEEP_FAILED,
    ORCHESTRATOR_SAVE_DRAFT_NOT_FOUND,
    ORCHESTRATOR_SINK_NOT_REGISTERED,
    IPC_SOURCE_NOT_FOUND,
    IPC_SINK_NOT_FOUND,
    IPC_LOCAL_REPO_NOT_FOUND,
    IPC_REPORT_DRAFT_NOT_FOUND,
    IPC_SOURCE_CONFIG_KIND_MISMATCH,
    IPC_SINK_INVALID_CONFIG,
    IPC_INVALID_DISPLAY_NAME,
    IPC_SHELL_URL_DISALLOWED,
    IPC_SHELL_URL_INVALID,
    IPC_SHELL_OPEN_FAILED,
    DB_SCHEMA_MIGRATION_FAILED,
];

#[cfg(test)]
mod tests {
    use super::ALL;
    use std::collections::HashSet;

    #[test]
    fn registry_has_no_duplicates() {
        let set: HashSet<_> = ALL.iter().collect();
        assert_eq!(
            set.len(),
            ALL.len(),
            "duplicate error code in error_codes::ALL"
        );
    }

    #[test]
    fn registry_snapshot() {
        insta::assert_yaml_snapshot!(ALL);
    }
}
