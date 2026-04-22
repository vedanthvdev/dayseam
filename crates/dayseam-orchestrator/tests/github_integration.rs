//! DAY-100 — orchestrator-level GitHub integration tests.
//!
//! Complements the per-connector wiremock tests in
//! [`connector-github/tests/walk.rs`] by driving the full
//! orchestrator stack end-to-end:
//!
//!   `GenerateRequest` → `Orchestrator::generate_report` →
//!   fan-out → `GithubMux` → real `reqwest` call →
//!   `wiremock::MockServer` → walker normalisation → dedup → render →
//!   persisted `ReportDraft`.
//!
//! The per-connector tests pin the walker's contract in isolation;
//! the tests here pin the *wiring* of that walker into the
//! orchestrator's fan-out, mirroring the DAY-90 Atlassian integration
//! shape one-for-one.
//!
//! Two scenarios exercise the matrix that matters for the
//! v0.3 → v0.4 upgrade path:
//!
//! 1. **GitHub-only day.** A single GitHub source with one
//!    self-authored PR-opened event surviving the day-window filter
//!    — ensures the GitHub mux hydrates a live connector at boot and
//!    the orchestrator routes the `SourceKind::GitHub` fan-out through
//!    it. Also pins that the events+search two-endpoint walker
//!    composes cleanly when the search stream is empty.
//! 2. **GitHub + GitLab day.** One GitHub source + one GitLab source
//!    in the same `GenerateRequest`, each with its own `MockServer`.
//!    Both fan out concurrently and contribute events to the same
//!    persisted draft. This is the "I connected both Git hosts to
//!    Dayseam" case from a user's perspective; at the orchestrator
//!    layer it confirms that the two connectors do not stomp on each
//!    other's per-source state (the same subtle regression the Phase
//!    3 multi-source dedup test caught for GitLab + LocalGit and the
//!    DAY-90 Atlassian scenarios caught for Jira + Confluence).

#![allow(clippy::too_many_lines)]

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use chrono::FixedOffset;
use common::{build_orchestrator, fixture_date, test_person, test_pool};
use connector_github::{GithubConfig, GithubMux, GithubSourceCfg};
use connector_gitlab::{GitlabMux, GitlabSourceCfg};
use connectors_sdk::{AuthStrategy, PatAuth};
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
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- Constants ------------------------------------------------------------

/// The self-user id attached to every GitHub source in this file.
/// Matches the DAY-96 walker fixtures so the `actor.id` self-filter
/// keeps a recognisable shape; the value itself is arbitrary.
const SELF_GITHUB_USER_ID: i64 = 17;
const SELF_GITHUB_LOGIN: &str = "vedanth";

/// The self-user id attached to the GitLab source in the combined
/// scenario. Deliberately different from [`SELF_GITHUB_USER_ID`] so
/// that a per-source event attribution regression (GitHub events
/// leaking into the GitLab source's state or vice versa) would not
/// pass by coincidence of shared ids.
const SELF_GITLAB_USER_ID: i64 = 4242;

// ---- Scaffolding helpers -------------------------------------------------

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).expect("UTC offset")
}

fn github_auth() -> Arc<dyn AuthStrategy> {
    Arc::new(PatAuth::github(
        "ghp-test-token",
        "dayseam.github",
        "source:integration-test",
    ))
}

fn gitlab_auth() -> Arc<dyn AuthStrategy> {
    Arc::new(PatAuth::gitlab("glpat-test", "dayseam.gitlab", "acme"))
}

/// Seed a GitHub source + its `GitHubUserId` / `GitHubLogin` identity
/// pair into the DB and return the [`SourceHandle`] the orchestrator
/// fans out to. The walker keys off the numeric `user_id` for the
/// self-filter and off the `login` for `/users/:login/events` URL
/// composition, so both identities must be present for the walk to
/// issue a request.
async fn seed_github_source(
    pool: &SqlitePool,
    person: &Person,
    api_base_url_in_db: &str,
) -> (Source, Vec<SourceIdentity>, SourceHandle) {
    let source = Source {
        id: Uuid::new_v4(),
        kind: SourceKind::GitHub,
        label: "github fixture".into(),
        config: SourceConfig::GitHub {
            api_base_url: api_base_url_in_db.to_string(),
        },
        secret_ref: None,
        created_at: chrono::Utc::now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    dayseam_db::SourceRepo::new(pool.clone())
        .insert(&source)
        .await
        .expect("seed github source");

    let user_id_identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: person.id,
        source_id: Some(source.id),
        kind: SourceIdentityKind::GitHubUserId,
        external_actor_id: SELF_GITHUB_USER_ID.to_string(),
    };
    let login_identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: person.id,
        source_id: Some(source.id),
        kind: SourceIdentityKind::GitHubLogin,
        external_actor_id: SELF_GITHUB_LOGIN.to_string(),
    };
    let repo = dayseam_db::SourceIdentityRepo::new(pool.clone());
    repo.insert(&user_id_identity)
        .await
        .expect("seed github user-id identity");
    repo.insert(&login_identity)
        .await
        .expect("seed github login identity");

    let identities = vec![user_id_identity, login_identity];
    let handle = SourceHandle {
        source_id: source.id,
        kind: SourceKind::GitHub,
        auth: github_auth(),
        source_identities: identities.clone(),
    };
    (source, identities, handle)
}

/// Parallel to [`seed_github_source`] for GitLab, used only by the
/// combined scenario. The DB row stores whatever `base_url` the
/// caller hands in; the live mux is built separately below and
/// points at the scenario's `MockServer`.
async fn seed_gitlab_source(
    pool: &SqlitePool,
    person: &Person,
    base_url_in_db: &str,
) -> (Source, SourceIdentity, SourceHandle) {
    let source = Source {
        id: Uuid::new_v4(),
        kind: SourceKind::GitLab,
        label: "gitlab fixture".into(),
        config: SourceConfig::GitLab {
            base_url: base_url_in_db.to_string(),
            user_id: SELF_GITLAB_USER_ID,
            username: "vedanth-gitlab".into(),
        },
        secret_ref: None,
        created_at: chrono::Utc::now(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    dayseam_db::SourceRepo::new(pool.clone())
        .insert(&source)
        .await
        .expect("seed gitlab source");

    let identity = SourceIdentity {
        id: Uuid::new_v4(),
        person_id: person.id,
        source_id: Some(source.id),
        kind: SourceIdentityKind::GitLabUserId,
        external_actor_id: SELF_GITLAB_USER_ID.to_string(),
    };
    dayseam_db::SourceIdentityRepo::new(pool.clone())
        .insert(&identity)
        .await
        .expect("seed gitlab identity");

    let handle = SourceHandle {
        source_id: source.id,
        kind: SourceKind::GitLab,
        auth: gitlab_auth(),
        source_identities: vec![identity.clone()],
    };
    (source, identity, handle)
}

/// Build a `GithubMux` pre-populated with one source pointing at
/// `server_url`. The mux carries `local_tz = UTC` so the
/// `SyncRequest::Day` window the orchestrator computes lines up with
/// the wiremock fixtures' UTC timestamps.
fn build_github_mux(source_id: SourceId, server_url: &url::Url) -> Arc<GithubMux> {
    let config = GithubConfig::from_raw(server_url.as_str()).expect("GithubConfig parses mock URL");
    Arc::new(GithubMux::new(
        utc(),
        [GithubSourceCfg { source_id, config }],
    ))
}

fn build_gitlab_mux(source_id: SourceId, server_url: &url::Url) -> Arc<GitlabMux> {
    Arc::new(GitlabMux::new(
        utc(),
        [GitlabSourceCfg {
            source_id,
            base_url: server_url.as_str().trim_end_matches('/').to_string(),
            user_id: SELF_GITLAB_USER_ID,
        }],
    ))
}

/// The walker needs the API base URL to carry a trailing slash so
/// `Url::join("users/:login/events")` does not drop the last path
/// segment. `MockServer::uri()` never emits one.
fn wiremock_api_base(server: &MockServer) -> url::Url {
    url::Url::parse(&format!("{}/", server.uri())).expect("parse mockserver uri")
}

// ---- Mock mounting helpers -----------------------------------------------

/// Mount the `/users/:login/events` endpoint with `body`. The walker
/// paginates via the `Link` header; omitting the header ends the walk
/// after the first page, which is exactly what every fixture here
/// wants.
async fn mount_github_events(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/users/{SELF_GITHUB_LOGIN}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Mount an always-empty `/search/issues` stub. The GitHub walker
/// hits both `/users/:login/events` and `/search/issues` in every
/// walk; an unmocked search call would return a 404 from wiremock
/// and fail the whole walk.
async fn mount_empty_github_search(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 0,
            "incomplete_results": false,
            "items": []
        })))
        .mount(server)
        .await;
}

async fn mount_gitlab_events(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{SELF_GITLAB_USER_ID}/events")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

// ---- Fixture builders ----------------------------------------------------

/// One self-authored `PullRequestEvent` on [`fixture_date`] at 10:00
/// UTC. Shape mirrors the DAY-96 walker fixtures exactly so the
/// normaliser's parsing stays covered by two independent tests.
fn github_pr_opened_body() -> Value {
    json!([{
        "id": "evt-pr-1",
        "type": "PullRequestEvent",
        "actor": {
            "id": SELF_GITHUB_USER_ID,
            "login": SELF_GITHUB_LOGIN,
            "display_login": SELF_GITHUB_LOGIN
        },
        "repo": {
            "id": 1,
            "name": "modulr/foo",
            "url": "https://api.github.com/repos/modulr/foo"
        },
        "created_at": "2026-04-18T10:00:00Z",
        "payload": {
            "action": "opened",
            "number": 42,
            "pull_request": {
                "id": 4200,
                "number": 42,
                "title": "Orchestrator-level GitHub PR",
                "state": "open",
                "html_url": "https://github.com/modulr/foo/pull/42",
                "user": {
                    "id": SELF_GITHUB_USER_ID,
                    "login": SELF_GITHUB_LOGIN
                }
            }
        }
    }])
}

/// One self-authored `opened` MR event on [`fixture_date`] at
/// 11:00 UTC. Shape mirrors the DAY-86 walker fixture exactly.
fn gitlab_mr_opened_body() -> Value {
    json!([{
        "id": 2001,
        "action_name": "opened",
        "target_type": "MergeRequest",
        "target_iid": 11,
        "target_id": 2001,
        "target_title": "Combined-day GitLab MR",
        "project_id": 42,
        "created_at": "2026-04-18T11:00:00.000Z",
        "author_id": SELF_GITLAB_USER_ID,
        "author": { "id": SELF_GITLAB_USER_ID, "username": "vedanth-gitlab" }
    }])
}

// ---- 1. GitHub-only scenario ---------------------------------------------

#[tokio::test]
async fn orchestrator_generates_report_for_github_only_day() {
    let server = MockServer::start().await;
    mount_github_events(&server, github_pr_opened_body()).await;
    mount_empty_github_search(&server).await;

    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();

    // The DB row carries the canonical GitHub cloud URL; the live
    // mux below is what actually points at the wiremock. The
    // orchestrator never re-reads `sources.config` during a run,
    // so this decoupling is legitimate.
    let (src, _ids, handle) = seed_github_source(&pool, &person, "https://api.github.com/").await;
    let github_mux = build_github_mux(src.id, &wiremock_api_base(&server));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::GitHub, github_mux);

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
        "the GitHub-only day must end Completed: {outcome:#?}",
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
        kinds.contains(&ActivityKind::GitHubPullRequestOpened),
        "the self-authored PR-opened event must reach the draft: got {kinds:?}",
    );

    // Per-source state must mark the GitHub source as Succeeded so
    // the UI's per-source badges light up green, not yellow — the
    // DOG-class silent-failure this scenario guards against is a
    // connector that returns zero events on a happy day and the
    // orchestrator still reporting Succeeded (or, worse, a connector
    // that errors and the orchestrator still reporting Succeeded).
    let per_source = draft
        .per_source_state
        .get(&src.id)
        .expect("draft carries per-source state for the GitHub source");
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

// ---- 2. GitHub + GitLab scenario -----------------------------------------

#[tokio::test]
async fn orchestrator_generates_report_for_github_plus_gitlab_day() {
    // Two independent wiremock servers — one per Git host. The
    // "shared identity" user experience is purely a presentation-
    // layer concern; at the orchestrator layer each `SourceHandle`
    // carries its own `AuthStrategy` clone, and the two muxes resolve
    // their per-source config independently. This test confirms that
    // fanning those two muxes out concurrently produces a single
    // draft that holds evidence from both.
    let gh_server = MockServer::start().await;
    let gl_server = MockServer::start().await;
    mount_github_events(&gh_server, github_pr_opened_body()).await;
    mount_empty_github_search(&gh_server).await;
    mount_gitlab_events(&gl_server, gitlab_mr_opened_body()).await;

    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();

    let (gh_src, _gh_ids, gh_handle) =
        seed_github_source(&pool, &person, "https://api.github.com/").await;
    let (gl_src, _gl_id, gl_handle) =
        seed_gitlab_source(&pool, &person, "https://gitlab.com").await;

    let gh_mux = build_github_mux(gh_src.id, &wiremock_api_base(&gh_server));
    let gl_mux = build_gitlab_mux(gl_src.id, &wiremock_api_base(&gl_server));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::GitHub, gh_mux);
    connectors.insert(SourceKind::GitLab, gl_mux);

    let orch = build_orchestrator(pool.clone(), connectors, SinkRegistry::default());

    let request = GenerateRequest {
        person: person.clone(),
        sources: vec![gh_handle, gl_handle],
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
        "the combined GitHub + GitLab day must end Completed: {outcome:#?}",
    );
    let draft_id = outcome.draft_id.expect("completed runs carry a draft id");

    let draft = dayseam_db::DraftRepo::new(pool.clone())
        .get(&draft_id)
        .await
        .expect("draft lookup")
        .expect("draft persisted");

    // Both sources must appear in per-source state as Succeeded. The
    // DOG-class regression this guards against: a partial fan-out
    // where one connector swallows the other's error (the Phase 3
    // `in_flight` serialisation bug).
    for src_id in [gh_src.id, gl_src.id] {
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
        kinds.contains(&ActivityKind::GitHubPullRequestOpened),
        "combined day must carry the self-authored GitHub PR-opened event: got {kinds:?}",
    );
    assert!(
        kinds.contains(&ActivityKind::MrOpened),
        "combined day must carry the self-authored GitLab MR-opened event: got {kinds:?}",
    );

    // Event → source provenance: at least one persisted event per
    // Git host. Guards against a regression where a mux's events
    // silently get attributed to the other mux's source_id (the
    // registry-level bug DAY-95's `registry_kind_round_trips_for_every_registered_connector`
    // catches at build time, pinned again here at run time).
    let gh_events = events.iter().filter(|e| e.source_id == gh_src.id).count();
    let gl_events = events.iter().filter(|e| e.source_id == gl_src.id).count();
    assert!(
        gh_events >= 1,
        "expected at least 1 event attributed to the GitHub source; got {gh_events}",
    );
    assert!(
        gl_events >= 1,
        "expected at least 1 event attributed to the GitLab source; got {gl_events}",
    );
}
