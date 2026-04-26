//! [`MarkdownFileSink`] — the `SinkAdapter` that writes one markdown
//! file per date per destination directory, splicing into an existing
//! marker block when one for the same date is already present.
//!
//! The adapter is the thin orchestration layer between `sinks-sdk` and
//! the focused, unit-tested modules in this crate: [`crate::markers`]
//! (parse + splice), [`crate::markdown`] (draft → body text),
//! [`crate::frontmatter`] (optional YAML header), [`crate::lock`]
//! (single-writer sentinel), and [`crate::atomic`] (temp-file +
//! rename). Every one of those modules is unit-tested in isolation;
//! the integration tests in `tests/roundtrip.rs` cover the invariants
//! of the adapter as a whole.

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use dayseam_core::{
    error_codes, DayseamError, ProgressPhase, ReportDraft, SinkCapabilities, SinkConfig, SinkKind,
    WriteReceipt,
};
use sinks_sdk::{SinkAdapter, SinkCtx};
use tracing::warn;

use crate::atomic;
use crate::frontmatter::{self, FrontmatterFields};
use crate::lock;
use crate::markdown;
use crate::markers::{self, Block, MarkerAttrs, MarkerError};

/// Default filename pattern. The leading capital-D plus the ISO date
/// is what Obsidian's daily-note plugin looks for when auto-creating
/// the note; matching it by convention (not by plugin coupling) means
/// the sink Just Works for users who turn the plugin on.
pub(crate) const FILENAME_PREFIX: &str = "Dayseam ";
pub(crate) const FILENAME_SUFFIX: &str = ".md";

/// Public helper: return the default filename this sink would write
/// for `date` (e.g. `Dayseam 2026-04-20.md`). Exposed so other
/// crates — notably the desktop scheduler task — can check whether
/// a report file for a given scheduled day already exists without
/// duplicating the naming convention. Keeping this single
/// definition here means changes to the pattern propagate
/// automatically to the catch-up planner.
pub fn report_filename_for_date(date: chrono::NaiveDate) -> String {
    format!("{FILENAME_PREFIX}{date}{FILENAME_SUFFIX}")
}

/// The markdown-file sink itself. Cheap to clone — all state lives
/// behind an `Arc<Inner>`.
#[derive(Debug, Clone, Default)]
pub struct MarkdownFileSink {
    // Intentionally unit for now. Per-instance state (caches,
    // configured defaults, metrics handles) can grow here without
    // touching the trait surface.
}

impl MarkdownFileSink {
    /// Construct a new sink and sweep any orphaned temp files under
    /// `dest_dirs`. Ad-hoc callers that don't yet know their
    /// destinations should call [`Self::default`] instead and rely on
    /// the first `write()` to clean up — the sweep is idempotent.
    pub fn new(dest_dirs: &[PathBuf]) -> Self {
        for dir in dest_dirs {
            atomic::sweep_orphans(dir);
        }
        Self {}
    }
}

#[async_trait]
impl SinkAdapter for MarkdownFileSink {
    fn kind(&self) -> SinkKind {
        SinkKind::MarkdownFile
    }

    fn capabilities(&self) -> SinkCapabilities {
        SinkCapabilities::LOCAL_ONLY
    }

    async fn validate(&self, ctx: &SinkCtx, cfg: &SinkConfig) -> Result<(), DayseamError> {
        ctx.bail_if_cancelled()?;
        let SinkConfig::MarkdownFile { dest_dirs, .. } = cfg;
        for dir in dest_dirs {
            validate_dir(dir)?;
        }
        Ok(())
    }

    async fn write(
        &self,
        ctx: &SinkCtx,
        cfg: &SinkConfig,
        draft: &ReportDraft,
    ) -> Result<WriteReceipt, DayseamError> {
        ctx.bail_if_cancelled()?;
        let SinkConfig::MarkdownFile {
            dest_dirs,
            frontmatter: want_frontmatter,
            ..
        } = cfg;

        ctx.progress.send(
            None,
            ProgressPhase::Starting {
                message: format!("writing report for {}", draft.date),
            },
        );

        let filename = default_filename(draft);
        let body = markdown::render_body(draft);
        let run_id_for_marker = ctx
            .run_id
            .map(|r| r.to_string())
            .unwrap_or_else(|| draft.id.to_string());
        let new_block = Block {
            attrs: MarkerAttrs {
                date: draft.date,
                run_id: run_id_for_marker,
                template: draft.template_id.clone(),
                version: draft.template_version.clone(),
            },
            body,
        };

        let total = u32::try_from(dest_dirs.len()).unwrap_or(u32::MAX);
        let mut destinations_written: Vec<PathBuf> = Vec::new();
        let mut bytes_written: u64 = 0;
        let mut last_err: Option<DayseamError> = None;

        for (idx, dir) in dest_dirs.iter().enumerate() {
            ctx.bail_if_cancelled()?;
            let target = dir.join(&filename);
            ctx.progress.send(
                None,
                ProgressPhase::InProgress {
                    completed: u32::try_from(idx).unwrap_or(u32::MAX),
                    total: Some(total),
                    message: format!("writing {}", target.display()),
                },
            );

            match write_one(&target, &new_block, *want_frontmatter, draft) {
                Ok(n) => {
                    bytes_written += n;
                    destinations_written.push(target);
                }
                Err(err) => {
                    // Invariant #5: a failure at destination N never
                    // rolls back the successful write at destination
                    // N-1. Log and keep going, but remember the error
                    // so the adapter can surface a total failure if
                    // *every* destination failed.
                    warn!(
                        target = "sink-markdown-file",
                        destination = %target.display(),
                        error = %err,
                        "sink destination failed; preserving earlier successes"
                    );
                    last_err = Some(err);
                }
            }
        }

        if destinations_written.is_empty() {
            let err = last_err.unwrap_or_else(|| DayseamError::Internal {
                code: error_codes::SINK_FS_NOT_WRITABLE.to_string(),
                message: "no destinations were writable".to_string(),
            });
            ctx.progress.send(
                None,
                ProgressPhase::Failed {
                    code: err.code().to_string(),
                    message: err.to_string(),
                },
            );
            return Err(err);
        }

        ctx.progress.send(
            None,
            ProgressPhase::Completed {
                message: format!("wrote {} destination(s)", destinations_written.len()),
            },
        );

        Ok(WriteReceipt {
            run_id: ctx.run_id,
            sink_kind: self.kind(),
            destinations_written,
            external_refs: Vec::new(),
            bytes_written,
            written_at: Utc::now(),
        })
    }
}

/// Compose the file for `target`, including the marker-block splice
/// and optional frontmatter, and write it atomically. Returns the
/// number of bytes written.
fn write_one(
    target: &Path,
    new_block: &Block,
    want_frontmatter: bool,
    draft: &ReportDraft,
) -> Result<u64, DayseamError> {
    // Acquire the per-target lock before touching any bytes so a
    // concurrent call targeting the same path can refuse cleanly.
    let _guard = match lock::acquire(target) {
        Ok(g) => g,
        Err(lock::LockError::AlreadyHeld) => {
            return Err(DayseamError::Io {
                code: error_codes::SINK_FS_CONCURRENT_WRITE.to_string(),
                path: Some(target.to_path_buf()),
                message: format!(
                    "another write is already in flight for {}",
                    target.display()
                ),
            })
        }
        Err(lock::LockError::Io(err)) => {
            return Err(DayseamError::Io {
                code: error_codes::SINK_FS_NOT_WRITABLE.to_string(),
                path: Some(target.to_path_buf()),
                message: format!("could not acquire lock sentinel: {err}"),
            })
        }
    };

    let existing = read_target_if_any(target)?;

    // Split off existing frontmatter before parsing marker blocks.
    let (existing_frontmatter, existing_body) = match existing.as_deref() {
        Some(text) => {
            let (fm, body) = frontmatter::split(text);
            (fm.map(str::to_owned), body.to_owned())
        }
        None => (None, String::new()),
    };

    let mut doc = markers::parse(&existing_body).map_err(malformed_marker_error(target))?;
    markers::splice(&mut doc, new_block.clone());
    let rendered_body = markers::render(&doc);

    let final_bytes = if want_frontmatter {
        let fm = frontmatter::merge(
            existing_frontmatter.as_deref(),
            &FrontmatterFields {
                date: draft.date,
                template: draft.template_id.clone(),
                template_version: draft.template_version.clone(),
                generated_at: draft.generated_at,
            },
        );
        let mut s = String::with_capacity(fm.len() + rendered_body.len());
        s.push_str(&fm);
        s.push_str(&rendered_body);
        s.into_bytes()
    } else {
        // Preserve existing frontmatter verbatim if the user wrote one
        // by hand but the sink is configured without `frontmatter =
        // true`. Stripping their block silently would be surprising.
        let mut s = String::new();
        if let Some(fm) = existing_frontmatter {
            s.push_str(&fm);
        }
        s.push_str(&rendered_body);
        s.into_bytes()
    };

    atomic::atomic_write(target, &final_bytes).map_err(|err| DayseamError::Io {
        code: error_codes::SINK_FS_NOT_WRITABLE.to_string(),
        path: Some(target.to_path_buf()),
        message: format!("atomic write failed: {err}"),
    })
}

fn read_target_if_any(target: &Path) -> Result<Option<String>, DayseamError> {
    match fs::read_to_string(target) {
        Ok(s) => Ok(Some(s)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(DayseamError::Io {
            code: error_codes::SINK_FS_NOT_WRITABLE.to_string(),
            path: Some(target.to_path_buf()),
            message: format!("could not read existing target: {err}"),
        }),
    }
}

fn malformed_marker_error(target: &Path) -> impl Fn(MarkerError) -> DayseamError + '_ {
    move |err: MarkerError| DayseamError::Internal {
        code: error_codes::SINK_MALFORMED_MARKER.to_string(),
        message: format!(
            "existing marker block(s) in {} are malformed: {}",
            target.display(),
            err.describe()
        ),
    }
}

fn validate_dir(dir: &Path) -> Result<(), DayseamError> {
    let metadata = match fs::metadata(dir) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(DayseamError::InvalidConfig {
                code: error_codes::SINK_FS_DESTINATION_MISSING.to_string(),
                message: format!("destination directory does not exist: {}", dir.display()),
            });
        }
        Err(err) => {
            return Err(DayseamError::Io {
                code: error_codes::SINK_FS_NOT_WRITABLE.to_string(),
                path: Some(dir.to_path_buf()),
                message: format!("could not stat destination: {err}"),
            });
        }
    };

    if !metadata.is_dir() {
        return Err(DayseamError::InvalidConfig {
            code: error_codes::SINK_FS_DESTINATION_MISSING.to_string(),
            message: format!("destination is not a directory: {}", dir.display()),
        });
    }

    // Probe writability by creating + deleting a sentinel. Relying on
    // `metadata.permissions().readonly()` is unreliable on POSIX (it
    // reflects only the owner bit) and on Windows (it reflects only
    // the DOS read-only flag). The probe is the honest test.
    let probe = dir.join(format!(
        ".dayseam.writable_probe.{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => Err(DayseamError::Io {
            code: error_codes::SINK_FS_NOT_WRITABLE.to_string(),
            path: Some(dir.to_path_buf()),
            message: format!("destination is not writable: {err}"),
        }),
    }
}

/// Build the default `Dayseam <YYYY-MM-DD>.md` filename for a draft.
pub(crate) fn default_filename(draft: &ReportDraft) -> String {
    format!("{FILENAME_PREFIX}{}{FILENAME_SUFFIX}", draft.date)
}
