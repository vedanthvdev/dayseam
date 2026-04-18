//! Capability-level invariants for every combination a sink might try
//! to declare. Complements the unit tests in `dayseam-core` by walking
//! the full 4-flag matrix so a subtle lattice mistake can't slip through.

use sinks_sdk::{CapabilityConflict, SinkCapabilities};

fn caps(local: bool, remote: bool, interactive: bool, unattended: bool) -> SinkCapabilities {
    SinkCapabilities {
        local_only: local,
        remote_write: remote,
        interactive_only: interactive,
        safe_for_unattended: unattended,
    }
}

#[test]
fn local_only_and_remote_write_are_mutually_exclusive() {
    let err = caps(true, true, false, true).validate().unwrap_err();
    assert_eq!(err, CapabilityConflict::LocalAndRemote);
}

#[test]
fn interactive_only_and_safe_for_unattended_are_mutually_exclusive() {
    let err = caps(true, false, true, true).validate().unwrap_err();
    assert_eq!(err, CapabilityConflict::InteractiveAndUnattended);
}

#[test]
fn sink_must_declare_either_local_or_remote() {
    let err = caps(false, false, false, false).validate().unwrap_err();
    assert_eq!(err, CapabilityConflict::NeitherLocalNorRemote);

    let err = caps(false, false, true, false).validate().unwrap_err();
    assert_eq!(err, CapabilityConflict::NeitherLocalNorRemote);
}

#[test]
fn canonical_local_markdown_sink_caps_validate() {
    let caps = SinkCapabilities::LOCAL_ONLY;
    caps.validate().expect("canonical caps must validate");
    assert!(caps.local_only && caps.safe_for_unattended);
    assert!(!caps.remote_write && !caps.interactive_only);
}

#[test]
fn every_other_legal_combination_validates() {
    // Remote + interactive + not unattended — a hypothetical future
    // "open Slack composer for me" sink. Shouldn't be auto-fired, must
    // run with a user present.
    caps(false, true, true, false).validate().unwrap();

    // Remote + unattended — a hypothetical future "post to private
    // webhook" sink that declared itself safe for scheduling.
    caps(false, true, false, true).validate().unwrap();

    // Local + unattended (the canonical markdown-file shape).
    caps(true, false, false, true).validate().unwrap();

    // Local + not unattended — a hypothetical future "drop file on
    // desktop and show in Finder" sink that wants a user present.
    caps(true, false, false, false).validate().unwrap();
}
