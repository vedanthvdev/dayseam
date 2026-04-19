//! Single-writer lock sentinel to keep two concurrent `write()` calls
//! from racing on the same target path.
//!
//! The sentinel is a sibling file at `{target}.dayseam.lock`. It is
//! created with `OpenOptions::new().create_new(true)`, which fails
//! atomically on both POSIX and Windows if the file already exists —
//! no external crate required. The second concurrent writer observes
//! [`io::ErrorKind::AlreadyExists`] and the adapter surfaces a
//! [`dayseam_core::DayseamError::Io`] with
//! [`dayseam_core::error_codes::SINK_FS_CONCURRENT_WRITE`] instead of
//! interleaving its rename with the first writer's.
//!
//! ## Why a sentinel file, not `fs2::FileExt::try_lock_exclusive`
//!
//! `fs2` would pull a crate we otherwise don't need. A sentinel is
//! trivial, language-level, and — crucially — survives process death
//! unchanged (stale locks are then caught by
//! [`crate::atomic::sweep_orphans`], which also sweeps `.dayseam.lock`
//! files older than `STALE_TMP_AGE`).

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use tracing::warn;

/// Extension appended to the target path to form the sentinel.
pub(crate) const LOCK_SUFFIX: &str = ".dayseam.lock";

/// RAII guard that holds the sentinel for the lifetime of one write.
/// Drop removes the sentinel; if the process crashes before drop, the
/// sentinel file is left behind and cleared by the next init-time
/// orphan sweep.
#[derive(Debug)]
pub(crate) struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    /// Sentinel path for `target`. Public to the module so the orphan
    /// sweep can match on it.
    pub(crate) fn path_for(target: &Path) -> PathBuf {
        let mut s = target.as_os_str().to_owned();
        s.push(LOCK_SUFFIX);
        PathBuf::from(s)
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path) {
            if err.kind() != io::ErrorKind::NotFound {
                warn!(
                    target = "sink-markdown-file",
                    path = %self.path.display(),
                    error = %err,
                    "failed to release lock sentinel; manual cleanup may be required"
                );
            }
        }
    }
}

/// Reason an attempt to acquire the lock failed.
#[derive(Debug)]
pub(crate) enum LockError {
    /// Sentinel already exists — another writer is either in flight or
    /// crashed without cleanup. Callers surface this as
    /// `SINK_FS_CONCURRENT_WRITE`.
    AlreadyHeld,
    /// An unexpected filesystem error prevented us from trying.
    /// Surfaced as `SINK_FS_NOT_WRITABLE` at the adapter boundary.
    Io(io::Error),
}

impl From<io::Error> for LockError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::AlreadyExists {
            Self::AlreadyHeld
        } else {
            Self::Io(err)
        }
    }
}

/// Try to acquire the lock for `target`. Returns `Err(LockError::AlreadyHeld)`
/// if a concurrent writer owns it, `Err(LockError::Io)` on any other
/// filesystem error, and a [`LockGuard`] on success.
pub(crate) fn acquire(target: &Path) -> Result<LockGuard, LockError> {
    let path = LockGuard::path_for(target);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(LockGuard { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_creates_sentinel_and_drop_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Dayseam 2026-04-18.md");
        fs::write(&target, b"").unwrap();
        let lock_path = LockGuard::path_for(&target);
        {
            let _guard = acquire(&target).expect("first acquire succeeds");
            assert!(
                lock_path.exists(),
                "sentinel must exist while the guard is alive"
            );
        }
        assert!(
            !lock_path.exists(),
            "sentinel must be removed on LockGuard drop"
        );
    }

    #[test]
    fn second_acquire_returns_already_held() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Dayseam 2026-04-18.md");
        fs::write(&target, b"").unwrap();
        let _first = acquire(&target).expect("first acquire succeeds");
        let err = acquire(&target).expect_err("second acquire must fail");
        assert!(matches!(err, LockError::AlreadyHeld));
    }

    #[test]
    fn path_for_appends_suffix_to_target() {
        let target = Path::new("/tmp/Dayseam 2026-04-18.md");
        let got = LockGuard::path_for(target);
        assert_eq!(got, Path::new("/tmp/Dayseam 2026-04-18.md.dayseam.lock"));
    }
}
