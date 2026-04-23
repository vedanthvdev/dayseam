//! Serde round-trip coverage for every public domain type.
//!
//! A mix of deterministic sample-based tests (easy to read, easy to diff
//! on failure) and one proptest on `ActivityEvent` with its most complex
//! field (`metadata: serde_json::Value`) so randomly generated values
//! can't silently drop through a serialize/deserialize cycle.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use dayseam_core::{
    error_codes, ActivityEvent, ActivityKind, Actor, Artifact, ArtifactId, ArtifactKind,
    ArtifactPayload, DayseamError, EntityKind, EntityRef, Evidence, Identity, Link, LocalRepo,
    LogEntry, LogEvent, LogLevel, PerSourceState, Person, Privacy, ProgressEvent, ProgressPhase,
    RawRef, RenderedBullet, RenderedSection, ReportDraft, RunId, RunStatus, SecretRef, Sink,
    SinkConfig, SinkKind, Source, SourceConfig, SourceHealth, SourceIdentity, SourceIdentityKind,
    SourceKind, SourceRunState, SyncRun, SyncRunCancelReason, SyncRunStatus, SyncRunTrigger,
    ToastEvent, ToastSeverity, WriteReceipt,
};
use proptest::prelude::*;
use uuid::Uuid;

fn sample_event() -> ActivityEvent {
    let source_id = Uuid::nil();
    ActivityEvent {
        id: ActivityEvent::deterministic_id(&source_id.to_string(), "!1234", "MrOpened"),
        source_id,
        external_id: "1234".into(),
        kind: ActivityKind::MrOpened,
        occurred_at: Utc.with_ymd_and_hms(2026, 4, 17, 9, 30, 0).unwrap(),
        actor: Actor {
            display_name: "Vedanth".into(),
            email: Some("v@example.com".into()),
            external_id: Some("42".into()),
        },
        title: "Add report engine".into(),
        body: Some("draft-first rendering".into()),
        links: vec![Link {
            url: "https://gitlab.example/acme/app/-/merge_requests/1234".into(),
            label: Some("!1234".into()),
        }],
        entities: vec![EntityRef {
            kind: EntityKind::Other("merge_request".into()),
            external_id: "1234".into(),
            label: None,
        }],
        parent_external_id: None,
        metadata: serde_json::json!({ "labels": ["feature", "needs-review"] }),
        raw_ref: RawRef {
            storage_key: "gitlab:mr:1234".into(),
            content_type: "application/json".into(),
        },
        privacy: Privacy::Normal,
    }
}

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(value, &back, "round-trip produced a different value");
}

#[test]
fn activity_event_round_trips() {
    round_trip(&sample_event());
}

#[test]
fn activity_kind_round_trips() {
    // Iterates over `ActivityKind::all()` rather than an inline list so
    // the test exhaustively covers every new variant the moment it's
    // added to the enum. DAY-73 introduces seven new variants; the
    // `all_activity_kinds_has_expected_count_and_is_unique` test in
    // `types::activity` already guards the slice itself.
    for k in ActivityKind::all() {
        round_trip(k);
    }
}

#[test]
fn source_round_trips() {
    let src = Source {
        id: Uuid::new_v4(),
        kind: SourceKind::GitLab,
        label: "gitlab.internal.acme.com".into(),
        config: SourceConfig::GitLab {
            base_url: "https://gitlab.internal.acme.com".into(),
            user_id: 42,
            username: "vedanth".into(),
        },
        secret_ref: Some(SecretRef {
            keychain_service: "dayseam.gitlab".into(),
            keychain_account: "gitlab.internal.acme.com".into(),
        }),
        created_at: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    round_trip(&src);

    let local = Source {
        id: Uuid::new_v4(),
        kind: SourceKind::LocalGit,
        label: "Work repos".into(),
        config: SourceConfig::LocalGit {
            scan_roots: vec![PathBuf::from("/Users/v/Code")],
        },
        secret_ref: None,
        created_at: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
        last_sync_at: None,
        last_health: SourceHealth::unchecked(),
    };
    round_trip(&local);
}

#[test]
fn identity_round_trips() {
    let id = Identity {
        id: Uuid::new_v4(),
        emails: vec!["v@example.com".into(), "v@work.example".into()],
        gitlab_user_ids: vec![42],
        display_name: "Vedanth".into(),
    };
    round_trip(&id);
}

#[test]
fn person_round_trips() {
    round_trip(&Person::new_self("Vedanth"));
    round_trip(&Person {
        id: Uuid::new_v4(),
        display_name: "Coworker".into(),
        is_self: false,
    });
}

#[test]
fn source_identity_round_trips_for_every_kind() {
    let person_id = Uuid::new_v4();
    let source_id = Some(Uuid::new_v4());
    for kind in [
        SourceIdentityKind::GitEmail,
        SourceIdentityKind::GitLabUserId,
        SourceIdentityKind::GitLabUsername,
        SourceIdentityKind::GitHubLogin,
        SourceIdentityKind::AtlassianAccountId,
    ] {
        round_trip(&SourceIdentity {
            id: Uuid::new_v4(),
            person_id,
            source_id,
            kind,
            external_actor_id: "42".into(),
        });
    }
}

#[test]
fn source_kind_round_trips_for_every_variant() {
    // DAY-73. The `sources.kind` column stores this value verbatim, so
    // round-tripping each variant is the contract that keeps v0.2
    // Atlassian rows readable on the next app launch.
    for kind in [
        SourceKind::GitLab,
        SourceKind::LocalGit,
        SourceKind::Jira,
        SourceKind::Confluence,
    ] {
        round_trip(&kind);
    }
}

#[test]
fn sink_round_trips() {
    let sink = Sink {
        id: Uuid::new_v4(),
        kind: SinkKind::MarkdownFile,
        label: "Obsidian vault".into(),
        config: SinkConfig::MarkdownFile {
            config_version: 1,
            dest_dirs: vec![
                PathBuf::from("/Users/v/notes/daily"),
                PathBuf::from("/Users/v/Documents/backup"),
            ],
            frontmatter: true,
        },
        created_at: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
        last_write_at: None,
    };
    round_trip(&sink);
}

#[test]
fn write_receipt_round_trips() {
    let receipt = WriteReceipt {
        run_id: Some(RunId::new()),
        sink_kind: SinkKind::MarkdownFile,
        destinations_written: vec![PathBuf::from("/Users/v/notes/daily/2026-04-17.md")],
        external_refs: Vec::new(),
        bytes_written: 4321,
        written_at: Utc.with_ymd_and_hms(2026, 4, 17, 18, 0, 5).unwrap(),
    };
    round_trip(&receipt);

    // Adhoc write (not dispatched through the orchestrator) — `run_id`
    // is `None` so the UI can distinguish a scheduled result from a
    // manual "Save as…" click.
    let adhoc = WriteReceipt {
        run_id: None,
        sink_kind: SinkKind::MarkdownFile,
        destinations_written: vec![PathBuf::from("/tmp/one-off.md")],
        external_refs: Vec::new(),
        bytes_written: 12,
        written_at: Utc.with_ymd_and_hms(2026, 4, 17, 18, 0, 5).unwrap(),
    };
    round_trip(&adhoc);
}

#[test]
fn local_repo_round_trips() {
    let repo = LocalRepo {
        path: PathBuf::from("/Users/v/Code/dayseam"),
        label: "dayseam".into(),
        is_private: false,
        discovered_at: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
    };
    round_trip(&repo);
}

#[test]
fn report_draft_round_trips() {
    let source_id = Uuid::new_v4();
    let mut per_source_state = HashMap::new();
    per_source_state.insert(
        source_id,
        SourceRunState {
            status: RunStatus::Succeeded,
            started_at: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
            finished_at: Some(Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 5).unwrap()),
            fetched_count: 12,
            error: None,
        },
    );
    let draft = ReportDraft {
        id: Uuid::new_v4(),
        date: chrono::NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
        template_id: "dev-eod".into(),
        template_version: "1".into(),
        sections: vec![RenderedSection {
            id: "completed".into(),
            title: "Completed".into(),
            bullets: vec![RenderedBullet {
                id: "b1".into(),
                text: "Merged !1234".into(),
                source_kind: Some(dayseam_core::SourceKind::GitLab),
            }],
        }],
        evidence: vec![Evidence {
            bullet_id: "b1".into(),
            event_ids: vec![Uuid::new_v4()],
            reason: "1 MR merged".into(),
        }],
        per_source_state,
        verbose_mode: false,
        generated_at: Utc.with_ymd_and_hms(2026, 4, 17, 18, 0, 0).unwrap(),
    };
    round_trip(&draft);
}

#[test]
fn log_entry_round_trips() {
    let entry = LogEntry {
        timestamp: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
        level: LogLevel::Info,
        source_id: None,
        message: "hello".into(),
    };
    round_trip(&entry);
}

#[test]
fn run_id_round_trips() {
    let rid = RunId::new();
    round_trip(&rid);
}

#[test]
fn progress_event_round_trips_for_every_phase() {
    let run_id = RunId::new();
    let source_id = Some(Uuid::new_v4());
    let emitted_at = Utc.with_ymd_and_hms(2026, 4, 17, 9, 30, 0).unwrap();
    let phases = vec![
        ProgressPhase::Starting {
            message: "Starting GitLab fetch".into(),
        },
        ProgressPhase::InProgress {
            completed: 3,
            total: Some(10),
            message: "3 / 10".into(),
        },
        ProgressPhase::InProgress {
            completed: 0,
            total: None,
            message: "Scanning repos…".into(),
        },
        ProgressPhase::Completed {
            message: "Done".into(),
        },
        ProgressPhase::Cancelled {
            message: "cancelled by user".into(),
        },
        ProgressPhase::Failed {
            code: error_codes::GITLAB_AUTH_INVALID_TOKEN.into(),
            message: "token expired".into(),
        },
    ];
    for phase in phases {
        round_trip(&ProgressEvent {
            run_id,
            source_id,
            phase,
            emitted_at,
        });
    }
}

#[test]
fn log_event_round_trips() {
    let event = LogEvent {
        run_id: Some(RunId::new()),
        source_id: Some(Uuid::new_v4()),
        level: LogLevel::Warn,
        message: "retrying after 429".into(),
        context: serde_json::json!({ "retry_after_secs": 30, "attempt": 2 }),
        emitted_at: Utc.with_ymd_and_hms(2026, 4, 17, 9, 30, 0).unwrap(),
    };
    round_trip(&event);

    let system_event = LogEvent {
        run_id: None,
        source_id: None,
        level: LogLevel::Info,
        message: "Dayseam started".into(),
        context: serde_json::Value::Null,
        emitted_at: Utc.with_ymd_and_hms(2026, 4, 17, 9, 0, 0).unwrap(),
    };
    round_trip(&system_event);
}

#[test]
fn toast_event_round_trips_for_every_severity() {
    let emitted_at = Utc.with_ymd_and_hms(2026, 4, 17, 9, 30, 0).unwrap();
    for severity in [
        ToastSeverity::Info,
        ToastSeverity::Success,
        ToastSeverity::Warning,
        ToastSeverity::Error,
    ] {
        round_trip(&ToastEvent {
            id: Uuid::new_v4(),
            severity,
            title: "Connected to GitLab".into(),
            body: Some("Found 3 projects you're a member of.".into()),
            emitted_at,
        });
    }
}

#[test]
fn dayseam_error_round_trips_for_every_variant() {
    let cases = vec![
        DayseamError::Auth {
            code: error_codes::GITLAB_AUTH_INVALID_TOKEN.into(),
            message: "expired token".into(),
            retryable: false,
            action_hint: Some("Create a new PAT with api scope".into()),
        },
        DayseamError::Network {
            code: error_codes::GITLAB_URL_DNS.into(),
            message: "no such host".into(),
        },
        DayseamError::RateLimited {
            code: error_codes::GITLAB_RATE_LIMITED.into(),
            retry_after_secs: 30,
        },
        DayseamError::UpstreamChanged {
            code: error_codes::GITLAB_UPSTREAM_SHAPE_CHANGED.into(),
            message: "missing field `iid`".into(),
        },
        DayseamError::InvalidConfig {
            code: "gitlab.config.bad_base_url".into(),
            message: "base_url must be https".into(),
        },
        DayseamError::Io {
            code: error_codes::SINK_FS_NOT_WRITABLE.into(),
            path: Some(PathBuf::from("/tmp/dayseam/report.md")),
            message: "permission denied".into(),
        },
        DayseamError::Internal {
            code: "core.internal.bug".into(),
            message: "unreachable".into(),
        },
        DayseamError::Cancelled {
            code: error_codes::RUN_CANCELLED_BY_USER.into(),
            message: "user pressed Cancel".into(),
        },
        DayseamError::Unsupported {
            code: error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST.into(),
            message: "local-git does not support Since(Checkpoint)".into(),
        },
    ];
    for case in cases {
        round_trip(&case);
    }
}

#[test]
fn artifact_id_round_trips() {
    round_trip(&ArtifactId::new());
    round_trip(&ArtifactId::deterministic(
        &Uuid::nil(),
        ArtifactKind::CommitSet,
        "/Users/v/Code/dayseam@2026-04-17",
    ));
}

#[test]
fn artifact_round_trips_for_every_payload() {
    let source_id = Uuid::nil();

    let commit_external_id = "/Users/v/Code/dayseam@2026-04-17".to_string();
    let commit_artifact = Artifact {
        id: ArtifactId::deterministic(&source_id, ArtifactKind::CommitSet, &commit_external_id),
        source_id,
        kind: ArtifactKind::CommitSet,
        external_id: commit_external_id,
        payload: ArtifactPayload::CommitSet {
            repo_path: PathBuf::from("/Users/v/Code/dayseam"),
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
            event_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            commit_shas: vec!["abc123".into(), "def456".into()],
        },
        created_at: Utc.with_ymd_and_hms(2026, 4, 17, 10, 0, 0).unwrap(),
    };
    round_trip(&commit_artifact);

    // DAY-73. The Jira walker in DAY-77 writes these artefacts for
    // every `(source_id, issue_key, date)` tuple; keeping the
    // round-trip test here makes the on-disk shape explicit before
    // the walker lands.
    let jira_external_id = "CAR-5117@2026-04-20".to_string();
    let jira_artifact = Artifact {
        id: ArtifactId::deterministic(&source_id, ArtifactKind::JiraIssue, &jira_external_id),
        source_id,
        kind: ArtifactKind::JiraIssue,
        external_id: jira_external_id,
        payload: ArtifactPayload::JiraIssue {
            issue_key: "CAR-5117".into(),
            project_key: "CAR".into(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            event_ids: vec![Uuid::new_v4()],
        },
        created_at: Utc.with_ymd_and_hms(2026, 4, 20, 18, 0, 0).unwrap(),
    };
    round_trip(&jira_artifact);

    let confluence_external_id = "123456789@2026-04-20".to_string();
    let confluence_artifact = Artifact {
        id: ArtifactId::deterministic(
            &source_id,
            ArtifactKind::ConfluencePage,
            &confluence_external_id,
        ),
        source_id,
        kind: ArtifactKind::ConfluencePage,
        external_id: confluence_external_id,
        payload: ArtifactPayload::ConfluencePage {
            page_id: "123456789".into(),
            space_key: "ENG".into(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
            event_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        },
        created_at: Utc.with_ymd_and_hms(2026, 4, 20, 18, 0, 0).unwrap(),
    };
    round_trip(&confluence_artifact);
}

#[test]
fn sync_run_trigger_round_trips() {
    round_trip(&SyncRunTrigger::User);
    round_trip(&SyncRunTrigger::Retry {
        previous_run_id: RunId::new(),
    });
}

#[test]
fn sync_run_cancel_reason_round_trips() {
    round_trip(&SyncRunCancelReason::User);
    round_trip(&SyncRunCancelReason::SupersededBy {
        run_id: RunId::new(),
    });
}

#[test]
fn sync_run_status_round_trips() {
    for status in [
        SyncRunStatus::Running,
        SyncRunStatus::Completed,
        SyncRunStatus::Cancelled,
        SyncRunStatus::Failed,
    ] {
        round_trip(&status);
    }
}

#[test]
fn sync_run_round_trips_with_per_source_state() {
    let started_at = Utc.with_ymd_and_hms(2026, 4, 17, 9, 30, 0).unwrap();
    let source_id = Uuid::new_v4();
    let per_source = vec![PerSourceState {
        source_id,
        status: RunStatus::Failed,
        started_at,
        finished_at: Some(started_at + chrono::Duration::seconds(4)),
        fetched_count: 2,
        error: Some(DayseamError::Network {
            code: error_codes::GITLAB_URL_DNS.into(),
            message: "no such host".into(),
        }),
    }];
    let previous = RunId::new();
    let run = SyncRun {
        id: RunId::new(),
        started_at,
        finished_at: Some(started_at + chrono::Duration::seconds(5)),
        trigger: SyncRunTrigger::Retry {
            previous_run_id: previous,
        },
        status: SyncRunStatus::Cancelled,
        cancel_reason: Some(SyncRunCancelReason::SupersededBy { run_id: previous }),
        superseded_by: Some(previous),
        per_source_state: per_source,
    };
    round_trip(&run);
}

proptest! {
    /// Random `ActivityEvent`s with structured metadata must round-trip
    /// through JSON. This covers the most variable field on the hottest
    /// type in the system.
    #[test]
    fn activity_event_round_trips_for_arbitrary_values(
        source_id_bytes in any::<[u8; 16]>(),
        external_id in "[A-Za-z0-9_\\-]{1,32}",
        title in "[^\\x00-\\x08\\x0b-\\x1f]{0,64}",
        fetched_count in 0u64..10_000,
        retryable in any::<bool>(),
    ) {
        let source_id = Uuid::from_bytes(source_id_bytes);
        let event = ActivityEvent {
            id: ActivityEvent::deterministic_id(&source_id.to_string(), &external_id, "CommitAuthored"),
            source_id,
            external_id: external_id.clone(),
            kind: ActivityKind::CommitAuthored,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 17, 9, 30, 0).unwrap(),
            actor: Actor {
                display_name: "Test".into(),
                email: None,
                external_id: None,
            },
            title,
            body: None,
            links: vec![],
            entities: vec![],
            parent_external_id: None,
            metadata: serde_json::json!({
                "fetched_count": fetched_count,
                "retryable": retryable,
                "nested": { "labels": ["a", "b"] },
            }),
            raw_ref: RawRef {
                storage_key: "test".into(),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ActivityEvent = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(event, back);
    }
}
