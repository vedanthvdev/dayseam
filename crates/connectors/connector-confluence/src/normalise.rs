//! Confluence CQL content → [`dayseam_core::ActivityEvent`] mapping.
//!
//! The walker pulls one page of CQL results from
//! `GET /wiki/rest/api/search?cql=contributor%20%3D%20currentUser()%20…`
//! (see `crates/connectors/connector-confluence/src/walk.rs`) and
//! hands each `results[].content` object to this module. One content
//! row yields **zero or one** [`ActivityEvent`]s. The arm that fires
//! depends on `content.type` and the author / version-number shape the
//! spike documented in `docs/spikes/2026-04-20-atlassian-connectors-data-shape.md`
//! §8.2:
//!
//! | Arm | `content.type` | Extra gate | Kind emitted |
//! |---|---|---|---|
//! | Page created | `"page"` | `history.createdBy.accountId == self` AND `version.number == 1` AND `history.createdDate` ∈ window | [`ActivityKind::ConfluencePageCreated`] |
//! | Page edited | `"page"` | `version.by.accountId == self` AND `version.number > 1` AND `version.when` ∈ window | [`ActivityKind::ConfluencePageEdited`] |
//! | Comment | `"comment"` | `history.createdBy.accountId == self` AND `history.createdDate` ∈ window | [`ActivityKind::ConfluenceComment`] |
//!
//! A CQL contributor-search row that matches neither arm (e.g. the
//! user only *commented* on a page, so the page itself has
//! `createdBy != self` AND `version.by != self`) is silently dropped —
//! the comment row in the same response is what surfaces the user's
//! activity, not the page row.
//!
//! The module is deliberately thin on HTTP concerns: it takes already-
//! decoded JSON and returns events. Window bounds + self-identity
//! filtering live in the walker (which also owns pagination); rapid-
//! save collapse lives in [`crate::rollup`].

use chrono::{DateTime, Utc};
use connector_atlassian_common::{adf_to_plain, AtlassianError, Product};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, DayseamError, EntityKind, EntityRef, Link, Privacy, RawRef,
    SourceId,
};
use dayseam_events::LogSender;
use serde_json::{json, Value};
use url::Url;

/// UTC window bounds. A timestamp `ts` is considered in-window iff
/// `start <= ts < end`. Matches [`connector_jira::normalise::DayWindow`]
/// exactly so a reviewer reading one knows the shape of the other.
#[derive(Debug, Clone, Copy)]
pub struct DayWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl DayWindow {
    fn contains(&self, ts: DateTime<Utc>) -> bool {
        ts >= self.start && ts < self.end
    }
}

/// Normalise one CQL result (i.e. `results[i]`) into at most one
/// [`ActivityEvent`].
///
/// Returns `Ok(None)` when the row describes activity outside the
/// window or by a different author — the walker treats that as a
/// filter-by-identity / filter-by-date drop and bumps its counter.
///
/// Returns `Err(DayseamError::UpstreamChanged { code: confluence.walk.upstream_shape_changed, … })`
/// if a mandatory field is missing from the content envelope (`type`,
/// `id`, `space`, or `history`). A missing field is *not* treated as
/// silent zero — the DAY-71 "silent empty report is the worst outcome"
/// invariant applies to Confluence the same way it applies to GitLab /
/// Jira.
pub fn normalise_result(
    source_id: SourceId,
    workspace_url: &Url,
    self_account_id: &str,
    window: DayWindow,
    result: &Value,
    logs: Option<&LogSender>,
) -> Result<Option<ActivityEvent>, DayseamError> {
    // CQL search results under `/wiki/rest/api/search` wrap the
    // content object in a `content` key; each row also carries a
    // top-level `excerpt` and `url` used for rendering.
    let content = result
        .get("content")
        .ok_or_else(|| shape_error("result.content missing"))?;

    let content_type = content
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| shape_error("result.content.type missing"))?;

    let content_id = content
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| shape_error("result.content.id missing"))?
        .to_string();

    let space = content
        .get("space")
        .ok_or_else(|| shape_error("result.content.space missing"))?;
    let space_key = space
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| shape_error("result.content.space.key missing"))?
        .to_string();
    let space_name = space
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let title = content
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // `_links.webui` + `base` assemble the browser URL without
    // double-encoding the `+` / `%20` sequences Confluence already
    // encoded. The spike §8.4 regression-tests that assumption.
    let link = result_link(workspace_url, result, content)?;

    match content_type {
        "page" => normalise_page(
            source_id,
            &content_id,
            &title,
            &space_key,
            space_name.as_deref(),
            &link,
            self_account_id,
            window,
            content,
        ),
        "comment" => normalise_comment(
            source_id,
            &content_id,
            &space_key,
            space_name.as_deref(),
            &link,
            self_account_id,
            window,
            content,
            logs,
        ),
        other => {
            // CQL `contributor = currentUser()` can surface blogposts
            // or attachments; we do not model them as v0.2 activity
            // kinds. Silent drop is the right behaviour — the spike §8
            // explicitly defers them.
            if let Some(tx) = logs {
                tx.send(
                    dayseam_core::LogLevel::Debug,
                    None,
                    format!("confluence: ignoring content.type={other}"),
                    json!({ "content_id": content_id, "content_type": other }),
                );
            }
            Ok(None)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn normalise_page(
    source_id: SourceId,
    content_id: &str,
    title: &str,
    space_key: &str,
    space_name: Option<&str>,
    link: &Link,
    self_account_id: &str,
    window: DayWindow,
    content: &Value,
) -> Result<Option<ActivityEvent>, DayseamError> {
    let history = content
        .get("history")
        .ok_or_else(|| shape_error("page.history missing"))?;
    let version = content.get("version");

    let created_by_account = history
        .get("createdBy")
        .and_then(|c| c.get("accountId"))
        .and_then(Value::as_str);
    let created_at_raw = history.get("createdDate").and_then(Value::as_str);
    let created_at = created_at_raw.and_then(parse_confluence_datetime);

    let version_number = version
        .and_then(|v| v.get("number"))
        .and_then(Value::as_u64)
        .unwrap_or(1) as u32;
    let version_by_account = version
        .and_then(|v| v.get("by"))
        .and_then(|b| b.get("accountId"))
        .and_then(Value::as_str);
    let version_when = version
        .and_then(|v| v.get("when"))
        .and_then(Value::as_str)
        .and_then(parse_confluence_datetime);

    // Arm A — page created by self, still on first version, inside
    // the window.
    let is_self_creator = created_by_account == Some(self_account_id);
    if is_self_creator && version_number == 1 {
        if let Some(ts) = created_at {
            if window.contains(ts) {
                return Ok(Some(build_page_event(
                    source_id,
                    ActivityKind::ConfluencePageCreated,
                    content_id,
                    title,
                    space_key,
                    space_name,
                    link,
                    self_account_id,
                    ts,
                    version_number,
                )));
            }
        }
    }

    // Arm B — latest version authored by self, version.number > 1,
    // inside the window. Handles both "page I own and kept editing"
    // and "collaborative page someone else created whose latest save
    // was mine".
    let is_self_editor = version_by_account == Some(self_account_id);
    if is_self_editor && version_number > 1 {
        if let Some(ts) = version_when {
            if window.contains(ts) {
                return Ok(Some(build_page_event(
                    source_id,
                    ActivityKind::ConfluencePageEdited,
                    content_id,
                    title,
                    space_key,
                    space_name,
                    link,
                    self_account_id,
                    ts,
                    version_number,
                )));
            }
        }
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn normalise_comment(
    source_id: SourceId,
    content_id: &str,
    space_key: &str,
    space_name: Option<&str>,
    link: &Link,
    self_account_id: &str,
    window: DayWindow,
    content: &Value,
    logs: Option<&LogSender>,
) -> Result<Option<ActivityEvent>, DayseamError> {
    let history = content
        .get("history")
        .ok_or_else(|| shape_error("comment.history missing"))?;
    let created_by_account = history
        .get("createdBy")
        .and_then(|c| c.get("accountId"))
        .and_then(Value::as_str);
    if created_by_account != Some(self_account_id) {
        return Ok(None);
    }
    let created_at = history
        .get("createdDate")
        .and_then(Value::as_str)
        .and_then(parse_confluence_datetime);
    let Some(created_at) = created_at else {
        return Ok(None);
    };
    if !window.contains(created_at) {
        return Ok(None);
    }

    let author = history.get("createdBy").unwrap_or(&Value::Null);

    // `extensions.location` is `"inline"` or `"footer"` on real
    // Confluence responses. Missing / other → `"footer"` as the
    // safer default (a regular page-comment).
    let location = content
        .get("extensions")
        .and_then(|e| e.get("location"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "footer".to_string());

    // Body: prefer `content.body.atlas_doc_format.value` (what the
    // walker expands for with `expand=content.body.atlas_doc_format`),
    // falling back to the CQL result's plain `excerpt`. We never
    // consume `body.storage.value` — the plan's Task 8 invariant 7
    // requires `atlas_doc_format` for every body we ask for.
    let body_adf = content
        .get("body")
        .and_then(|b| b.get("atlas_doc_format"))
        .and_then(|adf| adf.get("value"));
    let body_plain = match body_adf {
        Some(Value::String(raw_json)) => {
            // Confluence returns the ADF as a JSON string payload
            // inside `value`. Parse it into a `Value` before running
            // the shared `adf_to_plain` walker so mentions render as
            // `@DisplayName` (spike §8.5 / Jira-parity).
            match serde_json::from_str::<Value>(raw_json) {
                Ok(adf_value) => adf_to_plain(&adf_value, logs),
                Err(_) => String::new(),
            }
        }
        Some(other_value) => adf_to_plain(other_value, logs),
        None => String::new(),
    };

    let body = if body_plain.is_empty() {
        None
    } else {
        Some(body_plain)
    };

    let kind_token = "ConfluenceComment";
    let external_id = format!("comment:{content_id}");
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);

    // CORR-v0.2-01. Surface the parent page as a first-class
    // `confluence_page` entity. Without this, the rollup's
    // `OrphanKey::ConfluencePage` falls back to the literal string
    // `"UNKNOWN"` (`crates/dayseam-report/src/rollup.rs::orphan_key`),
    // which collapses every comment on every page on the day into a
    // single bucket — the bug ships as one bullet "Comments (×17)
    // on UNKNOWN" instead of one bullet per parent page. The
    // `comment_parent_page_id` helper already finds the id; we
    // re-walk the same `ancestors` / `container` shape here to keep
    // the entity in sync with the rollup key without parsing the
    // `page:<id>` string back apart.
    let mut entities = vec![
        space_entity(space_key, space_name),
        comment_entity(content_id),
    ];
    let parent_ref = comment_parent_page_ref(content);
    let has_parent = parent_ref.is_some();
    if let Some((parent_id, parent_title)) = parent_ref {
        entities.push(page_entity(
            &parent_id,
            parent_title.as_deref().unwrap_or(""),
        ));
    } else {
        // CORR-v0.2-06 (narrowed). The CQL row carried neither
        // `ancestors[]` nor `container` — every in-the-wild response
        // we've seen during the spike populated one or the other, so
        // the absence is either a shape drift or a draft/archived
        // page the tenant hides. The normaliser still emits the
        // comment (its body is real work the user did); the rollup
        // layer routes events without a parent page entity into a
        // `ReportSection::Other` bucket so they render with a sane
        // "unattached comments" header instead of collapsing into
        // `UNKNOWN`. The warn surfaces this to the logs panel so an
        // operator can open the CQL response and decide whether to
        // file an upstream-shape ticket. Upgrading to `error` would
        // be wrong: no data loss, no user-visible breakage — just a
        // rendering quality hit that's worth knowing about.
        tracing::warn!(
            target: "connector_confluence::normalise",
            source_id = %source_id,
            comment_id = %content_id,
            space_key = %space_key,
            "confluence comment missing both ancestors[] and container; routing to report Other section",
        );
    }

    // Metadata is the canonical signal for "route to Other" rather
    // than re-deriving parent-absence downstream: report code reads
    // `metadata.unattached == true` and never has to reparse entities.
    // `Default::default` on serde_json::Value is `Value::Null`, which
    // would serialise to `"unattached": null` — use an explicit bool
    // field instead.
    let mut metadata = json!({ "location": location });
    if !has_parent {
        metadata["unattached"] = json!(true);
    }

    Ok(Some(ActivityEvent {
        id,
        source_id,
        external_id,
        kind: ActivityKind::ConfluenceComment,
        occurred_at: created_at,
        actor: actor_from_history(author),
        title: format!("Comment on {}", link.label.as_deref().unwrap_or("page")),
        body,
        links: vec![link.clone()],
        entities,
        parent_external_id: comment_parent_page_id(content),
        metadata,
        raw_ref: RawRef {
            storage_key: format!("confluence:comment:{content_id}"),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_page_event(
    source_id: SourceId,
    kind: ActivityKind,
    content_id: &str,
    title: &str,
    space_key: &str,
    space_name: Option<&str>,
    link: &Link,
    self_account_id: &str,
    occurred_at: DateTime<Utc>,
    version_number: u32,
) -> ActivityEvent {
    let kind_token = match kind {
        ActivityKind::ConfluencePageCreated => "ConfluencePageCreated",
        ActivityKind::ConfluencePageEdited => "ConfluencePageEdited",
        _ => unreachable!("build_page_event called with non-page kind"),
    };
    // For created: external_id collapses multiple saves into one row
    // trivially (there's only ever one `created` event per page). For
    // edited: external_id keys on version.when so a later walk that
    // returns a newer version produces a different deterministic id,
    // which is what the orchestrator's dedup-by-id pass wants.
    let external_id = match kind {
        ActivityKind::ConfluencePageCreated => format!("page:{content_id}:created"),
        ActivityKind::ConfluencePageEdited => format!(
            "page:{content_id}:edited:{}",
            occurred_at.timestamp_millis()
        ),
        _ => unreachable!(),
    };
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, &external_id, kind_token);

    let title_text = match kind {
        ActivityKind::ConfluencePageCreated => {
            if title.is_empty() {
                format!("Created page {content_id}")
            } else {
                format!("Created page \"{title}\"")
            }
        }
        ActivityKind::ConfluencePageEdited => {
            if title.is_empty() {
                format!("Edited page {content_id}")
            } else {
                format!("Edited page \"{title}\"")
            }
        }
        _ => unreachable!(),
    };

    ActivityEvent {
        id,
        source_id,
        external_id,
        kind,
        occurred_at,
        actor: Actor {
            display_name: String::new(),
            email: None,
            external_id: Some(self_account_id.to_string()),
        },
        title: title_text,
        body: None,
        links: vec![link.clone()],
        entities: vec![
            space_entity(space_key, space_name),
            page_entity(content_id, title),
        ],
        parent_external_id: Some(format!("page:{content_id}")),
        metadata: json!({
            "version_number": version_number,
        }),
        raw_ref: RawRef {
            storage_key: format!("confluence:{kind_token}:{content_id}:{occurred_at}"),
            content_type: "application/json".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

fn space_entity(space_key: &str, space_name: Option<&str>) -> EntityRef {
    EntityRef {
        kind: EntityKind::ConfluenceSpace,
        external_id: space_key.to_string(),
        label: space_name.map(str::to_string),
    }
}

fn page_entity(content_id: &str, title: &str) -> EntityRef {
    EntityRef {
        kind: EntityKind::ConfluencePage,
        external_id: content_id.to_string(),
        label: if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        },
    }
}

fn comment_entity(content_id: &str) -> EntityRef {
    EntityRef {
        kind: EntityKind::ConfluenceComment,
        external_id: content_id.to_string(),
        label: None,
    }
}

fn actor_from_history(created_by: &Value) -> Actor {
    Actor {
        display_name: created_by
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        email: created_by
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string),
        external_id: created_by
            .get("accountId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Extract the parent-page id from a comment's `ancestors[]` / `container`
/// hints. Returns `None` if neither is present — the walker then emits
/// the comment without a `parent_external_id`, which the rollup layer
/// treats as "unattached comment".
/// CORR-v0.2-01. Extract the parent page's raw id (no `page:` prefix)
/// and, when available, its title. The title is carried on
/// `ancestors[].title` (the `/rest/api/content/{id}/comment` expand
/// `ancestors` populates it); the container shape doesn't carry a
/// title, so the fallback path intentionally returns `None` there —
/// `page_entity` then renders a labelless entity, which is strictly
/// better than an `UNKNOWN` rollup bucket.
fn comment_parent_page_ref(content: &Value) -> Option<(String, Option<String>)> {
    if let Some(ancestors) = content.get("ancestors").and_then(Value::as_array) {
        if let Some(last) = ancestors.last() {
            if let Some(id) = last.get("id").and_then(Value::as_str) {
                let title = last
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string);
                return Some((id.to_string(), title));
            }
        }
    }
    if let Some(container_id) = content
        .get("container")
        .and_then(|c| c.get("id"))
        .and_then(Value::as_str)
    {
        let title = content
            .get("container")
            .and_then(|c| c.get("title"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        return Some((container_id.to_string(), title));
    }
    None
}

fn comment_parent_page_id(content: &Value) -> Option<String> {
    if let Some(ancestors) = content.get("ancestors").and_then(Value::as_array) {
        // The immediate parent is the *last* ancestor (ancestors are
        // root-to-leaf per Atlassian convention).
        if let Some(last) = ancestors.last() {
            if let Some(id) = last.get("id").and_then(Value::as_str) {
                return Some(format!("page:{id}"));
            }
        }
    }
    if let Some(container_id) = content
        .get("container")
        .and_then(|c| c.get("id"))
        .and_then(Value::as_str)
    {
        return Some(format!("page:{container_id}"));
    }
    None
}

/// Parse Confluence's ISO-8601 timestamps. The spike's fixtures use
/// `"2026-04-20T16:04:00.000Z"` (UTC `Z`), but we also tolerate
/// `"+0000"` / `"+00:00"` because Atlassian's various endpoints have
/// historically disagreed on the offset shape.
fn parse_confluence_datetime(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .or_else(|| DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Assemble the browser URL for a CQL result. Prefers the top-level
/// `_links.base + result.url` shape Confluence's CQL search
/// canonicalises; falls back to joining `_links.webui` onto the
/// workspace URL. Returns a shape-changed error if neither is present
/// — the DAY-71 invariant says a silent empty link is a worse outcome
/// than a loud failure.
fn result_link(workspace_url: &Url, result: &Value, content: &Value) -> Result<Link, DayseamError> {
    // Preferred: `result.url` is a path relative to `_links.base`
    // (which is usually `https://<tenant>.atlassian.net/wiki`).
    let base_from_links = result
        .get("_links")
        .and_then(|l| l.get("base"))
        .and_then(Value::as_str);
    if let (Some(path), Some(base)) = (result.get("url").and_then(Value::as_str), base_from_links) {
        let url = format!("{}{path}", base.trim_end_matches('/'));
        return Ok(Link {
            url,
            label: content
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    // Fallback: `content._links.webui` joined onto the workspace URL.
    if let Some(webui) = content
        .get("_links")
        .and_then(|l| l.get("webui"))
        .and_then(Value::as_str)
    {
        let joined = workspace_url
            .join(&format!("wiki{webui}"))
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("{workspace_url}wiki{webui}"));
        return Ok(Link {
            url: joined,
            label: content
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Err(shape_error(
        "result is missing both top-level url/_links.base and content._links.webui",
    ))
}

fn shape_error(message: impl Into<String>) -> DayseamError {
    AtlassianError::WalkShapeChanged {
        product: Product::Confluence,
        message: message.into(),
    }
    .into()
}

/// Keep the shape-change helper visible to the walker so it can
/// escalate top-level envelope surprises (missing `results` array,
/// un-parseable JSON) with the same code.
pub(crate) fn shape_changed(message: impl Into<String>) -> DayseamError {
    shape_error(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use dayseam_core::error_codes;
    use uuid::Uuid;

    const SELF_ID: &str = "5d53f3cbc6b9320d9ea5bdc2";
    const SHAPE_CHANGED_CODE: &str = error_codes::CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED;

    fn workspace() -> Url {
        Url::parse("https://acme.atlassian.net/").unwrap()
    }

    fn window() -> DayWindow {
        DayWindow {
            start: Utc.with_ymd_and_hms(2026, 4, 20, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap(),
        }
    }

    fn page_result(version_number: u64, version_by: &str, version_when: &str) -> Value {
        json!({
            "content": {
                "id": "2001142074",
                "type": "page",
                "status": "current",
                "title": "Engineering Rota Subscription",
                "space": { "key": "ST", "name": "Delivery Tribes" },
                "history": {
                    "createdDate": "2026-04-10T09:00:00.000Z",
                    "createdBy": {
                        "accountId": "other-account-id",
                        "displayName": "Someone Else"
                    }
                },
                "version": {
                    "number": version_number,
                    "when": version_when,
                    "by": {
                        "accountId": version_by,
                        "displayName": "Edit Author"
                    }
                },
                "_links": {
                    "webui": "/spaces/ST/pages/2001142074/Engineering+Rota+Subscription"
                }
            },
            "url": "/spaces/ST/pages/2001142074/Engineering+Rota+Subscription",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        })
    }

    #[test]
    fn page_first_version_created_by_self_emits_page_created() {
        // Fixture: page with version.number = 1, createdBy = self,
        // createdDate inside the walker's window. Any of those three
        // being wrong must drop the event — the other tests in this
        // module pin that.
        let mut result = page_result(1, SELF_ID, "2026-04-20T09:51:00.000Z");
        result["content"]["history"]["createdBy"]["accountId"] = json!(SELF_ID);
        result["content"]["history"]["createdDate"] = json!("2026-04-20T09:51:00.000Z");
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap()
        .expect("first-version self-authored page should emit");
        assert_eq!(ev.kind, ActivityKind::ConfluencePageCreated);
        assert_eq!(ev.metadata["version_number"], json!(1));
        assert!(ev.title.contains("Engineering Rota Subscription"));
    }

    #[test]
    fn page_later_version_edited_by_self_emits_page_edited() {
        let result = page_result(3, SELF_ID, "2026-04-20T16:04:00.000Z");
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap()
        .expect("self-edited page should emit");
        assert_eq!(ev.kind, ActivityKind::ConfluencePageEdited);
        assert_eq!(ev.metadata["version_number"], json!(3));
    }

    #[test]
    fn page_version_authored_by_other_user_is_dropped() {
        // CQL `contributor = currentUser()` returned the page because
        // the user *commented* on it, but the latest version is not
        // theirs. The page row itself must not emit an edit event —
        // that would claim credit for someone else's write.
        let result = page_result(2, "other-account-id", "2026-04-20T10:00:00.000Z");
        let out = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn page_edit_outside_window_is_dropped() {
        let result = page_result(2, SELF_ID, "2026-04-21T10:00:00.000Z");
        let out = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn comment_by_self_emits_confluence_comment_with_location_metadata() {
        let comment = json!({
            "content": {
                "id": "6239617072",
                "type": "comment",
                "title": "Re: Authy - Playwright implementation",
                "space": { "key": "FET", "name": "Front-End Tribe" },
                "history": {
                    "createdDate": "2026-04-20T10:43:00.000Z",
                    "createdBy": { "accountId": SELF_ID, "displayName": "Me" }
                },
                "extensions": { "location": "inline" },
                "body": {
                    "atlas_doc_format": {
                        "value": "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"looks good\"}]}]}"
                    }
                },
                "container": { "id": "6222414046" },
                "_links": {
                    "webui": "/spaces/FET/pages/6222414046/Authy?focusedCommentId=6239617072"
                }
            },
            "url": "/spaces/FET/pages/6222414046/Authy?focusedCommentId=6239617072",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &comment,
            None,
        )
        .unwrap()
        .expect("self-authored inline comment should emit");
        assert_eq!(ev.kind, ActivityKind::ConfluenceComment);
        assert_eq!(ev.metadata["location"], json!("inline"));
        assert_eq!(ev.body.as_deref(), Some("looks good"));
        assert_eq!(
            ev.parent_external_id.as_deref(),
            Some("page:6222414046"),
            "comment must attach to its parent page id",
        );
        // CORR-v0.2-01 regression. The rollup groups Confluence
        // comments by the `confluence_page` entity's external_id; if
        // the normaliser doesn't push one, the rollup falls back to
        // `"UNKNOWN"` and fans every comment on every page into a
        // single bullet. Title is absent on the container shape, so
        // we only assert the id.
        let page_entity = ev
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::ConfluencePage)
            .expect(
                "comment event must carry a confluence_page entity so the rollup can key on it",
            );
        assert_eq!(page_entity.external_id, "6222414046");
        assert!(page_entity.label.is_none());
    }

    #[test]
    fn comment_with_ancestors_carries_parent_page_title() {
        // CORR-v0.2-01 regression. When Atlassian populates
        // `ancestors[].title` (the `?expand=ancestors` path), the
        // normaliser should carry the parent page title on the
        // `confluence_page` entity so the rollup bullet reads
        // "Comment on <Page Title>" rather than a bare id.
        let comment = json!({
            "content": {
                "id": "9000",
                "type": "comment",
                "title": "Re: Something",
                "space": { "key": "FET", "name": "Front-End Tribe" },
                "history": {
                    "createdDate": "2026-04-20T10:43:00.000Z",
                    "createdBy": { "accountId": SELF_ID, "displayName": "Me" }
                },
                "extensions": { "location": "inline" },
                "body": {
                    "atlas_doc_format": {
                        "value": "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"lgtm\"}]}]}"
                    }
                },
                "ancestors": [
                    { "id": "1000", "title": "Root" },
                    { "id": "2000", "title": "Authy — Playwright implementation" }
                ],
                "_links": { "webui": "/spaces/FET/pages/2000/Authy?focusedCommentId=9000" }
            },
            "url": "/spaces/FET/pages/2000/Authy?focusedCommentId=9000",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &comment,
            None,
        )
        .unwrap()
        .expect("self-authored comment with ancestors should emit");
        let page_entity = ev
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::ConfluencePage)
            .expect("comment must carry parent page entity");
        assert_eq!(page_entity.external_id, "2000");
        assert_eq!(
            page_entity.label.as_deref(),
            Some("Authy — Playwright implementation"),
        );
        assert_eq!(ev.parent_external_id.as_deref(), Some("page:2000"));
    }

    #[test]
    fn comment_by_other_user_is_dropped() {
        let comment = json!({
            "content": {
                "id": "1",
                "type": "comment",
                "title": "Re: Something",
                "space": { "key": "FET" },
                "history": {
                    "createdDate": "2026-04-20T10:43:00.000Z",
                    "createdBy": { "accountId": "other-account-id" }
                },
                "extensions": { "location": "footer" },
                "_links": { "webui": "/x" }
            },
            "url": "/x",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let out = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &comment,
            None,
        )
        .unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn result_without_content_returns_shape_changed() {
        let result =
            json!({ "url": "/x", "_links": { "base": "https://acme.atlassian.net/wiki" } });
        let err = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code(), SHAPE_CHANGED_CODE);
    }

    #[test]
    fn page_without_space_returns_shape_changed() {
        let result = json!({
            "content": {
                "id": "1",
                "type": "page",
                "title": "t",
                "history": {
                    "createdDate": "2026-04-20T09:00:00.000Z",
                    "createdBy": { "accountId": SELF_ID }
                }
            }
        });
        let err = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code(), SHAPE_CHANGED_CODE);
    }

    #[test]
    fn unknown_content_type_drops_silently() {
        let result = json!({
            "content": {
                "id": "1",
                "type": "blogpost",
                "title": "Weekly notes",
                "space": { "key": "ST" },
                "history": {
                    "createdDate": "2026-04-20T09:00:00.000Z",
                    "createdBy": { "accountId": SELF_ID }
                },
                "_links": { "webui": "/x" }
            },
            "url": "/x",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let out = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &result,
            None,
        )
        .unwrap();
        assert!(out.is_none());
    }

    #[test]
    #[tracing_test::traced_test]
    fn comment_without_parent_page_emits_with_unattached_metadata_and_warns() {
        // CORR-v0.2-06 (narrowed) regression. Confluence CQL rows
        // almost always carry `ancestors[]` or `container`, but a
        // shape drift or a draft-parent tenant can produce a comment
        // row without either. Before DAY-88 the event still emitted
        // but carried no `confluence_page` entity — the rollup then
        // fell back to `"UNKNOWN"` and collapsed every unattached
        // comment on the day into one silent bullet. The fix: emit
        // the comment with `metadata.unattached == true` so the
        // report layer can route it to the Other section, *and*
        // `tracing::warn!` so operators notice the upstream-shape
        // anomaly. The event must still be emitted (the body is real
        // user work).
        let comment = json!({
            "content": {
                "id": "42",
                "type": "comment",
                "title": "Re: x",
                "space": { "key": "ST", "name": "Delivery Tribes" },
                "history": {
                    "createdDate": "2026-04-20T10:43:00.000Z",
                    "createdBy": { "accountId": SELF_ID, "displayName": "Me" }
                },
                "extensions": { "location": "footer" },
                "_links": { "webui": "/x" }
            },
            "url": "/x",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &comment,
            None,
        )
        .unwrap()
        .expect("unattached self-authored comment should still emit");
        assert_eq!(ev.kind, ActivityKind::ConfluenceComment);
        assert_eq!(ev.metadata["unattached"], json!(true));
        assert_eq!(ev.metadata["location"], json!("footer"));
        assert!(
            ev.entities
                .iter()
                .all(|e| e.kind != EntityKind::ConfluencePage),
            "unattached comment must not synthesise a confluence_page entity",
        );
        assert!(
            ev.parent_external_id.is_none(),
            "unattached comment must not guess a parent_external_id",
        );
        assert!(logs_contain(
            "confluence comment missing both ancestors[] and container"
        ));
    }

    #[test]
    #[tracing_test::traced_test]
    fn comment_with_parent_container_does_not_set_unattached_and_does_not_warn() {
        // Symmetric to the above: happy path must leave
        // `metadata.unattached` absent (not `false`, not `null`) so
        // downstream can use `get("unattached") == Some(true)` as a
        // three-state predicate without reasoning about defaults.
        let comment = json!({
            "content": {
                "id": "42",
                "type": "comment",
                "title": "Re: x",
                "space": { "key": "ST", "name": "Delivery Tribes" },
                "history": {
                    "createdDate": "2026-04-20T10:43:00.000Z",
                    "createdBy": { "accountId": SELF_ID, "displayName": "Me" }
                },
                "extensions": { "location": "footer" },
                "container": { "id": "999" },
                "_links": { "webui": "/spaces/ST/pages/999" }
            },
            "url": "/spaces/ST/pages/999",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &comment,
            None,
        )
        .unwrap()
        .expect("attached comment emits");
        assert!(
            ev.metadata.get("unattached").is_none(),
            "attached comment must omit `unattached` entirely, got {:?}",
            ev.metadata,
        );
        assert!(!logs_contain(
            "confluence comment missing both ancestors[] and container"
        ));
    }

    #[test]
    fn comment_body_renders_adf_mention_as_display_name() {
        let comment = json!({
            "content": {
                "id": "1",
                "type": "comment",
                "title": "Re: x",
                "space": { "key": "FET" },
                "history": {
                    "createdDate": "2026-04-20T10:43:00.000Z",
                    "createdBy": { "accountId": SELF_ID }
                },
                "extensions": { "location": "footer" },
                "body": {
                    "atlas_doc_format": {
                        "value": "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"hey \"},{\"type\":\"mention\",\"attrs\":{\"id\":\"colleague-account-id\",\"text\":\"@Saravanan\"}}]}]}"
                    }
                },
                "container": { "id": "42" },
                "_links": { "webui": "/x" }
            },
            "url": "/x",
            "_links": { "base": "https://acme.atlassian.net/wiki" }
        });
        let ev = normalise_result(
            Uuid::new_v4(),
            &workspace(),
            SELF_ID,
            window(),
            &comment,
            None,
        )
        .unwrap()
        .expect("comment with adf body should emit");
        let body = ev.body.as_deref().expect("adf body rendered");
        assert!(body.contains("@Saravanan"), "mention display name rendered");
        assert!(
            !body.contains("colleague-account-id"),
            "mention must not leak accountId: {body}"
        );
    }
}
