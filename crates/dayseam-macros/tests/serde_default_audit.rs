//! Trybuild suite for the `SerdeDefaultAudit` derive macro.
//!
//! The suite has two jobs:
//!
//! 1. Confirm the derive accepts every shape the production types
//!    actually use (enum variants with a mix of audited and
//!    non-`#[serde(default)]` fields; plain structs with explicit
//!    waivers; fields tagged `#[serde(default = "path")]`). These
//!    live under `pass/`.
//! 2. Confirm the derive *fails* the workspace build for the
//!    DOG-v0.2-04-class bug — a `#[serde(default)]` field without a
//!    paired `#[serde_default_audit(...)]` annotation — with a
//!    readable error that names the offending field. Compile-fail
//!    fixtures live under `fail/`, and the `.stderr` snapshots next
//!    to each are the golden error messages the derive must keep
//!    producing.
//!
//! Running the suite:
//!
//!   cargo test -p dayseam-macros --test serde_default_audit
//!
//! To refresh a `.stderr` snapshot after an intentional error-message
//! change, run with `TRYBUILD=overwrite`.

#[test]
fn serde_default_audit_trybuild() {
    let t = trybuild::TestCases::new();
    t.pass("tests/trybuild/pass/accepts_repair_annotation.rs");
    t.pass("tests/trybuild/pass/accepts_no_repair_waiver.rs");
    t.pass("tests/trybuild/pass/accepts_enum_variant_fields.rs");
    t.pass("tests/trybuild/pass/accepts_github_variant.rs");
    t.pass("tests/trybuild/pass/passes_fields_without_serde_default.rs");
    t.compile_fail("tests/trybuild/fail/missing_audit_annotation.rs");
    t.compile_fail("tests/trybuild/fail/empty_no_repair_reason.rs");
    t.compile_fail("tests/trybuild/fail/unknown_audit_key.rs");
    // DAY-100 TST-v0.3-01 — the class-detector also fires on
    // enum-variant fields, not just plain structs. A `SinkConfig`
    // variant that grows a `#[serde(default)]` field without a paired
    // audit annotation must fail to compile with the same error shape
    // as `missing_audit_annotation.rs` above.
    t.compile_fail("tests/trybuild/fail/sink_config_serde_default_without_audit.rs");
    // DAY-101 TST-v0.4-05 — symmetry with `empty_no_repair_reason.rs`.
    // Both empty-string shapes (`no_repair = ""` and `repair = ""`)
    // are audit-rejection cases and both must fail to compile.
    t.compile_fail("tests/trybuild/fail/empty_repair_name.rs");
    // DAY-109 TST-v0.4-01 — extends DAY-100's class-detector to the
    // five deeper persisted types in `dayseam-core` (`ArtifactPayload`,
    // `ActivityEvent`, `SyncRun`, `PerSourceState`, `LogEntry`). Each
    // production type carries the `SerdeDefaultAudit` derive as of
    // DAY-109; the fixture pairs below pin that pinning — the fail
    // case proves a future `#[serde(default)]` without a paired audit
    // annotation breaks the build, the pass case proves the same
    // shape with a documented `repair = "..."` or `no_repair = "..."`
    // compiles. One pair per type; the per-type rationale lives in
    // each fixture's module-doc.
    t.compile_fail("tests/trybuild/fail/artifact_payload_serde_default_without_audit.rs");
    t.pass("tests/trybuild/pass/artifact_payload_serde_default_with_audit.rs");
    t.compile_fail("tests/trybuild/fail/activity_event_serde_default_without_audit.rs");
    t.pass("tests/trybuild/pass/activity_event_serde_default_with_audit.rs");
    t.compile_fail("tests/trybuild/fail/sync_run_serde_default_without_audit.rs");
    t.pass("tests/trybuild/pass/sync_run_serde_default_with_audit.rs");
    t.compile_fail("tests/trybuild/fail/per_source_state_serde_default_without_audit.rs");
    t.pass("tests/trybuild/pass/per_source_state_serde_default_with_audit.rs");
    t.compile_fail("tests/trybuild/fail/log_entry_serde_default_without_audit.rs");
    t.pass("tests/trybuild/pass/log_entry_serde_default_with_audit.rs");
}
