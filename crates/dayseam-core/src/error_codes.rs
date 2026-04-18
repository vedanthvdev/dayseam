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

// -------- Sinks -------------------------------------------------------------

pub const SINK_FS_NOT_WRITABLE: &str = "sink.fs.not_writable";
pub const SINK_FS_DESTINATION_MISSING: &str = "sink.fs.destination_missing";
pub const SINK_MALFORMED_MARKER: &str = "sink.malformed_marker";

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
    SINK_FS_NOT_WRITABLE,
    SINK_FS_DESTINATION_MISSING,
    SINK_MALFORMED_MARKER,
    RUN_CANCELLED_BY_USER,
    RUN_CANCELLED_BY_SHUTDOWN,
    RUN_CANCELLED_BY_SUPERSEDED,
    CONNECTOR_UNSUPPORTED_SYNC_REQUEST,
    HTTP_RETRY_BUDGET_EXHAUSTED,
    HTTP_TRANSPORT,
    ORCHESTRATOR_RUN_SUPERSEDED,
    ORCHESTRATOR_RUN_CANCELLED,
    INTERNAL_PROCESS_RESTARTED,
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
