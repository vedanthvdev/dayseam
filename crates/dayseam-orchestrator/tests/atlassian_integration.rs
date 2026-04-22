//! DAY-90 TST-v0.2-04 — orchestrator-level Atlassian integration tests.
//!
//! These complement the per-connector wiremock tests in
//! `crates/connectors/connector-jira/tests/walk.rs` and
//! `crates/connectors/connector-confluence/tests/walk.rs` by driving
//! the full orchestrator stack end-to-end:
//!
//!   `GenerateRequest` → `Orchestrator::generate_report` →
//!   fan-out → `JiraMux` / `ConfluenceMux` → real `reqwest` call →
//!   `wiremock::MockServer` → walker normalisation → dedup → render →
//!   persisted `ReportDraft`.
//!
//! The per-connector tests pin the walker's contract in isolation;
//! these tests pin the *wiring* of that walker into the orchestrator's
//! fan-out, which is not covered by any other test today. Three
//! scenarios exercise the matrix that matters for the v0.2.1 → v0.3
//! upgrade path:
//!
//! 1. **Jira-only day.** A single Jira source, one self-authored
//!    issue, one comment — ensures the Jira mux hydrates a live
//!    connector at boot and the orchestrator routes the `SourceKind::
//!    Jira` fan-out through it.
//! 2. **Confluence-only day.** A single Confluence source, one
//!    self-authored page edit and one self-authored comment — the
//!    parallel invariant for the Confluence mux, and specifically the
//!    shape that the DAY-79 scaffold + DAY-80 CQL walker produce.
//! 3. **Both-at-once day.** One Jira source + one Confluence source
//!    in the same `GenerateRequest`. Both fan out concurrently, both
//!    hit their own `MockServer`, and both contribute events to the
//!    same persisted draft. This is the "shared Atlassian token" case
//!    from a user's perspective; at the orchestrator layer the two
//!    sources are independent handles so no token is actually shared,
//!    but the test confirms that two Atlassian connectors do not
//!    stomp on each other's per-source state (the same subtle
//!    regression the Phase 3 multi-source dedup test caught for
//!    GitLab + LocalGit).

#![allow(clippy::too_many_lines)]

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use chrono::FixedOffset;
use common::{build_orchestrator, fixture_date, test_person, test_pool};
use connector_confluence::{ConfluenceConfig, ConfluenceMux, ConfluenceSourceCfg};
use connector_jira::{JiraConfig, JiraMux, JiraSourceCfg};
use connectors_sdk::{AuthStrategy, BasicAuth};
use dayseam_core::{
    ActivityKind, Person, Source, SourceConfig, SourceHealth, SourceId, SourceIdentity,
    SourceIdentityKind, SourceKind, SyncRunStatus,
};
use dayseam_orchestrator::{
    orchestrator::{GenerateRequest, SourceHandle},
    ConnectorRegistry, SinkRegistry,
};
use dayseam_report::DEV_EOD_TEMPLATE_ID;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param_contains};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- Constants ------------------------------------------------------------

/// The account id for the self-identity attached to every Atlassian
/// source. Matches the spike fixtures so the field shape stays
/// recognisable; the value is arbitrary otherwise.
const SELF_ACCOUNT: &str = "5d53f3cbc6b9320d9ea5bdc2";

const TEST_EMAIL: &str = "dev@acme.com";

// ---- Scaffolding helpers -------------------------------------------------

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).expect("UTC offset")
}

fn atlassian_auth() -> Arc<dyn AuthStrategy> {
    Arc::new(BasicAuth::atlassian(
        TEST_EMAIL,
        "api-token",
        "dayseam.atlassian",
        "acme",
    ))
}

/// Seed one Atlassian source (Jira or Confluence) directly into the
/// DB with an `AtlassianAccountId` identity, and return the
/// [`SourceHandle`] the orchestrator needs to fan out to it. The
/// stock `common::seed_source` helper wires a `GitEmail` identity
/// which the Atlassian self-filter would reject; this variant is
/// specific to the TST-04 matrix.
async fn seed_atlassian_source(
    pool: &SqlitePool,
    person: &Person,
    kind: SourceKind,
    workspace_url_in_db: &str,
) -> (Source, SourceIdentity, SourceHandle) {
    assert!(
        matches!(kind, SourceKind::Jira | SourceKind::Confluence),
        "seed_atlassian_source is Atlassian-specific; got {kind:?}",
    );
    let config = match kind {
        SourceKind::Jira => SourceConfig::Jira {
            workspace_url: workspace_url_in_db.to_string(),
            email: TEST_EMAIL.to_string(),
        },
        SourceKind::Confluence => SourceConfig::Confluence {
            workspace_url: workspace_url_in_db.to_string(),
            email: TEST_EMAIL.to_string(),
        },
        _ => unreachable!(),
    };
    let source = Source {
        id: Uuid::new_v4(),
        kind,
        label: format!("{kind:?} fixture"),
        config,
        secret_ref: None,
        created_at: chrono::Utc::now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    dayseam_db::SourceRepo::new(pool.clone())
        .insert(&source)
        .await
        .expect("seed source");

    let identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: person.id,
        source_id: Some(source.id),
        kind: SourceIdentityKind::AtlassianAccountId,
        external_actor_id: SELF_ACCOUNT.to_string(),
    };
    dayseam_db::SourceIdentityRepo::new(pool.clone())
        .insert(&identity)
        .await
        .expect("seed identity");

    let handle = SourceHandle {
        source_id: source.id,
        kind,
        auth: atlassian_auth(),
        source_identities: vec![identity.clone()],
    };
    (source, identity, handle)
}

/// Build a `JiraMux` pre-populated with one source pointing at
/// `server_url`. The mux carries `local_tz = UTC` so the
/// `SyncRequest::Day` window the orchestrator computes lines up with
/// the wiremock fixtures' timestamps.
fn build_jira_mux(source_id: SourceId, server_url: &url::Url) -> Arc<JiraMux> {
    Arc::new(JiraMux::new(
        utc(),
        [JiraSourceCfg {
            source_id,
            config: JiraConfig::from_raw(server_url.as_str(), TEST_EMAIL)
                .expect("JiraConfig parses wiremock URL"),
        }],
    ))
}

fn build_confluence_mux(source_id: SourceId, server_url: &url::Url) -> Arc<ConfluenceMux> {
    Arc::new(ConfluenceMux::new(
        utc(),
        [ConfluenceSourceCfg {
            source_id,
            config: ConfluenceConfig::from_raw(server_url.as_str())
                .expect("ConfluenceConfig parses wiremock URL"),
        }],
    ))
}

fn wiremock_workspace(server: &MockServer) -> url::Url {
    // The walkers do `workspace_url.join("rest/api/3/search/jql")` and
    // friends, which drops the last path segment unless the base ends
    // with a trailing slash. `MockServer::uri()` never carries one.
    url::Url::parse(&format!("{}/", server.uri())).expect("parse mockserver uri")
}

async fn mount_jira_jql(server: &MockServer, body: Value) {
    Mock::given(method("POST"))
        .and(path("/rest/api/3/search/jql"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_confluence_cql(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .and(query_param_contains(
            "expand",
            "content.body.atlas_doc_format",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

// ---- Fixture builders ----------------------------------------------------

/// One self-authored Jira issue with a status-transition + self
/// comment, occurring on `fixture_date()`. Matches the happy-path
/// shape from `connector-jira/tests/walk.rs`.
fn jira_happy_path_body() -> Value {
    let histories = json!([
        {
            "id": "1",
            "author": {"accountId": SELF_ACCOUNT, "displayName": "Me"},
            "created": "2026-04-18T10:00:00.000+0000",
            "items": [
                {"field": "status", "fromString": "To Do", "toString": "In Progress",
                 "from": "1", "to": "3"}
            ]
        }
    ]);
    let comments = json!([
        {
            "id": "900",
            "author": {"accountId": SELF_ACCOUNT, "displayName": "Me"},
            "created": "2026-04-18T11:30:00.000+0000",
            "body": {
                "type": "doc",
                "content": [{"type": "paragraph",
                             "content": [{"type": "text", "text": "looks good"}]}]
            }
        }
    ]);
    json!({
        "issues": [{
            "id": "10001",
            "key": "CAR-5117",
            "fields": {
                "summary": "Fix review findings",
                "status": {
                    "name": "In Progress",
                    "statusCategory": {"key": "indeterminate", "name": "In Progress"}
                },
                "issuetype": {"name": "Task"},
                "project": {"id": "10", "key": "CAR", "name": "Test Project"},
                "priority": {"name": "Medium"},
                "labels": [],
                "updated": "2026-04-18T11:30:00.000+0000",
                "comment": {"comments": comments}
            },
            "changelog": {"histories": histories}
        }],
        "isLast": true
    })
}

/// One first-version self-authored Confluence page + one
/// self-authored comment, both occurring on `fixture_date()`.
fn confluence_happy_path_body() -> Value {
    let page = json!({
        "content": {
            "id": "100",
            "type": "page",
            "status": "current",
            "title": "New runbook",
            "space": {"key": "ENG", "name": "Engineering"},
            "history": {
                "createdDate": "2026-04-18T09:00:00.000Z",
                "createdBy": {"accountId": SELF_ACCOUNT, "displayName": "Me"}
            },
            "version": {
                "number": 1,
                "when": "2026-04-18T09:00:00.000Z",
                "by": {"accountId": SELF_ACCOUNT, "displayName": "Me"}
            },
            "body": {
                "atlas_doc_format": {
                    "value": "{\"type\":\"doc\",\"content\":[]}",
                    "representation": "atlas_doc_format"
                }
            },
            "_links": {"webui": "/spaces/ENG/pages/100/New+runbook"}
        },
        "url": "/spaces/ENG/pages/100/New+runbook",
        "_links": {"base": "https://acme.atlassian.net/wiki"}
    });
    let comment = json!({
        "content": {
            "id": "900",
            "type": "comment",
            "status": "current",
            "title": "Re: New runbook",
            "space": {"key": "ENG", "name": "Engineering"},
            "container": {"id": "100", "type": "page", "title": "New runbook"},
            "history": {
                "createdDate": "2026-04-18T10:15:00.000Z",
                "createdBy": {"accountId": SELF_ACCOUNT, "displayName": "Me"}
            },
            "version": {
                "number": 1,
                "when": "2026-04-18T10:15:00.000Z",
                "by": {"accountId": SELF_ACCOUNT, "displayName": "Me"}
            },
            "extensions": {"location": "inline"},
            "body": {
                "atlas_doc_format": {
                    "value": "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"thanks!\"}]}]}",
                    "representation": "atlas_doc_format"
                }
            },
            "_links": {"webui": "/spaces/ENG/pages/100/New+runbook?focusedCommentId=900"}
        },
        "url": "/spaces/ENG/pages/100/New+runbook?focusedCommentId=900",
        "_links": {"base": "https://acme.atlassian.net/wiki"}
    });
    json!({
        "results": [page, comment],
        "limit": 25,
        "size": 2,
        "_links": {}
    })
}

// ---- 1. Jira-only scenario ------------------------------------------------

#[tokio::test]
async fn orchestrator_generates_report_for_jira_only_day() {
    let server = MockServer::start().await;
    mount_jira_jql(&server, jira_happy_path_body()).await;

    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();

    let (src, _id, handle) = seed_atlassian_source(
        &pool,
        &person,
        SourceKind::Jira,
        // The DB row stores the human-typed workspace URL; the live
        // wiremock base is plumbed through the mux below instead. The
        // orchestrator never re-reads `sources.config` during a run;
        // it uses the handle's `source_id` + the mux's per-id lookup.
        "https://acme.atlassian.net",
    )
    .await;

    let jira_mux = build_jira_mux(src.id, &wiremock_workspace(&server));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::Jira, jira_mux);

    let orch = build_orchestrator(pool.clone(), connectors, SinkRegistry::default());

    let request = GenerateRequest {
        person: person.clone(),
        sources: vec![handle],
        date,
        template_id: DEV_EOD_TEMPLATE_ID.to_string(),
        template_version: "0.0.1".to_string(),
        verbose_mode: false,
    };

    let run = orch.generate_report(request).await;
    let outcome = run.completion.await.expect("join");
    assert_eq!(
        outcome.status,
        SyncRunStatus::Completed,
        "the Jira-only day must end Completed: {outcome:#?}",
    );
    let draft_id = outcome.draft_id.expect("completed runs carry a draft id");

    let draft = dayseam_db::DraftRepo::new(pool.clone())
        .get(&draft_id)
        .await
        .expect("draft lookup")
        .expect("draft persisted");

    let activity_ids: Vec<Uuid> = draft
        .evidence
        .iter()
        .flat_map(|e| e.event_ids.iter().copied())
        .collect();
    let events = dayseam_db::ActivityRepo::new(pool.clone())
        .get_many(&activity_ids)
        .await
        .expect("activity lookup");

    // The walker normalises one status transition + one comment; both
    // flow through the orchestrator into the draft's evidence chain.
    let kinds: std::collections::HashSet<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ActivityKind::JiraIssueTransitioned),
        "transition event must reach the draft: got {kinds:?}",
    );
    assert!(
        kinds.contains(&ActivityKind::JiraIssueCommented),
        "comment event must reach the draft: got {kinds:?}",
    );

    // Per-source state must mark the Jira source as Succeeded so the
    // UI's per-source badges light up green, not yellow.
    let per_source = draft
        .per_source_state
        .get(&src.id)
        .expect("draft carries per-source state for the Jira source");
    assert_eq!(
        per_source.status,
        dayseam_core::RunStatus::Succeeded,
        "per-source state must be Succeeded: {per_source:#?}",
    );
    assert!(
        per_source.error.is_none(),
        "per-source state carries no error on a happy-path day: {:#?}",
        per_source.error,
    );
}

// ---- 2. Confluence-only scenario ------------------------------------------

#[tokio::test]
async fn orchestrator_generates_report_for_confluence_only_day() {
    let server = MockServer::start().await;
    mount_confluence_cql(&server, confluence_happy_path_body()).await;

    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();

    let (src, _id, handle) = seed_atlassian_source(
        &pool,
        &person,
        SourceKind::Confluence,
        "https://acme.atlassian.net",
    )
    .await;

    let conf_mux = build_confluence_mux(src.id, &wiremock_workspace(&server));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::Confluence, conf_mux);

    let orch = build_orchestrator(pool.clone(), connectors, SinkRegistry::default());

    let request = GenerateRequest {
        person: person.clone(),
        sources: vec![handle],
        date,
        template_id: DEV_EOD_TEMPLATE_ID.to_string(),
        template_version: "0.0.1".to_string(),
        verbose_mode: false,
    };

    let run = orch.generate_report(request).await;
    let outcome = run.completion.await.expect("join");
    assert_eq!(
        outcome.status,
        SyncRunStatus::Completed,
        "the Confluence-only day must end Completed: {outcome:#?}",
    );
    let draft_id = outcome.draft_id.expect("completed runs carry a draft id");

    let draft = dayseam_db::DraftRepo::new(pool.clone())
        .get(&draft_id)
        .await
        .expect("draft lookup")
        .expect("draft persisted");

    let activity_ids: Vec<Uuid> = draft
        .evidence
        .iter()
        .flat_map(|e| e.event_ids.iter().copied())
        .collect();
    let events = dayseam_db::ActivityRepo::new(pool.clone())
        .get_many(&activity_ids)
        .await
        .expect("activity lookup");

    let kinds: std::collections::HashSet<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ActivityKind::ConfluencePageCreated),
        "first-version page should normalise to PageCreated: got {kinds:?}",
    );
    assert!(
        kinds.contains(&ActivityKind::ConfluenceComment),
        "self-authored comment should normalise to ConfluenceComment: got {kinds:?}",
    );

    let per_source = draft
        .per_source_state
        .get(&src.id)
        .expect("draft carries per-source state for the Confluence source");
    assert_eq!(
        per_source.status,
        dayseam_core::RunStatus::Succeeded,
        "per-source state must be Succeeded: {per_source:#?}",
    );
}

// ---- 3. Both-at-once scenario --------------------------------------------

#[tokio::test]
async fn orchestrator_generates_report_for_jira_plus_confluence_day() {
    // Two independent wiremock servers — one per Atlassian product.
    // The "shared Atlassian token" user experience is purely a
    // credential-layer concern; at the orchestrator layer each
    // `SourceHandle` carries its own `AuthStrategy` clone, and the
    // two muxes resolve their per-source config independently. This
    // test confirms that fanning those two muxes out concurrently
    // produces a single draft that holds evidence from both.
    let jira_server = MockServer::start().await;
    let conf_server = MockServer::start().await;
    mount_jira_jql(&jira_server, jira_happy_path_body()).await;
    mount_confluence_cql(&conf_server, confluence_happy_path_body()).await;

    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();

    let (jira_src, _jid, jira_handle) = seed_atlassian_source(
        &pool,
        &person,
        SourceKind::Jira,
        "https://acme.atlassian.net",
    )
    .await;
    let (conf_src, _cid, conf_handle) = seed_atlassian_source(
        &pool,
        &person,
        SourceKind::Confluence,
        "https://acme.atlassian.net",
    )
    .await;

    let jira_mux = build_jira_mux(jira_src.id, &wiremock_workspace(&jira_server));
    let conf_mux = build_confluence_mux(conf_src.id, &wiremock_workspace(&conf_server));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::Jira, jira_mux);
    connectors.insert(SourceKind::Confluence, conf_mux);

    let orch = build_orchestrator(pool.clone(), connectors, SinkRegistry::default());

    let request = GenerateRequest {
        person: person.clone(),
        sources: vec![jira_handle, conf_handle],
        date,
        template_id: DEV_EOD_TEMPLATE_ID.to_string(),
        template_version: "0.0.1".to_string(),
        verbose_mode: false,
    };

    let run = orch.generate_report(request).await;
    let outcome = run.completion.await.expect("join");
    assert_eq!(
        outcome.status,
        SyncRunStatus::Completed,
        "the combined Jira + Confluence day must end Completed: {outcome:#?}",
    );
    let draft_id = outcome.draft_id.expect("completed runs carry a draft id");

    let draft = dayseam_db::DraftRepo::new(pool.clone())
        .get(&draft_id)
        .await
        .expect("draft lookup")
        .expect("draft persisted");

    // Both sources must appear in per-source state as Completed. The
    // DOG-class regression this guards against: a partial fan-out
    // where one Atlassian connector swallows the other's error (the
    // Phase 3 `in_flight` serialisation bug).
    for src_id in [jira_src.id, conf_src.id] {
        let state = draft
            .per_source_state
            .get(&src_id)
            .unwrap_or_else(|| panic!("draft missing per-source state for {src_id}"));
        assert_eq!(
            state.status,
            dayseam_core::RunStatus::Succeeded,
            "per-source state for {src_id} must be Succeeded: {state:#?}",
        );
        assert!(
            state.error.is_none(),
            "per-source state for {src_id} carries no error on happy path: {:#?}",
            state.error,
        );
    }

    let activity_ids: Vec<Uuid> = draft
        .evidence
        .iter()
        .flat_map(|e| e.event_ids.iter().copied())
        .collect();
    let events = dayseam_db::ActivityRepo::new(pool.clone())
        .get_many(&activity_ids)
        .await
        .expect("activity lookup");

    let kinds: std::collections::HashSet<_> = events.iter().map(|e| e.kind).collect();
    // Both muxes contributed events to the same draft — the key
    // invariant for the combined day.
    assert!(
        kinds.contains(&ActivityKind::JiraIssueTransitioned)
            || kinds.contains(&ActivityKind::JiraIssueCommented),
        "combined day must carry at least one Jira event: got {kinds:?}",
    );
    assert!(
        kinds.contains(&ActivityKind::ConfluencePageCreated)
            || kinds.contains(&ActivityKind::ConfluenceComment),
        "combined day must carry at least one Confluence event: got {kinds:?}",
    );

    // Event → source provenance: at least one persisted event per
    // Atlassian source. Guards against a regression where a mux's
    // events silently get attributed to the other mux's source_id.
    let jira_events = events.iter().filter(|e| e.source_id == jira_src.id).count();
    let conf_events = events.iter().filter(|e| e.source_id == conf_src.id).count();
    assert!(
        jira_events >= 1,
        "expected at least 1 event attributed to the Jira source; got {jira_events}",
    );
    assert!(
        conf_events >= 1,
        "expected at least 1 event attributed to the Confluence source; got {conf_events}",
    );
}
