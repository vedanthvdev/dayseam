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
/// Run cancelled because the app is shutting down.
pub const RUN_CANCELLED_BY_SHUTDOWN: &str = "run.cancelled.by_shutdown";
/// Run cancelled because a newer run for the same source/date superseded
/// it.
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
    SINK_FS_NOT_WRITABLE,
    SINK_FS_DESTINATION_MISSING,
    SINK_MALFORMED_MARKER,
    SINK_FS_CONCURRENT_WRITE,
    RUN_CANCELLED_BY_USER,
    RUN_CANCELLED_BY_SHUTDOWN,
    RUN_CANCELLED_BY_SUPERSEDED,
    CONNECTOR_UNSUPPORTED_SYNC_REQUEST,
    HTTP_RETRY_BUDGET_EXHAUSTED,
    HTTP_TRANSPORT,
    ORCHESTRATOR_RUN_SUPERSEDED,
    ORCHESTRATOR_RUN_CANCELLED,
    INTERNAL_PROCESS_RESTARTED,
    ORCHESTRATOR_RETENTION_SWEEP_FAILED,
    ORCHESTRATOR_SAVE_DRAFT_NOT_FOUND,
    ORCHESTRATOR_SINK_NOT_REGISTERED,
    IPC_SOURCE_NOT_FOUND,
    IPC_SINK_NOT_FOUND,
    IPC_LOCAL_REPO_NOT_FOUND,
    IPC_REPORT_DRAFT_NOT_FOUND,
    IPC_SOURCE_CONFIG_KIND_MISMATCH,
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
