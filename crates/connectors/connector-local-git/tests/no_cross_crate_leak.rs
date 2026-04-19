//! CI guard that keeps the crate graph one-way.
//!
//! `connector-local-git` is a **source connector** and must live
//! strictly upstream of persistence, secrets, reporting, and sinks. It
//! is allowed to depend on:
//!
//! * `connectors-sdk` — the trait + context surface it implements.
//! * `dayseam-core` — canonical domain types (`ActivityEvent`,
//!   `Artifact`, `SourceId`, …). This is the shared vocabulary every
//!   crate in the workspace speaks.
//! * `dayseam-events` — run-scoped progress/log senders it emits into.
//!
//! It must **not** depend on:
//!
//! * `dayseam-db` — connectors never persist. The orchestrator turns a
//!   `SyncResult` into rows.
//! * `dayseam-secrets` — auth is handed in through `ConnCtx::auth`. A
//!   direct edge here would let the connector bypass our `Secret<T>`
//!   discipline.
//! * `dayseam-report` — rendering is downstream. A connector that knew
//!   about `ReportDraft` would be tempted to short-circuit the
//!   canonical artifact layer.
//! * `sinks-sdk` — sinks are strictly downstream of connectors. Any
//!   import edge between the two is a layering bug.
//!
//! The test parses `cargo metadata` and asserts the forbidden edges are
//! absent. It is fast enough to run in CI without feature flags.

use cargo_metadata::MetadataCommand;

const FORBIDDEN: &[&str] = &[
    "dayseam-db",
    "dayseam-secrets",
    "dayseam-report",
    "sinks-sdk",
];

#[test]
fn connector_local_git_does_not_depend_on_forbidden_crates() {
    let metadata = MetadataCommand::new()
        .exec()
        .expect("cargo metadata must succeed");

    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == "connector-local-git")
        .expect("connector-local-git must be a workspace member");

    let direct_deps: Vec<&str> = pkg.dependencies.iter().map(|d| d.name.as_str()).collect();

    for forbidden in FORBIDDEN {
        assert!(
            !direct_deps.contains(forbidden),
            "connector-local-git must not depend on `{forbidden}` — see tests/no_cross_crate_leak.rs for why.\nObserved direct deps: {direct_deps:?}"
        );
    }
}
