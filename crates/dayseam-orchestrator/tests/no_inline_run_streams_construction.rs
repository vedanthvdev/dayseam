//! ARC-03 guard: `RunStreams` construction is centralised.
//!
//! The Phase 2 review flagged that `generate_report` and `save_report`
//! each owned `RunStreams` slightly differently, and the Phase 3 plan
//! (Task 4.1) closed the divergence by routing both paths through
//! [`dayseam_events::RunStreams::with_progress`].
//!
//! This test grep-locks that convergence so a future refactor cannot
//! silently reintroduce bespoke destructuring. It asserts:
//!
//! 1. The orchestrator's production sources contain exactly two call
//!    sites of `RunStreams::with_progress(` — one in `generate.rs`,
//!    one in `save.rs`.
//! 2. Production orchestrator code never constructs `RunStreams`
//!    directly via `RunStreams::new(` or `RunStreams {` outside the
//!    canonical helper path.
//!
//! Tests, docs, and the `dayseam-events` crate itself are explicitly
//! allowed to use `RunStreams::new` (it is the underlying primitive
//! every unit test opens a pair of channels with).

use std::fs;
use std::path::{Path, PathBuf};

fn orchestrator_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read_rs_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read_dir orchestrator/src") {
        let entry = entry.expect("read_dir entry");
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let content = fs::read_to_string(&path).expect("read source file");
            out.push((path, content));
        }
    }
    out
}

#[test]
fn with_progress_is_called_exactly_in_generate_and_save() {
    let sources = read_rs_files(&orchestrator_src());
    let mut hits: Vec<String> = Vec::new();
    for (path, content) in &sources {
        if content.contains("RunStreams::with_progress(") {
            hits.push(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("<unknown>")
                    .to_owned(),
            );
        }
    }
    hits.sort();
    assert_eq!(
        hits,
        vec!["generate.rs".to_owned(), "save.rs".to_owned()],
        "ARC-03: `RunStreams::with_progress` must live on exactly \
         generate.rs + save.rs in orchestrator/src; observed: {hits:?}"
    );
}

#[test]
fn no_inline_run_streams_construction_in_orchestrator_src() {
    let sources = read_rs_files(&orchestrator_src());
    let mut offenders: Vec<String> = Vec::new();
    for (path, content) in &sources {
        // Skip nothing in production src — every file under
        // orchestrator/src is production code, tests live under
        // ../tests/.
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>");
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            // Reject direct construction via `RunStreams::new(` — the
            // helper covers this. Struct-literal construction
            // (`RunStreams { ... }`) would need every field named, so
            // we also reject `RunStreams {` on a non-comment line.
            if line.contains("RunStreams::new(") || line.contains("RunStreams {") {
                offenders.push(format!("{name}:{}: {}", lineno + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ARC-03: production orchestrator code must construct \
         `RunStreams` only through `RunStreams::with_progress`. \
         Offenders:\n  {}",
        offenders.join("\n  "),
    );
}
