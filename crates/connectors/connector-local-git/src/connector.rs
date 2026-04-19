//! `SourceConnector` impl for local git.
//!
//! Responsibilities in order:
//!
//! 1. Discover repositories under the configured scan roots
//!    ([`crate::discovery::discover_repos`]).
//! 2. Walk each repo for the requested day
//!    ([`crate::walk::walk_repo_for_day`]), deduplicating commits by
//!    SHA and filtering by the caller's [`dayseam_core::SourceIdentity`]
//!    set.
//! 3. Apply the privacy rule
//!    ([`crate::privacy::redact_private_event`]) to commits from
//!    repos flagged private.
//! 4. Emit progress + structured log events throughout and bail
//!    out promptly on cancellation.
//! 5. Assemble a [`connectors_sdk::SyncResult`] with the events,
//!    one [`dayseam_core::Artifact::CommitSet`] per productive
//!    `(repo, day)` bucket, and aggregate stats.

use std::collections::HashSet;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};
use connectors_sdk::{ConnCtx, SourceConnector, SyncRequest, SyncResult, SyncStats};
use dayseam_core::{
    error_codes, DayseamError, LogLevel, ProgressPhase, SourceHealth, SourceIdentityKind,
    SourceKind,
};
use tracing::{debug, warn};

use crate::discovery::{discover_repos, DiscoveryConfig};
use crate::privacy::is_private_repo;
use crate::walk::walk_repo_for_day;

/// Local-git [`SourceConnector`] implementation.
#[derive(Debug, Clone)]
pub struct LocalGitConnector {
    scan_roots: Vec<PathBuf>,
    configured_private_roots: HashSet<PathBuf>,
    discovery: DiscoveryConfig,
    local_tz: FixedOffset,
}

impl LocalGitConnector {
    /// Build a connector from the configured scan roots. The
    /// orchestrator typically resolves `configured_private_roots`
    /// from the `local_repos` table; callers without that list
    /// (tests, CLI) can pass an empty set and rely on the
    /// `.dayseam/private` marker file instead.
    ///
    /// `local_tz` is the user's local timezone. Every commit is
    /// bucketed into a day in this timezone, so UTC offset changes
    /// (daylight saving, travel) are a caller concern — pass the
    /// timezone active *at walk time*.
    pub fn new(
        scan_roots: Vec<PathBuf>,
        configured_private_roots: HashSet<PathBuf>,
        local_tz: FixedOffset,
    ) -> Self {
        Self {
            scan_roots,
            configured_private_roots,
            discovery: DiscoveryConfig::default(),
            local_tz,
        }
    }

    /// Override the default discovery bounds. Tests use this to pin
    /// `max_roots` low enough to observe the truncation path.
    #[must_use]
    pub fn with_discovery(mut self, discovery: DiscoveryConfig) -> Self {
        self.discovery = discovery;
        self
    }
}

#[async_trait]
impl SourceConnector for LocalGitConnector {
    fn kind(&self) -> SourceKind {
        SourceKind::LocalGit
    }

    async fn healthcheck(&self, _ctx: &ConnCtx) -> Result<SourceHealth, DayseamError> {
        // Best-effort discovery pass. We report the count rather
        // than a pass/fail boolean so a zero-repo scan root still
        // surfaces as "configured but empty" in the UI.
        match discover_repos(&self.scan_roots, self.discovery) {
            Ok(_outcome) => Ok(SourceHealth {
                ok: true,
                checked_at: Some(Utc::now()),
                last_error: None,
            }),
            Err(err) => Ok(SourceHealth {
                ok: false,
                checked_at: Some(Utc::now()),
                last_error: Some(err),
            }),
        }
    }

    async fn sync(&self, ctx: &ConnCtx, request: SyncRequest) -> Result<SyncResult, DayseamError> {
        ctx.bail_if_cancelled()?;

        let day = match request {
            SyncRequest::Day(d) => d,
            SyncRequest::Range { .. } | SyncRequest::Since(_) => {
                return Err(DayseamError::Unsupported {
                    code: error_codes::CONNECTOR_UNSUPPORTED_SYNC_REQUEST.to_string(),
                    message:
                        "local-git v0.1 only services SyncRequest::Day; see Phase 2 plan §Task 2"
                            .to_string(),
                });
            }
        };

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Starting {
                message: format!("Discovering repos for {day}"),
            },
        );

        // Identity filter: every git email registered for this source
        // (or source-agnostic, i.e. `source_id = None`). Lower-cased
        // because git email comparisons are case-insensitive by
        // convention.
        let identity_emails: HashSet<String> = ctx
            .source_identities
            .iter()
            .filter(|si| matches!(si.kind, SourceIdentityKind::GitEmail))
            .filter(|si| si.source_id.is_none() || si.source_id == Some(ctx.source_id))
            .map(|si| si.external_actor_id.to_lowercase())
            .collect();

        let outcome = discover_repos(&self.scan_roots, self.discovery)?;
        if outcome.truncated {
            ctx.logs.send(
                LogLevel::Warn,
                Some(ctx.source_id),
                format!(
                    "scan truncated at max_roots = {}; raise the cap or narrow your scan roots",
                    self.discovery.max_roots
                ),
                serde_json::json!({
                    "code": error_codes::LOCAL_GIT_TOO_MANY_ROOTS,
                    "max_roots": self.discovery.max_roots,
                }),
            );
        }

        let total_repos = outcome.repos.len() as u32;
        let mut events = Vec::new();
        let mut artifacts = Vec::new();
        let mut filtered_by_identity: u64 = 0;
        let mut filtered_by_date: u64 = 0;

        for (idx, repo) in outcome.repos.iter().enumerate() {
            ctx.bail_if_cancelled()?;
            let completed = (idx as u32).saturating_add(1);
            ctx.progress.send(
                Some(ctx.source_id),
                ProgressPhase::InProgress {
                    completed,
                    total: Some(total_repos),
                    message: repo.label.clone(),
                },
            );

            let is_private = is_private_repo(
                &repo.path,
                self.configured_private_roots.contains(&repo.path),
            );
            let walk = match walk_repo_for_day(
                &ctx.source_id,
                &repo.path,
                day,
                self.local_tz,
                &identity_emails,
                is_private,
            ) {
                Ok(w) => w,
                Err(err) => {
                    // A single corrupt repo must not kill the whole
                    // sync; surface it as a log event and move on.
                    ctx.logs.send(
                        LogLevel::Error,
                        Some(ctx.source_id),
                        format!("failed to walk {}: {}", repo.path.display(), err),
                        serde_json::json!({
                            "code": err.code(),
                            "repo": repo.path.display().to_string(),
                        }),
                    );
                    continue;
                }
            };

            filtered_by_identity += walk.filtered_by_identity;
            filtered_by_date += walk.filtered_by_date;

            if walk.saw_missing_signature {
                ctx.logs.send(
                    LogLevel::Warn,
                    Some(ctx.source_id),
                    format!(
                        "{} has commits without an author email; their identity attribution was skipped",
                        repo.path.display()
                    ),
                    serde_json::json!({
                        "code": error_codes::LOCAL_GIT_NO_SIGNATURE,
                        "repo": repo.path.display().to_string(),
                    }),
                );
            }

            if is_private {
                ctx.logs.send(
                    LogLevel::Warn,
                    Some(ctx.source_id),
                    format!(
                        "{} is marked private; commit bodies were redacted from the report",
                        repo.path.display()
                    ),
                    serde_json::json!({
                        "repo": repo.path.display().to_string(),
                        "redacted_events": walk.events.len(),
                    }),
                );
            }

            debug!(
                repo = %repo.path.display(),
                commits = walk.events.len(),
                "walked repo",
            );

            ctx.logs.send(
                LogLevel::Info,
                Some(ctx.source_id),
                format!("walked {} ({} commits)", repo.label, walk.events.len()),
                serde_json::json!({
                    "repo": repo.path.display().to_string(),
                    "commits": walk.events.len(),
                }),
            );

            events.extend(walk.events);
            if let Some(a) = walk.artifact {
                artifacts.push(a);
            }
        }

        if filtered_by_identity > 0 {
            // The most common reason a commit today is yours but got
            // filtered: you authored-merged via GitHub / GitLab's
            // web UI, which rewrites the committer as a noreply
            // alias (`NNNN+user@users.noreply.github.com` on GitHub)
            // that isn't in your identity list. Surface the count
            // plus the copy-pasteable hint so the user has a clear
            // path to fixing their identity mapping from the log.
            // See `error_codes::LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY`
            // for the machine-readable code. DAY-52.
            ctx.logs.send(
                LogLevel::Warn,
                Some(ctx.source_id),
                format!(
                    "{filtered_by_identity} commit(s) on this day were authored or committed \
                     by email(s) not in your identity list. If those commits are yours — \
                     e.g. merge commits made through the GitHub / GitLab web UI that use a \
                     `NNNN+user@users.noreply.github.com` alias — add the email to your \
                     identity mapping under Settings → Identities to include them in \
                     reports."
                ),
                serde_json::json!({
                    "code": error_codes::LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY,
                    "count": filtered_by_identity,
                }),
            );
        }

        let stats = SyncStats {
            fetched_count: events.len() as u64,
            filtered_by_identity,
            filtered_by_date,
            http_retries: 0,
        };

        ctx.progress.send(
            Some(ctx.source_id),
            ProgressPhase::Completed {
                message: format!(
                    "local-git walked {} repo(s); {} event(s), {} artifact(s)",
                    total_repos,
                    events.len(),
                    artifacts.len()
                ),
            },
        );

        if outcome.truncated {
            warn!(
                max_roots = self.discovery.max_roots,
                "local-git discovery truncated",
            );
        }

        Ok(SyncResult {
            events,
            artifacts,
            checkpoint: None,
            stats,
            warnings: Vec::new(),
            raw_refs: Vec::new(),
        })
    }
}
