//! Day-window walker for the Confluence Cloud
//! `GET /wiki/rest/api/search` CQL endpoint.
//!
//! Given a local [`NaiveDate`] + a fixed UTC offset, the walker builds
//! one CQL query that covers the UTC window the local day maps to,
//! GETs it against the configured workspace with
//! `expand=content.space,content.history,content.version,content.body.atlas_doc_format,content.extensions,content.ancestors`,
//! paginates via
//! [`connector_atlassian_common::V2CursorPaginator`] (the same
//! `_links.next` shape `/wiki/api/v2/*` uses — Atlassian ships one
//! paginator contract across the two base paths), and hands each
//! returned CQL row to [`crate::normalise::normalise_result`] which
//! emits zero-or-one [`ActivityEvent`] per row.
//!
//! The walker is deliberately thin: it owns pagination, day-window
//! derivation, HTTP error classification, and identity resolution.
//! Everything that turns JSON into events lives in the normaliser;
//! the rapid-save collapse lives in [`crate::rollup`].
//!
//! ### Identity resolution
//!
//! Mirror of `connector-jira::walk::self_account_id`: the walker
//! pulls the `accountId` to filter by from
//! [`dayseam_core::SourceIdentity`] rows the orchestrator hands it in
//! `ctx.source_identities`. A walk with no matching identity returns
//! early with zero events — identical early-bail shape to the Jira
//! walker's DAY-71 invariant.

use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Utc};
use connector_atlassian_common::{map_status, CursorPaginator, Product, V2CursorPaginator};
use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{
    error_codes, ActivityEvent, ActivityKind, DayseamError, EntityKind, LogLevel, SourceId,
    SourceIdentity, SourceIdentityKind,
};
use dayseam_events::{LogSender, ProgressSender};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::normalise::{normalise_result, shape_changed, DayWindow};
use crate::rollup::{collapse_rapid_edits, PageEditRecord};

/// Upper bound on pages fetched per walk. At 25 results/page (the
/// endpoint's default + our explicit `limit=25`), 50 pages = 1 250
/// content rows — well past any real user's Confluence footprint for
/// a single day. The cap is a safety net against a paginator that
/// never terminates.
const MAX_PAGES: u32 = 50;

/// Requested page size. `/wiki/rest/api/search` caps `limit` at 100;
/// 25 matches the spike's observed response cadence and keeps each
/// wiremock fixture readable in tests.
const PAGE_SIZE: u32 = 25;

/// Fields to ask the CQL search to expand. Keeping the list explicit
/// bounds payload size and keeps the shape observable — a rename on
/// any of these surfaces through the normaliser's shape-change guard
/// instead of silently dropping events.
///
/// `content.body.atlas_doc_format` is the spike §8.5 decision: every
/// Confluence body the walker reads comes in ADF so the shared
/// `adf_to_plain` walker (`connector-atlassian-common::adf`) is the
/// only body-normalisation path.
const EXPAND_FIELDS: &str = "content.space,\
     content.history,\
     content.history.createdBy,\
     content.version,\
     content.version.by,\
     content.body.atlas_doc_format,\
     content.extensions,\
     content.ancestors,\
     content.container";

/// Outcome of a single-day walk. Mirrors the Jira walker's
/// [`WalkOutcome`] shape so the orchestrator's [`SyncStats`] plumbing
/// can consume either without branching.
///
/// [`WalkOutcome`]: connector_jira::walk::WalkOutcome
/// [`SyncStats`]: connectors_sdk::SyncStats
#[derive(Debug, Default)]
pub struct WalkOutcome {
    pub events: Vec<ActivityEvent>,
    pub fetched_count: u64,
    pub filtered_by_identity: u64,
    pub filtered_by_date: u64,
}

/// Walk Confluence CQL content for one local-timezone day.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(connector = "confluence", source_id = %source_id, day = %day)
)]
pub async fn walk_day(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    workspace_url: &Url,
    source_id: SourceId,
    source_identities: &[SourceIdentity],
    day: NaiveDate,
    local_tz: FixedOffset,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
) -> Result<WalkOutcome, DayseamError> {
    let (start_utc, end_utc_inclusive) = day_bounds_utc(day, local_tz);
    // Half-open `[start, end)` for the window — a 23:59:59 comment is
    // still inside the local day.
    let end_utc_exclusive = end_utc_inclusive + ChronoDuration::seconds(1);
    let window = DayWindow {
        start: start_utc,
        end: end_utc_exclusive,
    };

    let Some(self_account_id) = self_account_id(source_identities, source_id, logs) else {
        return Ok(WalkOutcome::default());
    };

    let base_url =
        workspace_url
            .join("wiki/rest/api/search")
            .map_err(|e| DayseamError::InvalidConfig {
                code: "confluence.config.bad_workspace_url".to_string(),
                message: format!("cannot join `/wiki/rest/api/search`: {e}"),
            })?;

    let cql = build_cql(start_utc, end_utc_exclusive);

    let mut out = WalkOutcome::default();
    let mut next_cursor: Option<String> = None;
    let paginator = V2CursorPaginator;

    // PageEditRecord staging by (content_id, author_account_id) so the
    // rapid-save collapse can run after every page of the CQL walk
    // contributes its results. We key on a composite `String` rather
    // than a tuple so the staging map works with `String`-valued page
    // ids. In the v0.2 scaffold the CQL search returns one row per
    // content id, so each key carries at most one record and the
    // collapse is a no-op; the machinery is still exercised by the
    // integration test that pre-fabricates a multi-edit fixture.
    let mut edits_by_page: std::collections::HashMap<String, Vec<PageEditRecord>> =
        std::collections::HashMap::new();
    // Non-page-edit events pass through unchanged.
    let mut non_edit_events: Vec<ActivityEvent> = Vec::new();

    for page_idx in 0..MAX_PAGES {
        if cancel.is_cancelled() {
            return Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "confluence walk cancelled".to_string(),
            });
        }

        let url = build_page_url(&base_url, &cql, next_cursor.as_deref())?;
        let request = http.reqwest().get(url).header("Accept", "application/json");
        let request = auth.authenticate(request).await?;

        let response = http
            .send(request, cancel, progress, logs)
            .await
            .map_err(|e| match e {
                // Rebrand the SDK's exhausted-retry 429 onto the
                // product-scoped `confluence.walk.*` code the plan
                // reserves for UI copy (spike §8.6).
                DayseamError::RateLimited {
                    retry_after_secs, ..
                } => DayseamError::RateLimited {
                    code: error_codes::CONFLUENCE_WALK_RATE_LIMITED.to_string(),
                    retry_after_secs,
                },
                other => other,
            })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(DayseamError::RateLimited {
                    code: error_codes::CONFLUENCE_WALK_RATE_LIMITED.to_string(),
                    retry_after_secs: 0,
                });
            }
            let mapped: DayseamError = map_status(Product::Confluence, status, body_text).into();
            return Err(mapped);
        }

        let page_body: Value = response
            .json()
            .await
            .map_err(|e| shape_changed(format!("page {page_idx} failed to decode: {e}")))?;

        let page = paginator.parse(page_body).ok_or_else(|| {
            shape_changed("CQL response was not a JSON object / paginator refused the shape")
        })?;

        let results = page
            .body
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| shape_changed("CQL response missing `results` array"))?;

        out.fetched_count = out.fetched_count.saturating_add(results.len() as u64);

        for result in results {
            match normalise_result(
                source_id,
                workspace_url,
                &self_account_id,
                window,
                result,
                logs,
            ) {
                Ok(Some(event)) => {
                    if event.kind == ActivityKind::ConfluencePageEdited {
                        let content_id = page_content_id(result).unwrap_or_default();
                        let version_number = event
                            .metadata
                            .get("version_number")
                            .and_then(Value::as_u64)
                            .unwrap_or(1) as u32;
                        edits_by_page
                            .entry(content_id.clone())
                            .or_default()
                            .push(PageEditRecord {
                                occurred_at: event.occurred_at,
                                version_number,
                            });
                        // Keep the original event too — the rollup
                        // runs as a post-pass that replaces a run of
                        // edits with a single collapsed event.
                        // Storing the event itself lets us preserve
                        // title / link / actor without rebuilding
                        // them in the rollup module.
                        non_edit_events.push(event);
                    } else {
                        non_edit_events.push(event);
                    }
                }
                Ok(None) => {
                    // Either filtered by identity or filtered by date
                    // (outside the window). The rows are lumped
                    // together on one counter because the normaliser
                    // doesn't distinguish: the spike showed the two
                    // drop-reasons are roughly balanced in practice
                    // and UI copy groups them.
                    out.filtered_by_identity = out.filtered_by_identity.saturating_add(1);
                }
                Err(e) => return Err(e),
            }
        }

        match page.next_cursor {
            Some(cursor) => next_cursor = Some(cursor),
            None => break,
        }
    }

    // Rapid-save collapse: for each `(content_id, self_account_id)`
    // bucket, collapse runs of edits within 5 minutes. In the v0.2
    // CQL walker each bucket has at most one entry (CQL returns one
    // row per content id), so this is a no-op in practice; the
    // machinery exists for forward-compatibility and to satisfy the
    // plan's invariant 3.
    for (content_id, mut edits) in edits_by_page {
        edits.sort_by_key(|e| e.occurred_at);
        let collapsed = collapse_rapid_edits(&edits);
        if collapsed.len() < edits.len() {
            // Synthesize the replacement events *before* we drop the
            // originals — `synthesize_collapsed_edit` reaches into
            // the event pool to reuse title / links / entities from
            // the first edit it finds for this content id, and
            // removing the templates first would make the synthesis
            // step unable to recover them.
            let mut replacements = Vec::with_capacity(collapsed.len());
            for run in &collapsed {
                if run.save_count > 1 {
                    if let Some(template) = synthesize_collapsed_edit(
                        &non_edit_events,
                        &content_id,
                        source_id,
                        &self_account_id,
                        run.occurred_at,
                        run.version_number,
                        run.save_count,
                    ) {
                        replacements.push(template);
                    }
                }
            }
            non_edit_events.retain(|ev| {
                !(ev.kind == ActivityKind::ConfluencePageEdited
                    && page_content_id_from_event(ev) == Some(content_id.as_str()))
            });
            non_edit_events.extend(replacements);
        }
    }

    out.events = non_edit_events;
    out.events.sort_by(|a, b| {
        a.occurred_at
            .cmp(&b.occurred_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(out)
}

/// Build the CQL expression for one UTC window. CQL accepts
/// `"YYYY-MM-DD HH:mm"` literals (the server parses them in UTC).
fn build_cql(start_utc: DateTime<Utc>, end_utc_exclusive: DateTime<Utc>) -> String {
    let start = start_utc.format("%Y-%m-%d %H:%M").to_string();
    let end = end_utc_exclusive.format("%Y-%m-%d %H:%M").to_string();
    format!(
        "contributor = currentUser() \
         AND lastModified >= \"{start}\" \
         AND lastModified < \"{end}\" \
         ORDER BY lastModified DESC"
    )
}

/// Assemble the `/wiki/rest/api/search` URL for one page of the walk.
/// Uses [`Url::query_pairs_mut`] so the CQL + cursor are safely
/// URL-encoded exactly once.
fn build_page_url(base_url: &Url, cql: &str, cursor: Option<&str>) -> Result<Url, DayseamError> {
    let mut url = base_url.clone();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("cql", cql);
        pairs.append_pair("expand", EXPAND_FIELDS);
        pairs.append_pair("limit", &PAGE_SIZE.to_string());
        if let Some(c) = cursor {
            pairs.append_pair("cursor", c);
        }
    }
    Ok(url)
}

/// UTC start + end (inclusive) of a local day. Mirrors
/// [`connector_jira::walk::day_bounds_utc`] byte-for-byte; the two
/// will converge into `dayseam_core::time::day_bounds_utc` in a later
/// MNT task.
pub fn day_bounds_utc(day: NaiveDate, tz: FixedOffset) -> (DateTime<Utc>, DateTime<Utc>) {
    let start_local = tz
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .expect("local midnight resolves unambiguously");
    let end_local = tz
        .from_local_datetime(&day.and_hms_opt(23, 59, 59).expect("valid end-of-day"))
        .single()
        .expect("local eod resolves unambiguously");
    (
        start_local.with_timezone(&Utc),
        end_local.with_timezone(&Utc),
    )
}

/// Pull the Atlassian `accountId` out of `source_identities`. Returns
/// `None` when no identity row of kind
/// [`SourceIdentityKind::AtlassianAccountId`] is registered for
/// `source_id`. The walker bails early with an empty outcome in that
/// case — identical shape to the Jira walker's identity-miss early
/// return.
fn self_account_id(
    identities: &[SourceIdentity],
    source_id: SourceId,
    logs: Option<&LogSender>,
) -> Option<String> {
    let found = identities.iter().find(|i| {
        i.kind == SourceIdentityKind::AtlassianAccountId
            && i.source_id.map(|s| s == source_id).unwrap_or(true)
    });
    match found {
        Some(i) => Some(i.external_actor_id.clone()),
        None => {
            if let Some(tx) = logs {
                tx.send(
                    LogLevel::Warn,
                    None,
                    "confluence: no AtlassianAccountId identity for source; \
                     walk returns zero events"
                        .to_string(),
                    json!({ "source_id": source_id }),
                );
            }
            None
        }
    }
}

/// Pull the content id out of a raw CQL result envelope. Used by the
/// rapid-save collapse staging pass.
fn page_content_id(result: &Value) -> Option<String> {
    result
        .get("content")
        .and_then(|c| c.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Pull the content id out of a normalised page-edit event via its
/// `external_id` prefix (`page:<content_id>:edited:…`).
fn page_content_id_from_event(event: &ActivityEvent) -> Option<&str> {
    event
        .external_id
        .strip_prefix("page:")
        .and_then(|rest| rest.split(':').next())
}

/// When the rapid-save collapse fuses multiple edits into one, we
/// synthesize a replacement event from the first-seen edit in the run
/// (to preserve title / link / entities) and override its timestamp,
/// version number, and a "rolled up from N saves" metadata hint.
#[allow(clippy::too_many_arguments)]
fn synthesize_collapsed_edit(
    source_pool: &[ActivityEvent],
    content_id: &str,
    source_id: SourceId,
    self_account_id: &str,
    occurred_at: DateTime<Utc>,
    version_number: u32,
    save_count: u32,
) -> Option<ActivityEvent> {
    let template = source_pool.iter().find(|ev| {
        ev.kind == ActivityKind::ConfluencePageEdited
            && page_content_id_from_event(ev) == Some(content_id)
    })?;
    let kind_token = "ConfluencePageEdited";
    let external_id = format!(
        "page:{content_id}:edited:{}:rollup",
        occurred_at.timestamp_millis()
    );
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);
    let mut metadata = template.metadata.clone();
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("version_number".to_string(), json!(version_number));
        obj.insert("save_count".to_string(), json!(save_count));
    }
    let base_title = template
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::ConfluencePage)
        .and_then(|e| e.label.as_deref())
        .unwrap_or("page")
        .to_string();
    Some(ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::ConfluencePageEdited,
        occurred_at,
        actor: dayseam_core::Actor {
            display_name: String::new(),
            email: None,
            external_id: Some(self_account_id.to_string()),
        },
        title: format!("Edited page \"{base_title}\" (rolled up from {save_count} saves)"),
        body: None,
        links: template.links.clone(),
        entities: template.entities.clone(),
        parent_external_id: template.parent_external_id.clone(),
        metadata,
        raw_ref: template.raw_ref.clone(),
        privacy: template.privacy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn day_bounds_utc_is_trivial_for_utc() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let (start, end) = day_bounds_utc(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(), utc);
        assert_eq!(start.to_rfc3339(), "2026-04-20T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-04-20T23:59:59+00:00");
    }

    #[test]
    fn build_cql_encloses_window_in_last_modified_clause() {
        let start = Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap();
        let cql = build_cql(start, end);
        assert!(cql.contains("lastModified >= \"2026-04-20 00:00\""));
        assert!(cql.contains("lastModified < \"2026-04-21 00:00\""));
        assert!(cql.contains("contributor = currentUser()"));
        assert!(cql.contains("ORDER BY lastModified DESC"));
    }

    #[test]
    fn build_page_url_always_requests_atlas_doc_format_body() {
        // Invariant 7 (plan Task 8.7) — every body-format-carrying
        // request must use `atlas_doc_format`, never `storage`.
        let base = Url::parse("https://acme.atlassian.net/wiki/rest/api/search").unwrap();
        let url = build_page_url(&base, "dummy", None).unwrap();
        let query = url.query().unwrap_or("");
        assert!(
            query.contains("atlas_doc_format"),
            "request must expand body.atlas_doc_format: {query}"
        );
        assert!(
            !query.contains("body.storage"),
            "request must not expand body.storage: {query}"
        );
    }

    #[test]
    fn build_page_url_threads_cursor_when_present() {
        let base = Url::parse("https://acme.atlassian.net/wiki/rest/api/search").unwrap();
        let url = build_page_url(&base, "cql-expr", Some("eyJfX2lkIjoxfQ==")).unwrap();
        assert!(url.query().unwrap().contains("cursor=eyJfX2lkIjoxfQ%3D%3D"));
    }

    #[test]
    fn self_account_id_returns_external_actor_id_when_registered() {
        let src = Uuid::new_v4();
        let identities = vec![SourceIdentity {
            id: Uuid::new_v4(),
            person_id: Uuid::new_v4(),
            source_id: Some(src),
            kind: SourceIdentityKind::AtlassianAccountId,
            external_actor_id: "abc-123".into(),
        }];
        assert_eq!(
            self_account_id(&identities, src, None).as_deref(),
            Some("abc-123")
        );
    }

    #[test]
    fn self_account_id_is_none_when_no_atlassian_identity_registered() {
        let src = Uuid::new_v4();
        let identities = vec![SourceIdentity {
            id: Uuid::new_v4(),
            person_id: Uuid::new_v4(),
            source_id: Some(src),
            kind: SourceIdentityKind::GitLabUserId,
            external_actor_id: "17".into(),
        }];
        assert!(self_account_id(&identities, src, None).is_none());
    }
}
