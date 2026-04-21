//! Identity resolution types.
//!
//! Dayseam answers the question "was this work done by me?" by walking
//! a two-layer graph:
//!
//! - [`Person`] is a canonical human. v0.1 ships exactly one `Person`
//!   row with `is_self = true`; multi-identity (work laptop vs personal
//!   laptop, bots, contractors) is an additive v0.2 concern.
//! - [`SourceIdentity`] maps a single per-source actor id (an email
//!   address in git, a `gitlab_user_id` from `/user`, a GitHub login,
//!   and later Slack/Jira/Confluence handles) back to a `Person`.
//!
//! The authorship filter a connector applies while normalising
//! `ActivityEvent`s is literally "does this event's actor match any
//! `SourceIdentity` row whose `person_id == ctx.person.id` and
//! `source_id == ctx.source_id`?" See `ARCHITECTURE.md` §8.1.
//!
//! [`Identity`] is the older v0.1-convenience type that bundles all of
//! a single user's external handles into one struct. It stays in place
//! so existing plumbing (setup wizard, DB row) keeps compiling, and the
//! Phase 2 identity-resolution work will retire it in favour of the
//! `Person` + `Vec<SourceIdentity>` model shown above.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// The legacy v0.1 identity record, kept for backwards compatibility
/// with the schema shipped in Phase 1. Do **not** reach for this in new
/// connector code — use [`Person`] + [`SourceIdentity`] instead. This
/// type will be retired in Phase 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Identity {
    pub id: Uuid,
    pub emails: Vec<String>,
    pub gitlab_user_ids: Vec<i64>,
    pub display_name: String,
}

/// One canonical human that Dayseam attributes work to. The only
/// `Person` in v0.1 is the current user (`is_self = true`); later phases
/// add rows for coworkers when cross-person attribution becomes
/// interesting (weekly team digests, manager-of-team reports).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Person {
    pub id: Uuid,
    /// How this person is shown in reports and log lines.
    pub display_name: String,
    /// True for the row representing the Dayseam user themself. Used by
    /// the reporting engine to default filters to "show *my* work" and
    /// by the UI to pick an avatar.
    pub is_self: bool,
}

impl Person {
    /// Construct a fresh `Person` representing the current user. The id
    /// is v4 random — identity linking across sources is the caller's
    /// job (via `SourceIdentity`), not the id's.
    pub fn new_self(display_name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            display_name: display_name.into(),
            is_self: true,
        }
    }
}

/// Maps one per-source actor id (email string, GitLab numeric user id,
/// GitHub login, …) back to a canonical [`Person`]. A single `Person`
/// typically has several `SourceIdentity` rows — at minimum a work
/// email and a `(source, gitlab_user_id)` pair for each configured
/// source.
///
/// Fuzzy-match metadata (`confidence`, `provenance`, `manual_override`)
/// is intentionally **deferred** to v0.2 when real cross-source
/// ambiguity shows up. v0.1 treats every link as an exact, manually
/// approved assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceIdentity {
    /// Unique id for this mapping row. Distinct from the `Person` so
    /// a mapping can be retracted without deleting the person.
    pub id: Uuid,
    pub person_id: Uuid,
    /// Which configured source this identity belongs to. `None` for
    /// source-agnostic identities (e.g. an email address that matches
    /// every git commit regardless of which local repo produced it).
    pub source_id: Option<Uuid>,
    pub kind: SourceIdentityKind,
    /// The opaque external id — formatting depends on `kind`. Stored as
    /// a string so every kind fits one column; numeric ids are rendered
    /// as their decimal form.
    pub external_actor_id: String,
}

/// Tag for which external id space an [`SourceIdentity::external_actor_id`]
/// lives in. Adding a new variant is an additive schema change and does
/// **not** bump the major version; renaming one **does**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SourceIdentityKind {
    /// Git author/committer email address. Matches across every local
    /// repo owned by the user.
    GitEmail,
    /// GitLab numeric user id from `/api/v4/user`.
    GitLabUserId,
    /// GitLab username — the `@handle` form, used in comment mentions.
    GitLabUsername,
    /// GitHub login — the `@handle` form.
    GitHubLogin,
    /// Atlassian Cloud `accountId` — the workspace-scoped opaque id
    /// returned by `GET /rest/api/3/myself`. Deliberately one variant
    /// for both Jira and Confluence: Atlassian Cloud issues one
    /// `accountId` per human that resolves identically on both
    /// products, so a Jira source and a Confluence source for the same
    /// workspace share one `SourceIdentity` row. Added in DAY-73.
    AtlassianAccountId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn person_new_self_sets_is_self_flag() {
        let p = Person::new_self("Vedanth");
        assert!(p.is_self);
        assert_eq!(p.display_name, "Vedanth");
    }

    #[test]
    fn source_identity_round_trips_through_json() {
        let si = SourceIdentity {
            id: Uuid::nil(),
            person_id: Uuid::nil(),
            source_id: Some(Uuid::nil()),
            kind: SourceIdentityKind::GitEmail,
            external_actor_id: "me@example.com".into(),
        };
        let json = serde_json::to_string(&si).unwrap();
        let back: SourceIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(si, back);
    }
}
