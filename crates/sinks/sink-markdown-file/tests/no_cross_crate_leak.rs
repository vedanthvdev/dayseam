//! Crate-graph guard: `sink-markdown-file` deliberately does not
//! depend on any other workspace crate besides `sinks-sdk`,
//! `dayseam-core`, and (in dev-dependencies) `dayseam-events` for the
//! integration-test `RunStreams`. Adding any of the following edges
//! would be an architectural bug:
//!
//! * `dayseam-db` — sinks never talk to the activity store; they
//!   render bytes and return a receipt. The orchestrator is the only
//!   crate that writes sink bookkeeping.
//! * `dayseam-secrets` — v0.1 sinks are local-only and need no
//!   credentials. Remote sinks in v0.4+ will receive an
//!   `AuthStrategy` via an additive field on `SinkCtx`.
//! * `dayseam-report` — rendering is the step that produces the draft
//!   handed to the sink. A sink that depended on the report crate
//!   could trigger a re-render mid-write, which would break the
//!   atomicity guarantee.
//! * `connectors-sdk` — sinks are strictly downstream of connectors
//!   and share no types beyond what lives in `dayseam-core`. Any edge
//!   here is a layering bug.
//! * Any concrete `connector-*` crate — the sink must be
//!   connector-agnostic.

use cargo_metadata::MetadataCommand;

const FORBIDDEN: &[&str] = &[
    "dayseam-db",
    "dayseam-secrets",
    "dayseam-report",
    "connectors-sdk",
    "connector-local-git",
];

#[test]
fn sink_markdown_file_does_not_depend_on_forbidden_crates() {
    let metadata = MetadataCommand::new()
        .exec()
        .expect("cargo metadata must succeed");

    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == "sink-markdown-file")
        .expect("sink-markdown-file must be a workspace member");

    // Only inspect *non-dev* dependencies. Integration tests may pull
    // in `dayseam-events` for `RunStreams`, but a non-dev edge to any
    // forbidden crate is a layering bug.
    let direct_non_dev: Vec<&str> = pkg
        .dependencies
        .iter()
        .filter(|d| matches!(d.kind, cargo_metadata::DependencyKind::Normal))
        .map(|d| d.name.as_str())
        .collect();

    for forbidden in FORBIDDEN {
        assert!(
            !direct_non_dev.contains(forbidden),
            "sink-markdown-file must not depend on `{forbidden}` — see tests/no_cross_crate_leak.rs for why.\nObserved direct non-dev deps: {direct_non_dev:?}"
        );
    }
}
