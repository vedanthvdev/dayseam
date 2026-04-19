//! A fully in-memory connector for tests and for the dev IPC demo.
//!
//! Compiled out of release builds unless the `mock` feature is enabled.
//! `MockConnector` deliberately exercises the full [`crate::SourceConnector`]
//! surface — healthcheck, sync, progress emission, cancellation
//! polling, identity filtering — so the SDK's invariants are
//! reflected in at least one passing implementation.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, DayseamError, EntityRef, Link, Privacy, ProgressPhase,
    RawRef, SourceHealth, SourceKind,
};
use uuid::Uuid;

use crate::{
    connector::SourceConnector,
    ctx::ConnCtx,
    sync::{SyncRequest, SyncResult, SyncStats},
};

/// Deterministic in-memory connector. The caller hands it a set of
/// fixture events at construction time; `sync` filters them by
/// `ctx.source_identities` and the request window, emits the usual
/// progress phases, and returns the filtered subset.
#[derive(Debug, Clone)]
pub struct MockConnector {
    kind: SourceKind,
    fixtures: Vec<ActivityEvent>,
}

impl MockConnector {
    pub fn new(kind: SourceKind, fixtures: Vec<ActivityEvent>) -> Self {
        Self { kind, fixtures }
    }

    /// Build a simple fixture event pinned to `occurred_at`. Connectors
    /// tests use this to seed `MockConnector` with timestamps aligned
    /// to a requested `SyncRequest::Day`.
    pub fn fixture_event(
        source_id: Uuid,
        external_id: impl Into<String>,
        actor_email: &str,
        occurred_at: DateTime<Utc>,
    ) -> ActivityEvent {
        let external_id = external_id.into();
        ActivityEvent {
            id: ActivityEvent::deterministic_id(&source_id.to_string(), &external_id, "MrOpened"),
            source_id,
            external_id: external_id.clone(),
            kind: ActivityKind::MrOpened,
            occurred_at,
            actor: Actor {
                display_name: actor_email.to_string(),
                email: Some(actor_email.to_string()),
                external_id: None,
            },
            title: format!("fixture {external_id}"),
            body: None,
            links: vec![Link {
                url: format!("https://mock.example/{external_id}"),
                label: None,
            }],
            entities: vec![EntityRef {
                kind: "merge_request".into(),
                external_id: external_id.clone(),
                label: None,
            }],
            parent_external_id: None,
            metadata: serde_json::json!({}),
            raw_ref: RawRef {
                storage_key: format!("mock:mr:{external_id}"),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }
}

#[async_trait]
impl SourceConnector for MockConnector {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    async fn healthcheck(&self, _ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        Ok(SourceHealth {
            ok: true,
            checked_at: Some(Utc::now()),
            last_error: None,
        })
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        ctx.bail_if_cancelled()?;

        // Reject unsupported request shapes up front so an empty
        // fixture set still surfaces the error — a missing early
        // return here would make the mock silently "succeed" with zero
        // events, hiding the orchestrator fallback path from tests.
        if matches!(request, SyncRequest::Since(_)) {
            return Err(DayseamError::Unsupported {
                code: dayseam_core::error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST.to_string(),
                message: "MockConnector does not support Since(Checkpoint)".to_string(),
            });
        }

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Starting {
                message: "Mock connector starting".into(),
            },
        );

        let allowed_actors: std::collections::HashSet<&str> = ctx
            .source_identities
            .iter()
            .map(|si| si.external_actor_id.as_str())
            .collect();

        let mut fetched: Vec<ActivityEvent> = Vec::new();
        let mut filtered_by_identity: u64 = 0;
        let mut filtered_by_date: u64 = 0;

        let total = self.fixtures.len() as u32;
        for (idx, event) in self.fixtures.iter().enumerate() {
            ctx.bail_if_cancelled()?;

            let in_window = match &request {
                SyncRequest::Day(day) => event.occurred_at.date_naive() == *day,
                SyncRequest::Range { start, end } => {
                    let d = event.occurred_at.date_naive();
                    &d >= start && &d <= end
                }
                SyncRequest::Since(_) => unreachable!("filtered above"),
            };

            if !in_window {
                filtered_by_date += 1;
                continue;
            }

            let actor_matches = event
                .actor
                .email
                .as_deref()
                .map(|e| allowed_actors.contains(e))
                .unwrap_or(false);

            if !actor_matches {
                filtered_by_identity += 1;
                continue;
            }

            fetched.push(event.clone());
            let completed = (idx as u32).saturating_add(1);
            ctx.progress.send(
                Some(ctx.source_id),
                ProgressPhase::InProgress {
                    completed,
                    total: Some(total),
                    message: format!("{completed}/{total}"),
                },
            );
        }

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Completed {
                message: format!("Mock connector fetched {} events", fetched.len()),
            },
        );

        let stats = SyncStats {
            fetched_count: fetched.len() as u64,
            filtered_by_identity,
            filtered_by_date,
            http_retries: 0,
        };
        Ok(SyncResult {
            events: fetched,
            artifacts: Vec::new(),
            checkpoint: None,
            stats,
            warnings: Vec::new(),
            raw_refs: Vec::new(),
        })
    }
}
