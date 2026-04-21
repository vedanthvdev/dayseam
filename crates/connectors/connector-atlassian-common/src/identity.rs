//! Atlassian identity-seed helper.
//!
//! v0.1 taught us (DAY-71) that an activity walker producing events
//! before a matching [`SourceIdentity`] row exists ends in a silent
//! "unknown actor" drop at the render stage — every event survives
//! the DB layer and nothing surfaces in the report. The cross-source
//! fix, repeated here for Atlassian, is to **seed the identity row
//! the moment credentials validate**, not at first-sync time.
//!
//! This module provides the pure, layering-clean half of that fix:
//! convert an [`AtlassianAccountInfo`] (from
//! [`crate::cloud::discover_cloud`]) into a [`SourceIdentity`] value
//! the IPC layer can hand straight to
//! `SourceIdentityRepo::ensure` (DAY-82). Keeping the DB write in the
//! IPC layer — where `AppState::pool` and the existing Person repo
//! both already live — mirrors the `ensure_gitlab_self_identity`
//! pattern (`apps/desktop/src-tauri/src/ipc/commands.rs`) and keeps
//! `connector-atlassian-common` database-free.
//!
//! The account-id shape check (empty / non-ASCII / > 128 chars) runs
//! here, and a failure:
//!
//! 1. emits a single `LogLevel::Warn` via the optional `LogSender`
//!    (so the observability surface matches the DAY-72 CORR-08 fix
//!    for malformed `GitLabUserId` rows), **and**
//! 2. returns `Err(DayseamError::UpstreamChanged)` with code
//!    [`dayseam_core::error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID`]
//!    so the IPC caller can refuse to write the source (the
//!    equivalent of DAY-71's rollback-on-failure).

use dayseam_core::{DayseamError, LogLevel, SourceIdentity, SourceIdentityKind};
use dayseam_events::LogSender;
use uuid::Uuid;

use crate::cloud::AtlassianAccountInfo;
use crate::errors::validate_account_id;

/// Build the [`SourceIdentity`] that maps an Atlassian source's
/// `accountId` to the self-person.
///
/// The caller supplies:
/// * `info` — the [`AtlassianAccountInfo`] from
///   [`crate::cloud::discover_cloud`].
/// * `source_id` — the source this identity scopes to.
/// * `person_id` — the self-person this identity binds to.
/// * `logs` — optional observability channel; on a malformed
///   `accountId` this receives one `LogLevel::Warn` event carrying
///   the observed value (mirrors `identity_user_ids` after the
///   DAY-72 CORR-addendum-08 fix).
///
/// On success the returned `SourceIdentity` is ready for
/// `SourceIdentityRepo::ensure` — the unique index on
/// `(person_id, source_id, kind, external_actor_id)` makes the write
/// idempotent, so repeated calls collapse into no-ops the way the
/// startup backfill relies on.
pub fn seed_atlassian_identity(
    info: &AtlassianAccountInfo,
    source_id: Uuid,
    person_id: Uuid,
    logs: Option<&LogSender>,
) -> Result<SourceIdentity, DayseamError> {
    if let Err(err) = validate_account_id(&info.account_id) {
        if let Some(sender) = logs {
            sender.send(
                LogLevel::Warn,
                None,
                "atlassian: rejecting malformed accountId from /myself".to_string(),
                serde_json::json!({
                    "observed": info.account_id,
                    "source_id": source_id,
                }),
            );
        }
        return Err(err.into());
    }

    Ok(SourceIdentity {
        id: Uuid::new_v4(),
        person_id,
        source_id: Some(source_id),
        kind: SourceIdentityKind::AtlassianAccountId,
        external_actor_id: info.account_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dayseam_core::error_codes;
    use dayseam_events::{RunId, RunStreams};

    fn sample_info(account_id: &str) -> AtlassianAccountInfo {
        AtlassianAccountInfo {
            account_id: account_id.into(),
            display_name: "Vedanth Vasudev".into(),
            email: Some("vedanth@modulrfinance.com".into()),
            cloud_id: None,
        }
    }

    #[test]
    fn good_account_id_yields_source_identity_with_correct_kind() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info("5d53f3cbc6b9320d9ea5bdc2");
        let ident = seed_atlassian_identity(&info, source, person, None).unwrap();
        assert_eq!(ident.person_id, person);
        assert_eq!(ident.source_id, Some(source));
        assert_eq!(ident.kind, SourceIdentityKind::AtlassianAccountId);
        assert_eq!(ident.external_actor_id, "5d53f3cbc6b9320d9ea5bdc2");
    }

    #[test]
    fn repeated_calls_produce_different_row_ids_but_same_match_key() {
        // The `id` field is always a fresh UUID; the *dedup* key lives
        // in `(person_id, source_id, kind, external_actor_id)` and
        // must be stable across calls so the DB's unique index can
        // make `ensure` idempotent.
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info("5d53f3cbc6b9320d9ea5bdc2");
        let a = seed_atlassian_identity(&info, source, person, None).unwrap();
        let b = seed_atlassian_identity(&info, source, person, None).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.person_id, b.person_id);
        assert_eq!(a.source_id, b.source_id);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.external_actor_id, b.external_actor_id);
    }

    #[test]
    fn malformed_account_id_errors_with_registered_code_and_emits_warn_log() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info("naïve-account-id"); // non-ASCII

        let streams = RunStreams::new(RunId::new());
        let ((_ptx, ltx), (_, mut lrx)) = streams.split();
        let result = seed_atlassian_identity(&info, source, person, Some(&ltx));
        drop(ltx);

        let Err(err) = result else {
            panic!("expected error for malformed accountId")
        };
        assert_eq!(
            err.code(),
            error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID
        );

        let mut msgs = Vec::new();
        while let Ok(evt) = lrx.try_recv() {
            assert_eq!(evt.level, LogLevel::Warn);
            msgs.push(evt.message);
        }
        assert_eq!(
            msgs.len(),
            1,
            "expected exactly one warn log for malformed accountId"
        );
        assert!(msgs[0].contains("accountId"));
    }

    #[test]
    fn empty_account_id_is_rejected() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info("");
        let err = seed_atlassian_identity(&info, source, person, None).unwrap_err();
        assert_eq!(
            err.code(),
            error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID
        );
    }

    #[test]
    fn overlong_account_id_is_rejected() {
        let source = Uuid::new_v4();
        let person = Uuid::new_v4();
        let info = sample_info(&"a".repeat(200));
        let err = seed_atlassian_identity(&info, source, person, None).unwrap_err();
        assert_eq!(
            err.code(),
            error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID
        );
    }
}
