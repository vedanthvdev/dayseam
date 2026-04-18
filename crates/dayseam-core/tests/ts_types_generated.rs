//! CI guard that fails if the committed TypeScript types under
//! `packages/ipc-types/src/generated/` have drifted from the Rust types.
//!
//! The test explicitly re-exports every top-level type via `ts-rs`, which
//! overwrites the corresponding `.ts` files in-place, and then runs
//! `git diff --exit-code` against that directory. The diff test is
//! idempotent: if nothing changed, the test is a no-op; if something
//! changed, the assertion prints the diff so the author knows exactly
//! which type drifted.
//!
//! Running `cargo test -p dayseam-core --test ts_types_generated` locally
//! is how you regenerate the TS bindings after editing a Rust type.

use std::path::{Path, PathBuf};
use std::process::Command;

use dayseam_core::{
    ActivityEvent, ActivityKind, Actor, DayseamError, EntityRef, Evidence, Identity, Link,
    LocalRepo, LogEntry, LogLevel, Privacy, RawRef, RenderedBullet, RenderedSection, ReportDraft,
    RunStatus, SecretRef, Source, SourceConfig, SourceHealth, SourceKind, SourceRunState,
};
use ts_rs::{Config, TS};

fn export_all(out_dir: &Path) {
    // Writing is idempotent — ts-rs truncates and rewrites each file.
    // We set the output directory explicitly so the test is self-contained
    // and doesn't depend on `TS_RS_EXPORT_DIR` being set in the
    // environment. Large integers render as TS `number` rather than
    // `bigint` because the values we actually use (user ids, counts,
    // retry-after seconds) are well within `Number.MAX_SAFE_INTEGER`, and
    // `JSON.parse` produces a `number` anyway.
    let cfg = Config::default()
        .with_out_dir(out_dir.to_path_buf())
        .with_large_int("number");
    ActivityEvent::export_all(&cfg).expect("export ActivityEvent");
    ActivityKind::export_all(&cfg).expect("export ActivityKind");
    Actor::export_all(&cfg).expect("export Actor");
    Link::export_all(&cfg).expect("export Link");
    EntityRef::export_all(&cfg).expect("export EntityRef");
    RawRef::export_all(&cfg).expect("export RawRef");
    Privacy::export_all(&cfg).expect("export Privacy");

    Source::export_all(&cfg).expect("export Source");
    SourceKind::export_all(&cfg).expect("export SourceKind");
    SourceConfig::export_all(&cfg).expect("export SourceConfig");
    SourceHealth::export_all(&cfg).expect("export SourceHealth");
    SecretRef::export_all(&cfg).expect("export SecretRef");

    Identity::export_all(&cfg).expect("export Identity");
    LocalRepo::export_all(&cfg).expect("export LocalRepo");

    ReportDraft::export_all(&cfg).expect("export ReportDraft");
    RenderedSection::export_all(&cfg).expect("export RenderedSection");
    RenderedBullet::export_all(&cfg).expect("export RenderedBullet");
    Evidence::export_all(&cfg).expect("export Evidence");
    SourceRunState::export_all(&cfg).expect("export SourceRunState");
    RunStatus::export_all(&cfg).expect("export RunStatus");
    LogEntry::export_all(&cfg).expect("export LogEntry");
    LogLevel::export_all(&cfg).expect("export LogLevel");

    DayseamError::export_all(&cfg).expect("export DayseamError");
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at `crates/dayseam-core/`; the workspace
    // root is two levels up. Falling back to `env::current_dir` would be
    // wrong when the test is invoked from a different cwd.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(std::path::Path::parent)
        .map(PathBuf::from)
        .expect("crates/dayseam-core lives two levels below the workspace root")
}

#[test]
fn generated_ts_types_match_committed() {
    let root = repo_root();
    let out_dir = root.join("packages/ipc-types/src/generated");
    std::fs::create_dir_all(&out_dir).expect("create generated dir");
    export_all(&out_dir);

    // `git status --porcelain` surfaces both modified and untracked files,
    // so adding a new `#[ts(export)]` type without committing the
    // generated `.ts` file also fails the test — not just edits to
    // existing files.
    let output = Command::new("git")
        .args([
            "status",
            "--porcelain",
            "--",
            "packages/ipc-types/src/generated/",
        ])
        .current_dir(&root)
        .output()
        .expect("git must be on PATH to run this test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "git status failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if !stdout.trim().is_empty() {
        panic!(
            "\npackages/ipc-types/src/generated/ is out of date.\n\
             Regenerate with:\n\n    \
             cargo test -p dayseam-core --test ts_types_generated\n\n\
             then `git add packages/ipc-types/src/generated/` and commit.\n\n\
             --- git status ---\n{stdout}"
        );
    }
}
