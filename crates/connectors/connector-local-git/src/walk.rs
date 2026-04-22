//! Per-repository commit walker.
//!
//! Given one repository and a day window in the user's local
//! timezone, walk every branch tip + `HEAD`, deduplicate commits by
//! SHA, attribute each commit to its author or committer email, and
//! emit a `CommitAuthored` [`ActivityEvent`] for each commit whose
//! author **or committer** resolves to the current
//! [`dayseam_core::Person`] via `ctx.source_identities`.
//!
//! The committer path is load-bearing on rebased / amended / merged
//! work: a maintainer who rebased a co-worker's branch onto `main`
//! is the committer; the original author email need not match self
//! identity. Filtering on author-only drops that class of work.
//! Similarly, day-bucketing uses the **committer** timestamp, not
//! the author timestamp — the committer time is the moment the
//! commit object came into existence on this machine, which is the
//! moment the work actually happened from the dogfooder's point of
//! view. An amended-last-week commit committed today appears in
//! today's report, matching the Phase 2 plan invariant #4.
//!
//! The walker returns a [`RepoWalk`] rather than writing directly so
//! the connector layer can decide whether to redact, whether to
//! include the commit in a `CommitSet` artefact, and how to surface
//! walk-level warnings (empty signatures, opaque walk errors).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use dayseam_core::{
    error_codes, ActivityEvent, ActivityKind, Actor, Artifact, ArtifactId, ArtifactKind,
    ArtifactPayload, DayseamError, EntityKind, EntityRef, Link, Privacy, RawRef, SourceId,
};
use git2::{Repository, Sort};

use crate::privacy::redact_private_event;

/// Everything one `(repo, day)` walk produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWalk {
    /// One event per *kept* commit, deduplicated by SHA, already
    /// privacy-redacted if the repo was flagged private.
    pub events: Vec<ActivityEvent>,
    /// The artefact grouping the events above. `None` when no commit
    /// matched — the caller should not emit an empty `CommitSet`.
    pub artifact: Option<Artifact>,
    /// Commits visited whose actor did not resolve to `person`.
    /// Counted so the connector can roll them into `stats.filtered_by_identity`.
    pub filtered_by_identity: u64,
    /// Commits visited that fell outside the requested day window, or
    /// that carried a malformed / ambiguous committer timestamp that
    /// could not be bucketed safely. Malformed-timestamp commits are
    /// counted here rather than silently flagging "today" through
    /// `Utc::now()` — a corrupt commit should drop out, not surface
    /// in the current report.
    pub filtered_by_date: u64,
    /// True when at least one commit had no usable author email, so
    /// the connector emits a single
    /// [`error_codes::LOCAL_GIT_NO_SIGNATURE`] warning per repo.
    pub saw_missing_signature: bool,
}

/// Walk one repository for one day. `local_tz` is the user's local
/// timezone at walk time; commits are bucketed in this tz so a commit
/// at 01:00 UTC on day D+1 but 20:00 local on day D is attributed to
/// day D when the user is in UTC-5.
#[tracing::instrument(
    skip_all,
    fields(
        connector = "local-git",
        source_id = %source_id,
        day = %day,
        repo = %repo_root.display()
    )
)]
pub fn walk_repo_for_day(
    source_id: &SourceId,
    repo_root: &Path,
    day: NaiveDate,
    local_tz: FixedOffset,
    identity_emails: &HashSet<String>,
    is_private: bool,
) -> Result<RepoWalk, DayseamError> {
    let repo = Repository::open(repo_root).map_err(|e| map_repo_error(repo_root, e))?;

    let mut revwalk = repo.revwalk().map_err(|e| map_repo_error(repo_root, e))?;
    revwalk
        .set_sorting(Sort::TIME)
        .map_err(|e| map_repo_error(repo_root, e))?;
    push_every_branch_tip(&repo, &mut revwalk).map_err(|e| map_repo_error(repo_root, e))?;

    let mut seen: HashSet<git2::Oid> = HashSet::new();
    let mut by_sha: HashMap<String, ActivityEvent> = HashMap::new();
    let mut filtered_by_identity: u64 = 0;
    let mut filtered_by_date: u64 = 0;
    let mut saw_missing_signature = false;
    let mut commit_shas: Vec<String> = Vec::new();
    let mut event_ids: Vec<uuid::Uuid> = Vec::new();

    for oid in revwalk {
        let oid = oid.map_err(|e| map_repo_error(repo_root, e))?;
        if !seen.insert(oid) {
            continue;
        }

        let commit = repo
            .find_commit(oid)
            .map_err(|e| map_repo_error(repo_root, e))?;

        // Bucket on committer-time (COR-01): the moment the commit
        // entered this repo, not the original authoring time. A
        // commit authored last week but committed today belongs in
        // today's report.
        let when = match commit_timestamp_utc(&commit) {
            Some(t) => t,
            None => {
                // Malformed / ambiguous timestamps drop out rather
                // than silently surfacing in the current day (COR-02).
                filtered_by_date += 1;
                continue;
            }
        };
        let commit_day = when.with_timezone(&local_tz).date_naive();
        if commit_day != day {
            filtered_by_date += 1;
            continue;
        }

        let (display_name, author_email, committer_email) = commit_parts(&commit);
        if author_email.is_none() && committer_email.is_none() {
            saw_missing_signature = true;
        }

        // Identity filter matches on *either* author or committer
        // email (COR-04 / plan invariant #2). A rebased/amended
        // commit where the committer is self but the author is
        // someone else should still count as mine.
        let author_lower = author_email.as_ref().map(|s| s.to_lowercase());
        let committer_lower = committer_email.as_ref().map(|s| s.to_lowercase());
        let matches = author_lower
            .as_ref()
            .map(|e| identity_emails.contains(e))
            .unwrap_or(false)
            || committer_lower
                .as_ref()
                .map(|e| identity_emails.contains(e))
                .unwrap_or(false);
        if !matches {
            filtered_by_identity += 1;
            continue;
        }

        let sha = oid.to_string();

        // Pick the actor email that actually matched: prefer author
        // when it matches (reads more naturally in the report), fall
        // back to committer otherwise.
        let matched_email = if author_lower
            .as_ref()
            .map(|e| identity_emails.contains(e))
            .unwrap_or(false)
        {
            author_email.clone()
        } else {
            committer_email.clone()
        };

        let mut event = build_commit_event(
            *source_id,
            repo_root,
            &sha,
            &commit,
            when,
            display_name,
            matched_email,
        );
        if is_private {
            redact_private_event(&mut event);
        }
        commit_shas.push(sha.clone());
        event_ids.push(event.id);
        by_sha.insert(sha, event);
    }
    // The `seen: HashSet<git2::Oid>` above already rejects duplicate
    // OIDs before we reach `by_sha`; a SHA collision with an OID we
    // did not already see would require two distinct commits hashing
    // to the same SHA-1, which is not possible in practice.

    // Stable output ordering — the HashMap iteration order is not
    // stable across runs, but integration tests key off the same
    // repo-level ordering every run.
    let mut events: Vec<ActivityEvent> = by_sha.into_values().collect();
    events.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    commit_shas.sort();
    event_ids.sort();

    let artifact = if events.is_empty() {
        None
    } else {
        Some(build_commit_set_artifact(
            *source_id,
            repo_root,
            day,
            event_ids,
            commit_shas,
        ))
    };

    Ok(RepoWalk {
        events,
        artifact,
        filtered_by_identity,
        filtered_by_date,
        saw_missing_signature,
    })
}

fn push_every_branch_tip(
    repo: &Repository,
    revwalk: &mut git2::Revwalk<'_>,
) -> Result<(), git2::Error> {
    // HEAD first; a repo in the middle of a rebase with no branches
    // still has a HEAD we can walk.
    if let Ok(head) = repo.head() {
        if let Some(oid) = head.target() {
            revwalk.push(oid)?;
        }
    }
    let mut any = false;
    for b in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = b?;
        if let Some(oid) = branch.get().target() {
            revwalk.push(oid)?;
            any = true;
        }
    }
    // If a repo has *only* a detached HEAD we still want to walk it;
    // the HEAD push above handled that case.
    let _ = any;
    Ok(())
}

/// Convert the committer-time of `commit` to UTC. Returns `None`
/// when the underlying `git2::Time` is out-of-range or ambiguous —
/// callers treat that as "drop the commit" rather than silently
/// bucketing it to `Utc::now()`.
fn commit_timestamp_utc(commit: &git2::Commit<'_>) -> Option<DateTime<Utc>> {
    let when = commit.committer().when();
    // `git2::Time::seconds()` is unix seconds; the signature's
    // offset encodes the commit's original timezone, which we
    // discard — the walker owns the local-tz bucketing.
    Utc.timestamp_opt(when.seconds(), 0).single()
}

/// Display name + author/committer emails for `commit`. The walker
/// uses both emails for identity resolution so we surface them
/// separately rather than collapsing to a single tuple.
fn commit_parts(commit: &git2::Commit<'_>) -> (String, Option<String>, Option<String>) {
    let author = commit.author();
    let committer = commit.committer();
    let name = author.name().map(|s| s.to_string()).unwrap_or_else(|| {
        committer
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });
    let author_email = author.email().map(|s| s.to_string());
    let committer_email = committer.email().map(|s| s.to_string());
    (name, author_email, committer_email)
}

fn build_commit_event(
    source_id: SourceId,
    repo_root: &Path,
    sha: &str,
    commit: &git2::Commit<'_>,
    when: DateTime<Utc>,
    display_name: String,
    email: Option<String>,
) -> ActivityEvent {
    let source_id_str = source_id.to_string();
    let id = ActivityEvent::deterministic_id(&source_id_str, sha, "CommitAuthored");
    let message = commit.message().unwrap_or("").trim().to_string();
    let (title, body) = split_message(&message);
    let repo_url = format!("file://{}", repo_root.display());
    // DAY-72 CONS-addendum-04: surface the repo's human label on the
    // `repo` `EntityRef` to match the shape `connector-gitlab` emits
    // (where `label = Some(basename(path_with_namespace))`). Before
    // this, local-git events carried `label: None` while GitLab
    // events carried a populated label, forcing every downstream
    // reader (rollup, render, future tooling) to re-derive the
    // basename from `external_id`. Keeping the contract uniform
    // across connectors was an explicit review-addendum goal.
    let repo_label = repo_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    ActivityEvent {
        id,
        source_id,
        external_id: sha.to_string(),
        kind: ActivityKind::CommitAuthored,
        occurred_at: when,
        actor: Actor {
            display_name,
            email,
            external_id: None,
        },
        title,
        body,
        links: vec![Link {
            url: repo_url,
            label: Some(repo_label.clone()),
        }],
        entities: vec![EntityRef {
            kind: EntityKind::Repo,
            external_id: repo_root.display().to_string(),
            label: Some(repo_label),
        }],
        parent_external_id: None,
        metadata: serde_json::json!({}),
        raw_ref: RawRef {
            storage_key: format!("local-git:commit:{sha}"),
            content_type: "application/x-git-commit".to_string(),
        },
        privacy: Privacy::Normal,
    }
}

fn split_message(message: &str) -> (String, Option<String>) {
    match message.split_once('\n') {
        Some((title, rest)) => {
            let body = rest.trim();
            if body.is_empty() {
                (title.trim().to_string(), None)
            } else {
                (title.trim().to_string(), Some(body.to_string()))
            }
        }
        None => (message.to_string(), None),
    }
}

fn build_commit_set_artifact(
    source_id: SourceId,
    repo_root: &Path,
    day: NaiveDate,
    event_ids: Vec<uuid::Uuid>,
    commit_shas: Vec<String>,
) -> Artifact {
    let repo_path: PathBuf = repo_root.to_path_buf();
    let external_id = format!("{}::{}", repo_path.display(), day);
    let id = ArtifactId::deterministic(&source_id, ArtifactKind::CommitSet, &external_id);

    Artifact {
        id,
        source_id,
        kind: ArtifactKind::CommitSet,
        external_id,
        payload: ArtifactPayload::CommitSet {
            repo_path,
            date: day,
            event_ids,
            commit_shas,
        },
        created_at: Utc::now(),
    }
}

fn map_repo_error(repo_root: &Path, err: git2::Error) -> DayseamError {
    // We map every git2 error to a stable connector-level code so the
    // UI does not leak libgit2-specific strings. The three buckets
    // below cover the Task 2 plan: locked, corrupt, unreadable. The
    // distinction is coarse but review-friendly.
    let code = match err.class() {
        git2::ErrorClass::Os | git2::ErrorClass::Filesystem => {
            error_codes::LOCAL_GIT_REPO_UNREADABLE
        }
        git2::ErrorClass::Index | git2::ErrorClass::Reference => error_codes::LOCAL_GIT_REPO_LOCKED,
        _ => error_codes::LOCAL_GIT_REPO_CORRUPT,
    };
    DayseamError::Io {
        code: code.to_string(),
        path: Some(repo_root.to_path_buf()),
        message: err.message().to_string(),
    }
}
