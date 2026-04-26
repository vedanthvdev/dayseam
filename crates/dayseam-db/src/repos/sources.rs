//! Source configuration. One row per configured connector (a GitLab
//! instance, a local-git source). Everything that fans out from here —
//! activity events, raw payloads, local repos — cascades on delete so a
//! user "forgetting" a source genuinely clears it out.

use dayseam_core::{SecretRef, Source, SourceConfig, SourceHealth, SourceId, SourceKind};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{DbError, DbResult};

use super::helpers::{parse_rfc3339, source_kind_from_db, source_kind_to_db};

#[derive(Clone)]
pub struct SourceRepo {
    pool: SqlitePool,
}

impl SourceRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, src: &Source) -> DbResult<()> {
        let kind = source_kind_to_db(&src.kind);
        let config = serde_json::to_string(&src.config)?;
        let secret_ref = match &src.secret_ref {
            Some(sr) => Some(serde_json::to_string(sr)?),
            None => None,
        };
        let health = serde_json::to_string(&src.last_health)?;

        sqlx::query(
            "INSERT INTO sources
                (id, kind, label, config_json, secret_ref, created_at, last_sync_at, last_health_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(src.id.to_string())
        .bind(kind)
        .bind(&src.label)
        .bind(config)
        .bind(secret_ref)
        .bind(src.created_at.to_rfc3339())
        .bind(src.last_sync_at.map(|t| t.to_rfc3339()))
        .bind(health)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::classify_sqlx(e, "sources.insert"))?;

        Ok(())
    }

    pub async fn get(&self, id: &SourceId) -> DbResult<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, kind, label, config_json, secret_ref, created_at, last_sync_at, last_health_json
             FROM sources WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_source).transpose()
    }

    pub async fn list(&self) -> DbResult<Vec<Source>> {
        let rows = sqlx::query(
            "SELECT id, kind, label, config_json, secret_ref, created_at, last_sync_at, last_health_json
             FROM sources ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_source).collect()
    }

    /// Overwrite the `label` column for `id`. No-op if the row does
    /// not exist (the caller already re-reads via [`Self::get`] and
    /// surfaces the `None` as a user-visible error).
    pub async fn update_label(&self, id: &SourceId, label: &str) -> DbResult<()> {
        sqlx::query("UPDATE sources SET label = ? WHERE id = ?")
            .bind(label)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Overwrite the `config_json` column for `id`. The caller is
    /// responsible for ensuring `config.kind()` matches the row's
    /// persisted `kind`; replacing a `LocalGit` source with a `GitLab`
    /// config is never a valid operation.
    pub async fn update_config(&self, id: &SourceId, config: &SourceConfig) -> DbResult<()> {
        let config_json = serde_json::to_string(config)?;
        sqlx::query("UPDATE sources SET config_json = ? WHERE id = ?")
            .bind(config_json)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Overwrite the `secret_ref` column for `id`. `None` clears it.
    ///
    /// Introduced in DAY-70 to fix the GitLab report-generation bug: prior
    /// to this, `sources_add` always wrote `secret_ref = NULL` and the
    /// keychain never held a PAT, so every `report_generate` run for a
    /// GitLab source silently fell back to unauthenticated GET requests
    /// — which on a self-hosted instance return HTTP 200 with an empty
    /// `[]` array, and the user saw an empty report.
    pub async fn update_secret_ref(
        &self,
        id: &SourceId,
        secret_ref: Option<&SecretRef>,
    ) -> DbResult<()> {
        let secret_ref_json = match secret_ref {
            Some(sr) => Some(serde_json::to_string(sr)?),
            None => None,
        };
        sqlx::query("UPDATE sources SET secret_ref = ? WHERE id = ?")
            .bind(secret_ref_json)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_health(&self, id: &SourceId, health: &SourceHealth) -> DbResult<()> {
        let health_json = serde_json::to_string(health)?;
        let checked_at = health.checked_at.map(|t| t.to_rfc3339());
        sqlx::query(
            "UPDATE sources SET last_health_json = ?, last_sync_at = COALESCE(?, last_sync_at) WHERE id = ?",
        )
        .bind(health_json)
        .bind(checked_at)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete the `sources` row for `id`. Returns `Some(SecretRef)`
    /// when the deleted row held a `secret_ref` that no *other*
    /// `sources` row still points at — i.e. the caller now owns the
    /// keychain slot and is responsible for dropping it. Returns
    /// `None` when the row had no secret at all, when the row did not
    /// exist, *or* when another source still shares the same
    /// `secret_ref` (the shared-PAT case DAY-81 guards against).
    ///
    /// Why this lives here rather than in the IPC layer: the
    /// "is anyone else still pointing at this secret?" question is a
    /// read of the `sources` table that has to happen inside the
    /// same transaction as the `DELETE` or a second racing source
    /// deletion could make both of them think they were the last
    /// reference and drop the keychain row twice.
    ///
    /// The pre-DAY-81 shape of this method returned `()` and the IPC
    /// layer dropped the keyring secret unconditionally. That was
    /// correct for single-source-per-secret installs (Phase 3) and
    /// becomes silently wrong the moment two `sources` rows share a
    /// `secret_ref` — the v0.2 Atlassian shared-PAT flow. This
    /// signature change is the DB-layer half of the fix; the IPC
    /// layer's half is a `let orphaned = repo.delete(&id).await?;`
    /// / `if let Some(sr) = orphaned { drop_keyring(&sr) }` swap.
    pub async fn delete(&self, id: &SourceId) -> DbResult<Option<SecretRef>> {
        let mut tx = self.pool.begin().await?;

        let secret_ref_json: Option<String> =
            sqlx::query_scalar("SELECT secret_ref FROM sources WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .flatten();

        sqlx::query("DELETE FROM sources WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;

        let orphaned = match secret_ref_json {
            None => None,
            Some(json) => {
                // `secret_ref` is a JSON-serialised `SecretRef`. Two
                // `sources` rows share a secret iff they serialise
                // byte-identically; `SecretRef` is a plain struct with
                // a fixed field order so `serde_json::to_string` is
                // deterministic. A future migration that rewrites the
                // column format needs to revisit this compare.
                let still_referenced: Option<i64> =
                    sqlx::query_scalar("SELECT 1 FROM sources WHERE secret_ref = ? LIMIT 1")
                        .bind(&json)
                        .fetch_optional(&mut *tx)
                        .await?;
                if still_referenced.is_some() {
                    None
                } else {
                    Some(serde_json::from_str::<SecretRef>(&json)?)
                }
            }
        };

        tx.commit().await?;
        Ok(orphaned)
    }

    /// Find the single `sources` row (if any) whose
    /// `SourceConfig::Outlook` carries a matching `tenant_id` + UPN
    /// pair. Used by DAY-203's `outlook_sources_add` to reject a
    /// second add-attempt for a calendar already connected on this
    /// device (two identical rows would double-count meetings in
    /// every downstream report).
    ///
    /// Matches the `SourceConfig::Outlook` JSON shape directly via
    /// SQLite's `json_extract`; `SourceConfig` serialises its variant
    /// as `{"Outlook": { tenant_id, user_principal_name }}` so both
    /// fields live under the `$.Outlook.*` path. Comparisons are
    /// case-sensitive on `tenant_id` (a GUID) and case-insensitive
    /// on `user_principal_name` (an email-shaped string; Microsoft
    /// treats UPN case as cosmetic). Returns at most one row — the
    /// rare case where two rows somehow share an identity (migration
    /// bug, hand-edited DB) is treated as "the first hit wins" so
    /// the caller's duplicate guard still fires.
    pub async fn find_by_outlook_identity(
        &self,
        tenant_id: &str,
        user_principal_name: &str,
    ) -> DbResult<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, kind, label, config_json, secret_ref, created_at, last_sync_at, last_health_json
             FROM sources
             WHERE kind = ?
               AND json_extract(config_json, '$.Outlook.tenant_id') = ?
               AND LOWER(json_extract(config_json, '$.Outlook.user_principal_name')) = LOWER(?)
             LIMIT 1",
        )
        .bind(source_kind_to_db(&SourceKind::Outlook))
        .bind(tenant_id)
        .bind(user_principal_name)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_source).transpose()
    }

    /// Count `secrets` rows — here defined as distinct `secret_ref`
    /// JSON blobs currently referenced by at least one `sources` row.
    /// This is the positive counterpart that the DAY-81 orphan
    /// detector uses on startup to sanity-check the keychain
    /// against the DB; the detector lives in the IPC layer because
    /// the keychain access does.
    pub async fn distinct_secret_refs(&self) -> DbResult<Vec<SecretRef>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT secret_ref FROM sources WHERE secret_ref IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|s| serde_json::from_str::<SecretRef>(&s).map_err(Into::into))
            .collect()
    }
}

fn row_to_source(row: sqlx::sqlite::SqliteRow) -> DbResult<Source> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str).map_err(|e| DbError::InvalidData {
        column: "sources.id".into(),
        message: e.to_string(),
    })?;

    let kind_str: String = row.try_get("kind")?;
    let kind: SourceKind = source_kind_from_db(&kind_str)?;

    let config_json: String = row.try_get("config_json")?;
    let config: SourceConfig = serde_json::from_str(&config_json)?;

    let secret_ref_json: Option<String> = row.try_get("secret_ref")?;
    let secret_ref = match secret_ref_json {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };

    let created_at_str: String = row.try_get("created_at")?;
    let created_at = parse_rfc3339(&created_at_str, "sources.created_at")?;
    let last_sync_at: Option<String> = row.try_get("last_sync_at")?;
    let last_sync_at = match last_sync_at {
        Some(s) => Some(parse_rfc3339(&s, "sources.last_sync_at")?),
        None => None,
    };

    let last_health_json: String = row.try_get("last_health_json")?;
    let last_health: SourceHealth = serde_json::from_str(&last_health_json)?;

    Ok(Source {
        id,
        kind,
        label: row.try_get("label")?,
        config,
        secret_ref,
        created_at,
        last_sync_at,
        last_health,
    })
}
