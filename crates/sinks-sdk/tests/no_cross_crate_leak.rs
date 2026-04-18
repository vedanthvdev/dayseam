//! CI guard that keeps the crate graph one-way.
//!
//! `sinks-sdk` deliberately does **not** depend on:
//!
//! * `dayseam-db` — sinks never persist directly to the activity store.
//!   They receive a rendered [`ReportDraft`] and return a
//!   [`WriteReceipt`]; the orchestrator is the only component that
//!   writes sink-related bookkeeping into SQLite. Importing
//!   `dayseam-db` here would invite sinks to read from tables that are
//!   none of their business.
//! * `dayseam-secrets` — v0.1 sinks are local-only and need no
//!   credentials. Remote sinks (v0.4+) will obtain an `AuthStrategy`
//!   through an additive field on [`crate::SinkCtx`], mirroring
//!   `ConnCtx` — they must **not** reach for the `SecretStore` directly.
//! * `dayseam-report` — rendering is the step that produces the draft
//!   handed to a sink. A sink that depended on the report crate would
//!   be able to trigger a re-render mid-write, which would break
//!   atomicity guarantees.
//! * `connectors-sdk` — sinks are strictly downstream of connectors and
//!   share no types beyond what lives in `dayseam-core`. Any edge
//!   between these two SDK crates is a layering bug.
//!
//! The test parses the workspace's `cargo metadata` output and asserts
//! the forbidden edges are absent. It is fast enough to run in CI
//! without feature flags.

use cargo_metadata::MetadataCommand;

const FORBIDDEN: &[&str] = &[
    "dayseam-db",
    "dayseam-secrets",
    "dayseam-report",
    "connectors-sdk",
];

#[test]
fn sinks_sdk_does_not_depend_on_forbidden_crates() {
    let metadata = MetadataCommand::new()
        .exec()
        .expect("cargo metadata must succeed");

    let sdk = metadata
        .packages
        .iter()
        .find(|p| p.name == "sinks-sdk")
        .expect("sinks-sdk must be a workspace member");

    let direct_deps: Vec<&str> = sdk.dependencies.iter().map(|d| d.name.as_str()).collect();

    for forbidden in FORBIDDEN {
        assert!(
            !direct_deps.contains(forbidden),
            "sinks-sdk must not depend on `{forbidden}` — see tests/no_cross_crate_leak.rs for why.\nObserved direct deps: {direct_deps:?}"
        );
    }
}
