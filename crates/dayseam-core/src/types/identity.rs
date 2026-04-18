//! The user's identity across sources. Populated from git email hints and
//! the GitLab `/user` endpoint at connector-setup time; used during dedupe
//! to answer "was this event authored by the person writing the report?"

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// One persona owned by the user. v0.1 only has a single identity row, but
/// the schema permits more so we can support "work laptop" vs "personal
/// laptop" separations later without a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Identity {
    pub id: Uuid,
    pub emails: Vec<String>,
    pub gitlab_user_ids: Vec<i64>,
    pub display_name: String,
}
