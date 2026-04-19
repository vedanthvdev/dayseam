//! CI guard that keeps the crate graph one-way.
//!
//! `dayseam-report` is the **pure engine** at the centre of the
//! pipeline. It is allowed to depend on:
//!
//! * `dayseam-core` — the canonical domain types every crate speaks.
//!
//! It must **not** depend on:
//!
//! * any `connector-*` crate — connectors feed events and artifacts
//!   *into* the engine; a back-edge here would turn the engine into
//!   a connector-shaped mess and kill golden-snapshot determinism.
//! * any `sink-*` crate — sinks sit downstream of the engine; a
//!   back-edge would let the engine format for a specific output
//!   medium, which defeats the whole point of the structured
//!   `ReportDraft`.
//! * `dayseam-db` — persistence is the orchestrator's job; the
//!   engine reads its inputs off a struct and never touches SQLx.
//! * `dayseam-secrets` — no auth. The engine cannot need secrets by
//!   construction (no IO).
//! * `connectors-sdk` / `sinks-sdk` — SDKs exist so the orchestrator
//!   can dispatch across connectors/sinks; the engine never makes
//!   that kind of dispatch call.
//! * `dayseam-events` — run-scoped streams belong to the
//!   orchestrator. A pure engine has no progress to report.
//!
//! The test parses `cargo metadata` and asserts the forbidden edges
//! are absent. It is fast enough to run in CI without feature flags.

use cargo_metadata::MetadataCommand;

const FORBIDDEN: &[&str] = &[
    "dayseam-db",
    "dayseam-secrets",
    "dayseam-events",
    "connectors-sdk",
    "connector-local-git",
    "sinks-sdk",
];

#[test]
fn dayseam_report_does_not_depend_on_forbidden_crates() {
    let metadata = MetadataCommand::new()
        .exec()
        .expect("cargo metadata must succeed");

    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == "dayseam-report")
        .expect("dayseam-report must be a workspace member");

    let direct_deps: Vec<&str> = pkg.dependencies.iter().map(|d| d.name.as_str()).collect();

    for forbidden in FORBIDDEN {
        assert!(
            !direct_deps.contains(forbidden),
            "dayseam-report must not depend on `{forbidden}` — see tests/no_cross_crate_leak.rs for why.\nObserved direct deps: {direct_deps:?}"
        );
    }
}
