//! Source connectors — the read-only side of Dayseam. Each configured
//! source represents one place we pull activity from (a GitLab instance, a
//! set of local git scan roots).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::DayseamError;

/// Opaque id for a configured source. We use `Uuid` rather than a string
/// slug so connectors can be reconfigured (e.g. rename a GitLab instance)
/// without breaking primary-key invariants in the activity store.
pub type SourceId = Uuid;

/// The persisted record describing one configured source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Source {
    pub id: SourceId,
    pub kind: SourceKind,
    /// Human-readable label shown in the UI ("gitlab.internal.acme.com",
    /// "Work laptop repos"). Not required to be unique.
    pub label: String,
    pub config: SourceConfig,
    pub secret_ref: Option<SecretRef>,
    pub created_at: DateTime<Utc>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_health: SourceHealth,
}

/// The high-level category of a source. Used for UI grouping and so the
/// dispatcher knows which connector implementation to call.
///
/// `Jira` and `Confluence` were added in DAY-73 (v0.2 Atlassian connectors).
/// A single email + API-token credential can back one source of each kind
/// for the same workspace — the sources share a `secret_ref` pointing at
/// one keychain row (ref-counted on delete in DAY-81). Neither connector
/// implementation ships in DAY-73: this PR only lands the discriminant so
/// later tasks can register themselves into the dispatcher without a
/// core-types amendment. The connector scaffolds in DAY-76 / DAY-79
/// add the matching [`SourceConfig`] variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SourceKind {
    GitLab,
    LocalGit,
    Jira,
    Confluence,
}

/// Per-kind configuration. The enum is externally tagged so the on-disk
/// JSON carries the variant name, which makes schema migrations obvious
/// when we add new source kinds later.
///
/// `LocalGit` intentionally only carries `scan_roots` — approved repos are
/// first-class rows in the `local_repos` table so we never have two
/// sources of truth for the same list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SourceConfig {
    GitLab {
        base_url: String,
        user_id: i64,
        username: String,
    },
    LocalGit {
        scan_roots: Vec<PathBuf>,
    },
}

impl SourceConfig {
    /// Project a [`SourceConfig`] down to its [`SourceKind`] discriminant.
    /// Used by the IPC layer to reject patches that would secretly
    /// widen a `LocalGit` source into a `GitLab` one.
    #[must_use]
    pub fn kind(&self) -> SourceKind {
        match self {
            SourceConfig::GitLab { .. } => SourceKind::GitLab,
            SourceConfig::LocalGit { .. } => SourceKind::LocalGit,
        }
    }
}

/// Partial update payload for the `sources_update` IPC command. Both
/// fields are optional so the frontend can update just the label,
/// just the config, or both in one round-trip. The command enforces
/// that any supplied `config.kind()` matches the persisted source's
/// `kind`; otherwise the call is rejected before any write happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourcePatch {
    pub label: Option<String>,
    pub config: Option<SourceConfig>,
}

/// Opaque handle the secrets crate resolves against the OS keychain. The
/// actual secret bytes never touch the database or IPC layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SecretRef {
    pub keychain_service: String,
    pub keychain_account: String,
}

/// Last observed health of a source. `ok == true` with no error means the
/// last probe succeeded; `ok == false` surfaces the specific
/// `DayseamError` so the UI can display an actionable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceHealth {
    pub ok: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<DayseamError>,
}

impl SourceHealth {
    /// Sensible default for a freshly created source that has never been
    /// probed — we mark it as "ok unless proven otherwise" so the UI
    /// doesn't show a spurious red badge before the first sync.
    pub fn unchecked() -> Self {
        Self {
            ok: true,
            checked_at: None,
            last_error: None,
        }
    }
}

/// Successful return shape of the `gitlab_validate_pat` IPC command. The
/// frontend's add-source dialog captures these two fields onto the new
/// [`SourceConfig::GitLab`] row before persisting the source, so the
/// identity the connector walks by (`user_id`) is the one GitLab itself
/// echoed back, not whatever the user typed. The username is returned
/// alongside purely for UI display — the authoritative match is on the
/// numeric id, which never changes when a username is renamed upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitlabValidationResult {
    pub user_id: i64,
    pub username: String,
}
