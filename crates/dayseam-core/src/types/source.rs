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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SourceKind {
    GitLab,
    LocalGit,
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
