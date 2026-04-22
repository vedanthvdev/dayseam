//! Internal helpers for converting between Rust enums and the short
//! string representations we store in columns that are effectively
//! discriminators (`sources.kind`, `activity_events.kind`,
//! `activity_events.privacy`, `log_entries.level`). JSON-blob columns go
//! through `serde_json` directly — this file only covers the short
//! single-token cases.
//!
//! Keeping these conversions here (rather than in `dayseam-core`) means
//! the core types stay free of storage concerns.

use chrono::{DateTime, Utc};
use dayseam_core::{
    ActivityKind, ArtifactKind, LogLevel, Privacy, SinkKind, SourceIdentityKind, SourceKind,
    SyncRunStatus,
};

use crate::error::DbError;

/// Parse an ISO-8601 / RFC-3339 timestamp pulled out of a TEXT column.
/// Wraps parse errors in `DbError::InvalidData` so the column name is
/// visible in the error message.
pub(crate) fn parse_rfc3339(s: &str, column: &str) -> Result<DateTime<Utc>, DbError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| DbError::InvalidData {
            column: column.into(),
            message: e.to_string(),
        })
}

pub(crate) fn source_kind_to_db(k: &SourceKind) -> &'static str {
    match k {
        SourceKind::GitLab => "GitLab",
        SourceKind::LocalGit => "LocalGit",
        SourceKind::Jira => "Jira",
        SourceKind::Confluence => "Confluence",
    }
}

pub(crate) fn source_kind_from_db(s: &str) -> Result<SourceKind, DbError> {
    match s {
        "GitLab" => Ok(SourceKind::GitLab),
        "LocalGit" => Ok(SourceKind::LocalGit),
        "Jira" => Ok(SourceKind::Jira),
        "Confluence" => Ok(SourceKind::Confluence),
        other => Err(DbError::InvalidData {
            column: "sources.kind".into(),
            message: format!("unknown SourceKind `{other}`"),
        }),
    }
}

pub(crate) fn activity_kind_to_db(k: &ActivityKind) -> &'static str {
    match k {
        ActivityKind::CommitAuthored => "CommitAuthored",
        ActivityKind::MrOpened => "MrOpened",
        ActivityKind::MrMerged => "MrMerged",
        ActivityKind::MrClosed => "MrClosed",
        ActivityKind::MrReviewComment => "MrReviewComment",
        ActivityKind::MrApproved => "MrApproved",
        ActivityKind::IssueOpened => "IssueOpened",
        ActivityKind::IssueClosed => "IssueClosed",
        ActivityKind::IssueComment => "IssueComment",
        ActivityKind::JiraIssueTransitioned => "JiraIssueTransitioned",
        ActivityKind::JiraIssueCommented => "JiraIssueCommented",
        ActivityKind::JiraIssueAssigned => "JiraIssueAssigned",
        ActivityKind::JiraIssueUnassigned => "JiraIssueUnassigned",
        ActivityKind::JiraIssueCreated => "JiraIssueCreated",
        ActivityKind::ConfluencePageCreated => "ConfluencePageCreated",
        ActivityKind::ConfluencePageEdited => "ConfluencePageEdited",
        ActivityKind::ConfluenceComment => "ConfluenceComment",
    }
}

pub(crate) fn activity_kind_from_db(s: &str) -> Result<ActivityKind, DbError> {
    let kind = match s {
        "CommitAuthored" => ActivityKind::CommitAuthored,
        "MrOpened" => ActivityKind::MrOpened,
        "MrMerged" => ActivityKind::MrMerged,
        "MrClosed" => ActivityKind::MrClosed,
        "MrReviewComment" => ActivityKind::MrReviewComment,
        "MrApproved" => ActivityKind::MrApproved,
        "IssueOpened" => ActivityKind::IssueOpened,
        "IssueClosed" => ActivityKind::IssueClosed,
        "IssueComment" => ActivityKind::IssueComment,
        "JiraIssueTransitioned" => ActivityKind::JiraIssueTransitioned,
        "JiraIssueCommented" => ActivityKind::JiraIssueCommented,
        "JiraIssueAssigned" => ActivityKind::JiraIssueAssigned,
        "JiraIssueUnassigned" => ActivityKind::JiraIssueUnassigned,
        "JiraIssueCreated" => ActivityKind::JiraIssueCreated,
        "ConfluencePageCreated" => ActivityKind::ConfluencePageCreated,
        "ConfluencePageEdited" => ActivityKind::ConfluencePageEdited,
        "ConfluenceComment" => ActivityKind::ConfluenceComment,
        other => {
            return Err(DbError::InvalidData {
                column: "activity_events.kind".into(),
                message: format!("unknown ActivityKind `{other}`"),
            });
        }
    };
    Ok(kind)
}

pub(crate) fn privacy_to_db(p: &Privacy) -> &'static str {
    match p {
        Privacy::Normal => "Normal",
        Privacy::RedactedPrivateRepo => "RedactedPrivateRepo",
    }
}

pub(crate) fn privacy_from_db(s: &str) -> Result<Privacy, DbError> {
    match s {
        "Normal" => Ok(Privacy::Normal),
        "RedactedPrivateRepo" => Ok(Privacy::RedactedPrivateRepo),
        other => Err(DbError::InvalidData {
            column: "activity_events.privacy".into(),
            message: format!("unknown Privacy `{other}`"),
        }),
    }
}

pub(crate) fn log_level_to_db(l: &LogLevel) -> &'static str {
    match l {
        LogLevel::Debug => "Debug",
        LogLevel::Info => "Info",
        LogLevel::Warn => "Warn",
        LogLevel::Error => "Error",
    }
}

pub(crate) fn log_level_from_db(s: &str) -> Result<LogLevel, DbError> {
    match s {
        "Debug" => Ok(LogLevel::Debug),
        "Info" => Ok(LogLevel::Info),
        "Warn" => Ok(LogLevel::Warn),
        "Error" => Ok(LogLevel::Error),
        other => Err(DbError::InvalidData {
            column: "log_entries.level".into(),
            message: format!("unknown LogLevel `{other}`"),
        }),
    }
}

pub(crate) fn artifact_kind_to_db(k: &ArtifactKind) -> &'static str {
    match k {
        ArtifactKind::CommitSet => "CommitSet",
        ArtifactKind::JiraIssue => "JiraIssue",
        ArtifactKind::ConfluencePage => "ConfluencePage",
    }
}

pub(crate) fn artifact_kind_from_db(s: &str) -> Result<ArtifactKind, DbError> {
    match s {
        "CommitSet" => Ok(ArtifactKind::CommitSet),
        "JiraIssue" => Ok(ArtifactKind::JiraIssue),
        "ConfluencePage" => Ok(ArtifactKind::ConfluencePage),
        other => Err(DbError::InvalidData {
            column: "artifacts.kind".into(),
            message: format!("unknown ArtifactKind `{other}`"),
        }),
    }
}

pub(crate) fn sync_run_status_to_db(s: &SyncRunStatus) -> &'static str {
    match s {
        SyncRunStatus::Running => "Running",
        SyncRunStatus::Completed => "Completed",
        SyncRunStatus::Cancelled => "Cancelled",
        SyncRunStatus::Failed => "Failed",
    }
}

pub(crate) fn sync_run_status_from_db(s: &str) -> Result<SyncRunStatus, DbError> {
    match s {
        "Running" => Ok(SyncRunStatus::Running),
        "Completed" => Ok(SyncRunStatus::Completed),
        "Cancelled" => Ok(SyncRunStatus::Cancelled),
        "Failed" => Ok(SyncRunStatus::Failed),
        other => Err(DbError::InvalidData {
            column: "sync_runs.status".into(),
            message: format!("unknown SyncRunStatus `{other}`"),
        }),
    }
}

pub(crate) fn sink_kind_to_db(k: &SinkKind) -> &'static str {
    match k {
        SinkKind::MarkdownFile => "MarkdownFile",
    }
}

pub(crate) fn sink_kind_from_db(s: &str) -> Result<SinkKind, DbError> {
    match s {
        "MarkdownFile" => Ok(SinkKind::MarkdownFile),
        other => Err(DbError::InvalidData {
            column: "sinks.kind".into(),
            message: format!("unknown SinkKind `{other}`"),
        }),
    }
}

pub(crate) fn source_identity_kind_to_db(k: &SourceIdentityKind) -> &'static str {
    match k {
        SourceIdentityKind::GitEmail => "GitEmail",
        SourceIdentityKind::GitLabUserId => "GitLabUserId",
        SourceIdentityKind::GitLabUsername => "GitLabUsername",
        SourceIdentityKind::GitHubLogin => "GitHubLogin",
        SourceIdentityKind::AtlassianAccountId => "AtlassianAccountId",
    }
}

pub(crate) fn source_identity_kind_from_db(s: &str) -> Result<SourceIdentityKind, DbError> {
    match s {
        "GitEmail" => Ok(SourceIdentityKind::GitEmail),
        "GitLabUserId" => Ok(SourceIdentityKind::GitLabUserId),
        "GitLabUsername" => Ok(SourceIdentityKind::GitLabUsername),
        "GitHubLogin" => Ok(SourceIdentityKind::GitHubLogin),
        "AtlassianAccountId" => Ok(SourceIdentityKind::AtlassianAccountId),
        other => Err(DbError::InvalidData {
            column: "source_identities.kind".into(),
            message: format!("unknown SourceIdentityKind `{other}`"),
        }),
    }
}
