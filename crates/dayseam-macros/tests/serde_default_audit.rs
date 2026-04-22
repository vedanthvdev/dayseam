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
}
