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
    error_codes, ActivityEvent, ActivityKind, Actor, DayseamError, EntityRef, Evidence, Identity,
    Link, LocalRepo, LogEntry, LogEvent, LogLevel, Privacy, ProgressEvent, ProgressPhase, RawRef,
    RenderedBullet, RenderedSection, ReportDraft, RunId, RunStatus, SecretRef, Source,
    SourceConfig, SourceHealth, SourceKind, SourceRunState, ToastEvent, ToastSeverity,
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
            kind: "merge_request".into(),
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
    for k in [
        ActivityKind::CommitAuthored,
        ActivityKind::MrOpened,
        ActivityKind::MrMerged,
        ActivityKind::MrClosed,
        ActivityKind::MrReviewComment,
        ActivityKind::MrApproved,
        ActivityKind::IssueOpened,
        ActivityKind::IssueClosed,
        ActivityKind::IssueComment,
    ] {
        round_trip(&k);
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
    ];
    for case in cases {
        round_trip(&case);
    }
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
