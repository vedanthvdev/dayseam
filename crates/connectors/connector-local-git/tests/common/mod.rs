//! Shared helpers for the connector-local-git integration tests.
//!
//! The tests construct real on-disk git repositories via `git2`
//! rather than shipping tarball blobs. This keeps the fixtures
//! reviewable in Rust (diffs show intent, not base64), deterministic
//! (we pin every author, timestamp, and SHA), and self-contained
//! (one `tempfile::TempDir` per test, cleaned up automatically).
//!
//! Every helper is `#[allow(dead_code)]` at the module level — each
//! integration test file uses a subset.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use connectors_sdk::{
    AuthStrategy, ConnCtx, HttpClient, NoneAuth, NoopRawStore, RetryPolicy, SystemClock,
};
use dayseam_core::{Person, SourceIdentity, SourceIdentityKind};
use dayseam_events::{LogReceiver, LogSender, ProgressReceiver, ProgressSender, RunStreams};
use git2::{Repository, Signature};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const SELF_EMAIL: &str = "me@example.com";
pub const OTHER_EMAIL: &str = "someone-else@example.com";

/// Bundle the connector context needs to run, plus the receiving
/// ends so tests can inspect emitted progress/log events.
pub struct CtxHarness {
    pub ctx: ConnCtx,
    pub cancel: CancellationToken,
    pub progress_rx: ProgressReceiver,
    pub log_rx: LogReceiver,
    pub progress_tx: ProgressSender,
    pub log_tx: LogSender,
    pub source_id: Uuid,
}

/// Build a [`ConnCtx`] with one [`SourceIdentity::GitEmail`] row
/// matching `SELF_EMAIL`.
pub fn build_ctx_with_self() -> CtxHarness {
    let source_id = Uuid::new_v4();
    let identities = vec![SourceIdentity {
        id: Uuid::new_v4(),
        person_id: Uuid::new_v4(),
        source_id: Some(source_id),
        kind: SourceIdentityKind::GitEmail,
        external_actor_id: SELF_EMAIL.to_string(),
    }];
    build_ctx(source_id, identities)
}

pub fn build_ctx(source_id: Uuid, identities: Vec<SourceIdentity>) -> CtxHarness {
    let streams = RunStreams::new(dayseam_events::RunId::new());
    let ((progress_tx, log_tx), (progress_rx, log_rx)) = streams.split();
    let run_id = progress_tx.run_id();
    let cancel = CancellationToken::new();

    let ctx = ConnCtx {
        run_id,
        source_id,
        person: Person::new_self("Test"),
        source_identities: identities,
        auth: Arc::new(NoneAuth) as Arc<dyn AuthStrategy>,
        progress: progress_tx.clone(),
        logs: log_tx.clone(),
        raw_store: Arc::new(NoopRawStore),
        clock: Arc::new(SystemClock),
        http: HttpClient::new()
            .expect("build http client")
            .with_policy(RetryPolicy::instant()),
        cancel: cancel.clone(),
    };

    CtxHarness {
        ctx,
        cancel,
        progress_rx,
        log_rx,
        progress_tx,
        log_tx,
        source_id,
    }
}

/// One commit to seed into a fixture repo.
pub struct FixtureCommit {
    pub author_name: &'static str,
    pub author_email: &'static str,
    pub message: &'static str,
    pub when_utc: DateTime<Utc>,
}

/// Like [`FixtureCommit`] but with separate committer identity and
/// timestamp. Use this for rebased / amended / committed-on-behalf
/// fixtures where `author_*` and `committer_*` differ. A real-world
/// example: a maintainer rebases a PR author's branch onto `main`
/// — the commit's author remains the PR author but the committer
/// is the maintainer, and the committer-time is "now" rather than
/// the original authoring time.
pub struct RebasedCommit {
    pub author_name: &'static str,
    pub author_email: &'static str,
    pub committer_name: &'static str,
    pub committer_email: &'static str,
    pub message: &'static str,
    pub author_when_utc: DateTime<Utc>,
    pub committer_when_utc: DateTime<Utc>,
}

/// Initialise a git repo at `path` and append `commits` in order.
/// Each commit has an empty tree (no working files) so the fixture
/// is small and deterministic; the walker doesn't care about tree
/// contents.
pub fn make_fixture_repo(path: &Path, commits: &[FixtureCommit]) {
    std::fs::create_dir_all(path).unwrap();
    let repo = Repository::init(path).unwrap();
    let mut index = repo.index().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    let mut parent_oid: Option<git2::Oid> = None;
    for c in commits {
        let sig = Signature::new(
            c.author_name,
            c.author_email,
            &git2::Time::new(c.when_utc.timestamp(), 0),
        )
        .unwrap();

        let parents: Vec<git2::Commit> = parent_oid
            .map(|oid| vec![repo.find_commit(oid).unwrap()])
            .unwrap_or_default();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, c.message, &tree, &parent_refs)
            .unwrap();
        parent_oid = Some(oid);
    }

    // Drop the index lock before the repo handle goes out of scope so
    // tempdir cleanup doesn't race with it on Windows CI.
    drop(index);
    drop(tree);
    drop(repo);
}

/// Initialise a git repo at `path` with commits whose author and
/// committer signatures differ. Used by the author ≠ committer
/// regression tests (plan invariants #2 and #4).
pub fn make_fixture_repo_rebased(path: &Path, commits: &[RebasedCommit]) {
    std::fs::create_dir_all(path).unwrap();
    let repo = Repository::init(path).unwrap();
    let mut index = repo.index().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    let mut parent_oid: Option<git2::Oid> = None;
    for c in commits {
        let author = Signature::new(
            c.author_name,
            c.author_email,
            &git2::Time::new(c.author_when_utc.timestamp(), 0),
        )
        .unwrap();
        let committer = Signature::new(
            c.committer_name,
            c.committer_email,
            &git2::Time::new(c.committer_when_utc.timestamp(), 0),
        )
        .unwrap();

        let parents: Vec<git2::Commit> = parent_oid
            .map(|oid| vec![repo.find_commit(oid).unwrap()])
            .unwrap_or_default();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

        let oid = repo
            .commit(
                Some("HEAD"),
                &author,
                &committer,
                c.message,
                &tree,
                &parent_refs,
            )
            .unwrap();
        parent_oid = Some(oid);
    }

    drop(index);
    drop(tree);
    drop(repo);
}

/// Mark `repo_path` as private via the in-repo `.dayseam/private`
/// marker file. Used by the privacy test.
pub fn mark_private(repo_path: &Path) {
    let d = repo_path.join(".dayseam");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("private"), b"").unwrap();
}

/// Convenience: construct a UTC [`DateTime`] for a local date-time.
pub fn at_utc(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, hh, mm, 0).unwrap()
}

/// The local timezone the connector should use for the day-bucket
/// tests. UTC-5 so `01:00 UTC` day D+1 lands on day D locally.
pub fn tz_minus_five() -> FixedOffset {
    FixedOffset::west_opt(5 * 3600).unwrap()
}

pub fn utc_tz() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

/// Paths used by several tests — one "mine" repo and one "theirs"
/// repo placed side by side under a common scan root.
pub struct TwoRepos {
    pub scan_root: PathBuf,
    pub mine: PathBuf,
    pub theirs: PathBuf,
}

pub fn layout_two_repos(scan_root: &Path) -> TwoRepos {
    TwoRepos {
        scan_root: scan_root.to_path_buf(),
        mine: scan_root.join("mine"),
        theirs: scan_root.join("theirs"),
    }
}
