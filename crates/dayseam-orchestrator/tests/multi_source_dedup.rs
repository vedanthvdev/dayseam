//! Phase 3 Task 2 integration test: two sources emitting the same
//! commit SHA render as one bullet, and the surviving event is
//! stamped with the MR iid when the MR metadata claims that SHA.
//!
//! The assertion surface is the persisted draft (post-render,
//! post-persist): one bullet per SHA, the evidence hydrates to the
//! canonical surviving event, and the `parent_external_id` is set.

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use common::{build_orchestrator, fixture_date, seed_source, test_person, test_pool};
use connectors_sdk::MockConnector;
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, EntityRef, Link, Privacy, RawRef, SourceId, SourceKind,
    SyncRunStatus,
};
use dayseam_orchestrator::{orchestrator::GenerateRequest, ConnectorRegistry, SinkRegistry};
use dayseam_report::DEV_EOD_TEMPLATE_ID;
use uuid::Uuid;

fn commit_authored(
    source_id: SourceId,
    sha: &str,
    actor_email: &str,
    d: NaiveDate,
    body: Option<&str>,
) -> ActivityEvent {
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&source_id.to_string(), sha, "CommitAuthored"),
        source_id,
        external_id: sha.into(),
        kind: ActivityKind::CommitAuthored,
        occurred_at: Utc.from_utc_datetime(&d.and_hms_opt(9, 0, 0).expect("valid hms")),
        actor: Actor {
            display_name: actor_email.into(),
            email: Some(actor_email.into()),
            external_id: None,
        },
        title: format!("commit {sha}"),
        body: body.map(str::to_string),
        links: vec![Link {
            url: format!("https://mock.example/commit/{sha}"),
            label: None,
        }],
        entities: vec![EntityRef {
            kind: "repo".into(),
            external_id: "/work/dayseam".into(),
            label: None,
        }],
        parent_external_id: None,
        metadata: serde_json::Value::Null,
        raw_ref: RawRef {
            storage_key: format!("k:{sha}"),
            content_type: "application/x-git-commit".into(),
        },
        privacy: Privacy::Normal,
    }
}

fn mr_event(
    source_id: SourceId,
    iid: &str,
    actor_email: &str,
    d: NaiveDate,
    commit_shas: &[&str],
) -> ActivityEvent {
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&source_id.to_string(), iid, "MrOpened"),
        source_id,
        external_id: iid.into(),
        kind: ActivityKind::MrOpened,
        occurred_at: Utc.from_utc_datetime(&d.and_hms_opt(10, 0, 0).expect("valid hms")),
        actor: Actor {
            display_name: actor_email.into(),
            email: Some(actor_email.into()),
            external_id: None,
        },
        title: format!("Opened MR: {iid}"),
        body: Some("Review please".into()),
        links: Vec::new(),
        entities: vec![EntityRef {
            kind: "merge_request".into(),
            external_id: iid.into(),
            label: None,
        }],
        parent_external_id: None,
        metadata: serde_json::json!({
            "commit_shas": commit_shas,
        }),
        raw_ref: RawRef {
            storage_key: format!("k:{iid}"),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

#[tokio::test]
async fn two_sources_emitting_same_sha_render_as_one_bullet_and_rolled_into_mr() {
    let (pool, _tmp) = test_pool().await;
    let person = test_person();
    let date = fixture_date();
    let email = "dev@example.com";

    // Two sources — one `LocalGit`, one `GitLab` — whose identity
    // lists both claim `dev@example.com` so the author filter lets
    // both events through. The orchestrator's fan-out hands each
    // `MockConnector` its own request; the shared SHA creates the
    // cross-source collision the dedup pass exists for.
    let (src_local, _id_l, handle_local) =
        seed_source(&pool, &person, SourceKind::LocalGit, "git fixture", email).await;
    let (src_gitlab, _id_g, handle_gitlab) =
        seed_source(&pool, &person, SourceKind::GitLab, "gitlab fixture", email).await;

    let shared_sha = "0123456789abcdef0123456789abcdef01234567";
    let local_only_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    // Local-git emits one row per commit with a minimal body.
    let local_shared = commit_authored(src_local.id, shared_sha, email, date, None);
    let local_only = commit_authored(src_local.id, local_only_sha, email, date, None);

    // GitLab's summary push event shares the tip SHA and carries a
    // longer body (the commit message GitLab returned). That longer
    // body makes it the dedup survivor.
    let gitlab_shared = commit_authored(
        src_gitlab.id,
        shared_sha,
        email,
        date,
        Some("GitLab-enriched commit message with context"),
    );

    // MR whose commit list claims the shared SHA. The orchestrator's
    // MR-rollup pass stamps the surviving `CommitAuthored` with
    // `!42` as its `parent_external_id`.
    let mr = mr_event(src_gitlab.id, "!42", email, date, &[shared_sha]);

    let conn_local = Arc::new(MockConnector::new(
        SourceKind::LocalGit,
        vec![local_shared, local_only],
    ));
    let conn_gitlab = Arc::new(MockConnector::new(
        SourceKind::GitLab,
        vec![gitlab_shared, mr],
    ));

    let mut connectors = ConnectorRegistry::default();
    connectors.insert(SourceKind::LocalGit, conn_local);
    connectors.insert(SourceKind::GitLab, conn_gitlab);

    let orch = build_orchestrator(pool.clone(), connectors, SinkRegistry::default());

    let request = GenerateRequest {
        person: person.clone(),
        sources: vec![handle_local, handle_gitlab],
        date,
        template_id: DEV_EOD_TEMPLATE_ID.to_string(),
        template_version: "0.0.1".to_string(),
        verbose_mode: false,
    };

    let handle = orch.generate_report(request).await;
    let outcome = handle.completion.await.expect("join");
    assert_eq!(outcome.status, SyncRunStatus::Completed);
    let draft_id = outcome.draft_id.expect("completed runs carry a draft id");

    // Pull the persisted `activity_events` that back the draft's
    // evidence. Dedup ran before insert, so exactly one row per SHA
    // should have been written.
    let draft = dayseam_db::DraftRepo::new(pool.clone())
        .get(&draft_id)
        .await
        .expect("drafts lookup")
        .expect("draft persisted");
    let activity_ids: Vec<Uuid> = draft
        .evidence
        .iter()
        .flat_map(|e| e.event_ids.iter().copied())
        .collect();
    let events = dayseam_db::ActivityRepo::new(pool.clone())
        .get_many(&activity_ids)
        .await
        .expect("activity_events lookup");

    let commit_events: Vec<&ActivityEvent> = events
        .iter()
        .filter(|e| e.kind == ActivityKind::CommitAuthored)
        .collect();
    let shas: std::collections::BTreeSet<&str> = commit_events
        .iter()
        .map(|e| e.external_id.as_str())
        .collect();
    assert_eq!(
        shas.len(),
        2,
        "two distinct SHAs (one shared, one local-only) survive dedup; got {shas:?}"
    );
    assert!(
        shas.contains(shared_sha),
        "shared SHA must survive dedup as a single row"
    );
    assert!(
        shas.contains(local_only_sha),
        "the local-only SHA is not affected by dedup"
    );

    // The shared-SHA survivor is the GitLab row (richer body) and
    // carries the MR iid as its parent_external_id from the rollup.
    let survivor = commit_events
        .iter()
        .find(|e| e.external_id == shared_sha)
        .expect("shared SHA present");
    assert_eq!(
        survivor.body.as_deref(),
        Some("GitLab-enriched commit message with context"),
        "dedup kept the richer GitLab body"
    );
    assert_eq!(
        survivor.parent_external_id.as_deref(),
        Some("!42"),
        "rollup stamped the MR iid on the shared SHA"
    );

    // The local-only commit is untouched by rollup.
    let local_event = commit_events
        .iter()
        .find(|e| e.external_id == local_only_sha)
        .expect("local-only SHA present");
    assert_eq!(
        local_event.parent_external_id, None,
        "a commit not in any MR stays unparented"
    );
}
