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
    error_codes, ActivityEvent, ActivityKind, Actor, Artifact, ArtifactId, ArtifactKind,
    ArtifactPayload, AtlassianValidationResult, DayseamError, EntityRef, Evidence,
    GithubValidationResult, GitlabValidationResult, Identity, Link, LocalRepo, LogEntry, LogEvent,
    LogLevel, PerSourceState, Person, Privacy, ProgressEvent, ProgressPhase, RawRef,
    RenderedBullet, RenderedSection, ReportCompletedEvent, ReportDraft, RunId, RunStatus,
    ScheduleConfig, SecretRef, Settings, SettingsPatch, Sink, SinkCapabilities, SinkConfig,
    SinkKind, Source, SourceConfig, SourceHealth, SourceIdentity, SourceIdentityKind, SourceKind,
    SourcePatch, SourceRunState, SyncRun, SyncRunCancelReason, SyncRunStatus, SyncRunTrigger,
    ThemePreference, ToastEvent, ToastSeverity, WriteReceipt,
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

    Artifact::export_all(&cfg).expect("export Artifact");
    ArtifactId::export_all(&cfg).expect("export ArtifactId");
    ArtifactKind::export_all(&cfg).expect("export ArtifactKind");
    ArtifactPayload::export_all(&cfg).expect("export ArtifactPayload");

    SyncRun::export_all(&cfg).expect("export SyncRun");
    SyncRunTrigger::export_all(&cfg).expect("export SyncRunTrigger");
    SyncRunStatus::export_all(&cfg).expect("export SyncRunStatus");
    SyncRunCancelReason::export_all(&cfg).expect("export SyncRunCancelReason");
    PerSourceState::export_all(&cfg).expect("export PerSourceState");

    Source::export_all(&cfg).expect("export Source");
    SourceKind::export_all(&cfg).expect("export SourceKind");
    SourceConfig::export_all(&cfg).expect("export SourceConfig");
    SourceHealth::export_all(&cfg).expect("export SourceHealth");
    SourcePatch::export_all(&cfg).expect("export SourcePatch");
    SecretRef::export_all(&cfg).expect("export SecretRef");
    GitlabValidationResult::export_all(&cfg).expect("export GitlabValidationResult");
    AtlassianValidationResult::export_all(&cfg).expect("export AtlassianValidationResult");
    GithubValidationResult::export_all(&cfg).expect("export GithubValidationResult");

    Sink::export_all(&cfg).expect("export Sink");
    SinkKind::export_all(&cfg).expect("export SinkKind");
    SinkConfig::export_all(&cfg).expect("export SinkConfig");
    SinkCapabilities::export_all(&cfg).expect("export SinkCapabilities");
    WriteReceipt::export_all(&cfg).expect("export WriteReceipt");

    Identity::export_all(&cfg).expect("export Identity");
    Person::export_all(&cfg).expect("export Person");
    SourceIdentity::export_all(&cfg).expect("export SourceIdentity");
    SourceIdentityKind::export_all(&cfg).expect("export SourceIdentityKind");
    LocalRepo::export_all(&cfg).expect("export LocalRepo");

    ReportDraft::export_all(&cfg).expect("export ReportDraft");
    RenderedSection::export_all(&cfg).expect("export RenderedSection");
    RenderedBullet::export_all(&cfg).expect("export RenderedBullet");
    Evidence::export_all(&cfg).expect("export Evidence");
    SourceRunState::export_all(&cfg).expect("export SourceRunState");
    RunStatus::export_all(&cfg).expect("export RunStatus");
    LogEntry::export_all(&cfg).expect("export LogEntry");
    LogLevel::export_all(&cfg).expect("export LogLevel");

    RunId::export_all(&cfg).expect("export RunId");
    ProgressEvent::export_all(&cfg).expect("export ProgressEvent");
    ProgressPhase::export_all(&cfg).expect("export ProgressPhase");
    LogEvent::export_all(&cfg).expect("export LogEvent");
    ToastEvent::export_all(&cfg).expect("export ToastEvent");
    ToastSeverity::export_all(&cfg).expect("export ToastSeverity");
    ReportCompletedEvent::export_all(&cfg).expect("export ReportCompletedEvent");

    Settings::export_all(&cfg).expect("export Settings");
    SettingsPatch::export_all(&cfg).expect("export SettingsPatch");
    ThemePreference::export_all(&cfg).expect("export ThemePreference");

    ScheduleConfig::export_all(&cfg).expect("export ScheduleConfig");

    DayseamError::export_all(&cfg).expect("export DayseamError");

    export_gitlab_error_codes(out_dir);
    export_atlassian_error_codes(out_dir);
    export_github_error_codes(out_dir);
}

/// Regenerate `gitlabErrorCodes.ts` so the frontend parity test always sees
/// the authoritative list from `dayseam_core::error_codes`. Kept next to the
/// ts-rs exports so that `generated_ts_types_match_committed` catches drift
/// the moment a new `gitlab.*` code is added in Rust without a matching copy
/// entry in `gitlabErrorCopy`.
fn export_gitlab_error_codes(out_dir: &std::path::Path) {
    let codes: Vec<&str> = error_codes::ALL
        .iter()
        .copied()
        .filter(|c| c.starts_with("gitlab."))
        .collect();
    let mut body = String::new();
    body.push_str("// AUTO-GENERATED FILE. Do not edit by hand.\n");
    body.push_str(
        "// Regenerated from `dayseam_core::error_codes::ALL` by the\n\
         // `ts_types_generated` test. Add the copy entry in\n\
         // `src/features/sources/gitlabErrorCopy.ts` whenever this list\n\
         // grows, otherwise the frontend parity test fails.\n\n",
    );
    body.push_str("export const GITLAB_ERROR_CODES = [\n");
    for code in &codes {
        body.push_str(&format!("  \"{code}\",\n"));
    }
    body.push_str("] as const;\n\n");
    body.push_str("export type GitlabErrorCode = (typeof GITLAB_ERROR_CODES)[number];\n");
    std::fs::write(out_dir.join("gitlabErrorCodes.ts"), body).expect("write gitlabErrorCodes.ts");
}

/// Regenerate `atlassianErrorCodes.ts` with every `atlassian.*`,
/// `jira.*`, and `confluence.*` code from `error_codes::ALL`. The
/// Atlassian stack splits its codes across three prefixes — shared
/// auth / cloud / ADF concerns sit under `atlassian.`, per-product
/// walker failures sit under `jira.` or `confluence.` — so the
/// generated set is a union of those three families rather than a
/// single prefix filter. `SourceErrorCard` keys its copy lookup off
/// this literal array, which keeps the "every code has copy" parity
/// test honest when a new code is added in Rust.
fn export_atlassian_error_codes(out_dir: &std::path::Path) {
    let codes: Vec<&str> = error_codes::ALL
        .iter()
        .copied()
        .filter(|c| {
            c.starts_with("atlassian.") || c.starts_with("jira.") || c.starts_with("confluence.")
        })
        .collect();
    let mut body = String::new();
    body.push_str("// AUTO-GENERATED FILE. Do not edit by hand.\n");
    body.push_str(
        "// Regenerated from `dayseam_core::error_codes::ALL` by the\n\
         // `ts_types_generated` test. Includes every `atlassian.*`,\n\
         // `jira.*`, and `confluence.*` code — the three-prefix split\n\
         // the Atlassian stack uses. Add the copy entry in\n\
         // `src/features/sources/atlassianErrorCopy.ts` whenever this\n\
         // list grows, otherwise the frontend parity test fails.\n\n",
    );
    body.push_str("export const ATLASSIAN_ERROR_CODES = [\n");
    for code in &codes {
        body.push_str(&format!("  \"{code}\",\n"));
    }
    body.push_str("] as const;\n\n");
    body.push_str("export type AtlassianErrorCode = (typeof ATLASSIAN_ERROR_CODES)[number];\n");
    std::fs::write(out_dir.join("atlassianErrorCodes.ts"), body)
        .expect("write atlassianErrorCodes.ts");
}

/// Regenerate `githubErrorCodes.ts` with every `github.*` code from
/// `error_codes::ALL`. `SourceErrorCard` keys its GitHub copy lookup
/// off this literal array so that when a new `github.*` code is
/// introduced in Rust, the frontend parity test (see
/// `SourceErrorCard.parity.test.tsx`) fails until a matching entry is
/// added to `githubErrorCopy.ts`.
fn export_github_error_codes(out_dir: &std::path::Path) {
    let codes: Vec<&str> = error_codes::ALL
        .iter()
        .copied()
        .filter(|c| c.starts_with("github."))
        .collect();
    let mut body = String::new();
    body.push_str("// AUTO-GENERATED FILE. Do not edit by hand.\n");
    body.push_str(
        "// Regenerated from `dayseam_core::error_codes::ALL` by the\n\
         // `ts_types_generated` test. Add the copy entry in\n\
         // `src/features/sources/githubErrorCopy.ts` whenever this list\n\
         // grows, otherwise the frontend parity test fails.\n\n",
    );
    body.push_str("export const GITHUB_ERROR_CODES = [\n");
    for code in &codes {
        body.push_str(&format!("  \"{code}\",\n"));
    }
    body.push_str("] as const;\n\n");
    body.push_str("export type GithubErrorCode = (typeof GITHUB_ERROR_CODES)[number];\n");
    std::fs::write(out_dir.join("githubErrorCodes.ts"), body).expect("write githubErrorCodes.ts");
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
