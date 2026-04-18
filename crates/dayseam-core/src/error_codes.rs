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
