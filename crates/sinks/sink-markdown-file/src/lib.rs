//! `sink-markdown-file` — Dayseam's v0.1 local markdown-file sink.
//!
//! The sink writes a rendered [`dayseam_core::ReportDraft`] to one or
//! two filesystem roots as a single markdown file per date, wrapping
//! the rendered content in a machine-parseable *marker block* so the
//! same file can be re-opened on a later day without clobbering the
//! user's prose. Obsidian vaults are supported by virtue of being "a
//! folder with `.md` files in it"; no vault-specific coupling.
//!
//! # The contract, in short
//!
//! Given a target path and a `ReportDraft` for date `D`:
//!
//! 1. If the target file does not exist, write a new file containing
//!    exactly one marker block for `D`.
//! 2. If it exists and already contains a marker block for `D`, **replace
//!    only that block**; every other byte (other marker blocks for
//!    other dates, user prose between blocks, trailing newlines) stays
//!    byte-for-byte identical.
//! 3. If it exists and does **not** contain a block for `D`, append a
//!    new block at the end of the file preserving the file's existing
//!    trailing newline convention.
//!
//! Writes are atomic (temp file + rename) and guarded by a lock
//! sentinel so a second concurrent write against the same target
//! refuses rather than risks interleaving renames.
//!
//! # Why the sink, not the report engine, renders markdown
//!
//! [`dayseam_report`](../dayseam_report/index.html) produces a
//! structured [`dayseam_core::ReportDraft`] — sections, bullets,
//! evidence — and deliberately leaves markdown assembly to the output
//! layer. That keeps the engine pure and testable, and lets each sink
//! decide its own markdown dialect (bullet glyph, heading style, block
//! wrapper) without coordinating with the renderer.
//!
//! # Layering
//!
//! `sink-markdown-file` depends only on `sinks-sdk` and `dayseam-core`.
//! The `no_cross_crate_leak` integration test asserts the absence of
//! any edge to `dayseam-db`, `dayseam-secrets`, `dayseam-report`,
//! `connectors-sdk`, or a connector crate; any such edge would be a
//! layering bug.

#![deny(missing_docs)]

mod adapter;
mod atomic;
mod frontmatter;
mod lock;
mod markdown;
mod markers;

pub use adapter::MarkdownFileSink;
