//! The single, reviewable redaction rule for private-repo commits.
//!
//! Task 2 invariant #5: a repo flagged private (either via
//! [`dayseam_core::LocalRepo::is_private`] in the DB or via a
//! `.dayseam/private` marker file at the repo root) still emits a
//! `CommitAuthored` [`ActivityEvent`] so the user's "I did work
//! here" signal is preserved, but every message-derived field is
//! stripped and the [`RawRef`] is a stub that does **not** round-trip
//! the commit body. The connector never writes redacted commits to
//! the raw store.
//!
//! This module exists as a standalone file because the Phase 1/2
//! cross-cutting review audits the rule in one place. Every change to
//! "what does Privacy::RedactedPrivateRepo preserve?" must happen
//! here and nowhere else.

use std::path::Path;

use dayseam_core::{ActivityEvent, Privacy, RawRef};

/// Returns `true` iff the repository at `repo_root` should be treated
/// as private. The rule is deliberately narrow:
///
/// 1. The caller's `configured_private` flag (from
///    `local_repos.is_private`) takes precedence — the user's DB
///    choice is authoritative.
/// 2. An in-repo marker file `.dayseam/private` (any contents, any
///    size) promotes an otherwise-public repo to private. This lets
///    the user mark a single fork private without having to re-open
///    the setup wizard.
pub fn is_private_repo(repo_root: &Path, configured_private: bool) -> bool {
    if configured_private {
        return true;
    }
    repo_root.join(".dayseam").join("private").exists()
}

/// Redact `event` in place for a private repo. The rule is one
/// function so the review cycle is one file; connector code paths
/// that construct the event always call this before handing it off.
///
/// What stays:
/// * `id` (deterministic from `(source, external_id, kind)`),
/// * `source_id`, `external_id` (commit SHA), `kind`, `occurred_at`,
/// * `actor` — the user opted in to "I did work here" signal; the
///   SHA + identity are exactly that signal.
/// * `links` — the repo-level link so the user can click through.
///
/// What goes:
/// * `title`, `body` — the commit message.
/// * `entities` — these include file paths and issue refs scraped
///   from the message; all sensitive.
/// * `parent_external_id` — may reveal branch/PR structure.
/// * `metadata` — cleared to `{}` because connectors freely stash
///   message-derived metadata there.
/// * `raw_ref` — stubbed so no caller can go "but the raw payload
///   knows". The stub's `storage_key` is prefixed `redacted:` to
///   make the redaction visible in logs.
pub fn redact_private_event(event: &mut ActivityEvent) {
    event.title = String::new();
    event.body = None;
    event.entities.clear();
    event.parent_external_id = None;
    event.metadata = serde_json::json!({});
    event.raw_ref = RawRef {
        storage_key: format!("redacted:local-git:{}", event.external_id),
        content_type: "application/x-redacted".to_string(),
    };
    event.privacy = Privacy::RedactedPrivateRepo;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use dayseam_core::{ActivityKind, Actor, EntityRef, Link};
    use tempfile::tempdir;
    use uuid::Uuid;

    fn sample_event() -> ActivityEvent {
        ActivityEvent {
            id: Uuid::nil(),
            source_id: Uuid::nil(),
            external_id: "abc123".into(),
            kind: ActivityKind::CommitAuthored,
            occurred_at: Utc::now(),
            actor: Actor {
                display_name: "Me".into(),
                email: Some("me@example.com".into()),
                external_id: None,
            },
            title: "fix: secret thing".into(),
            body: Some("Closes PRIVATE-42: rotate the kms key".into()),
            links: vec![Link {
                url: "file:///repo".into(),
                label: Some("repo".into()),
            }],
            entities: vec![EntityRef {
                kind: "issue".into(),
                external_id: "PRIVATE-42".into(),
                label: None,
            }],
            parent_external_id: Some("main".into()),
            metadata: serde_json::json!({"files": ["secrets.rs"]}),
            raw_ref: RawRef {
                storage_key: "git:abc123".into(),
                content_type: "text/plain".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    #[test]
    fn is_private_respects_configured_flag() {
        let d = tempdir().unwrap();
        assert!(is_private_repo(d.path(), true));
        assert!(!is_private_repo(d.path(), false));
    }

    #[test]
    fn is_private_detects_marker_file() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".dayseam")).unwrap();
        std::fs::write(d.path().join(".dayseam").join("private"), b"").unwrap();
        assert!(is_private_repo(d.path(), false));
    }

    #[test]
    fn redact_strips_message_and_entities_but_keeps_identity() {
        let mut e = sample_event();
        let original_actor = e.actor.clone();
        let original_sha = e.external_id.clone();
        let original_links = e.links.clone();

        redact_private_event(&mut e);

        assert_eq!(e.privacy, Privacy::RedactedPrivateRepo);
        assert_eq!(e.title, "");
        assert_eq!(e.body, None);
        assert!(e.entities.is_empty());
        assert_eq!(e.parent_external_id, None);
        assert_eq!(e.metadata, serde_json::json!({}));
        assert_eq!(e.raw_ref.storage_key, "redacted:local-git:abc123");
        assert_eq!(e.raw_ref.content_type, "application/x-redacted");

        // Preserved fields:
        assert_eq!(e.actor, original_actor);
        assert_eq!(e.external_id, original_sha);
        assert_eq!(e.links, original_links);
    }
}
