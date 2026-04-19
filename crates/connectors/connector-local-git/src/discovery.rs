//! Discover git repositories under the configured scan roots.
//!
//! The walk is deliberately simple: breadth-first, bounded by
//! [`DiscoveryConfig::max_depth`] (default 6) and
//! [`DiscoveryConfig::max_roots`] (default 512), and deterministic in
//! its output order. Hidden directories other than `.git` itself are
//! skipped so we don't dive into editor caches, `node_modules`, etc.
//!
//! The function returns a [`DiscoveryOutcome`] rather than a plain
//! `Vec<DiscoveredRepo>` so the caller can tell the difference between
//! "finished cleanly" and "truncated at `max_roots`" and emit the
//! corresponding `LogEvent::Warn` with code
//! [`dayseam_core::error_codes::LOCAL_GIT_TOO_MANY_ROOTS`].

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use dayseam_core::error_codes;
use dayseam_core::DayseamError;

/// Tuning knobs for [`discover_repos`]. Defaults are the values in
/// Task 2 of the Phase 2 plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryConfig {
    /// Maximum depth beneath a scan root to descend into. Depth `0`
    /// means "only look at the scan root itself"; the default `6`
    /// handles typical `~/Code/<org>/<repo>` layouts comfortably
    /// without chasing node_modules trees.
    pub max_depth: usize,
    /// Hard cap on the number of repositories returned. If discovery
    /// hits the cap it truncates and the outcome carries
    /// `truncated = true` so the connector can emit a warning log.
    pub max_roots: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_roots: 512,
        }
    }
}

/// One discovered git repository. `label` is always the final path
/// component so renderers have something to show without having to
/// know about the user's filesystem layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRepo {
    pub path: PathBuf,
    pub label: String,
}

impl DiscoveredRepo {
    fn from_path(path: PathBuf) -> Self {
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self { path, label }
    }
}

/// Result of a discovery pass. `truncated == true` means the walk
/// aborted early because it hit [`DiscoveryConfig::max_roots`];
/// callers should warn the user so they can either raise the cap or
/// narrow their scan roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryOutcome {
    pub repos: Vec<DiscoveredRepo>,
    pub truncated: bool,
}

/// Walk every scan root and return the approved git repositories
/// beneath them. Paths are returned in stable lexicographic order so
/// downstream consumers (progress bars, artefact listings, tests)
/// see the same order every run.
///
/// Errors:
///
/// * [`DayseamError::Io`] with
///   [`error_codes::LOCAL_GIT_REPO_NOT_FOUND`] if any scan root does
///   not exist on disk. We fail fast here because a missing scan
///   root is almost always a config bug (the user moved a folder
///   since setup) and silently skipping it would hide that.
pub fn discover_repos(
    scan_roots: &[PathBuf],
    config: DiscoveryConfig,
) -> Result<DiscoveryOutcome, DayseamError> {
    let mut out: Vec<DiscoveredRepo> = Vec::new();
    let mut truncated = false;

    for root in scan_roots {
        if !root.exists() {
            return Err(DayseamError::Io {
                code: error_codes::LOCAL_GIT_REPO_NOT_FOUND.to_string(),
                path: Some(root.clone()),
                message: format!("scan root does not exist: {}", root.display()),
            });
        }

        let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
        queue.push_back((root.clone(), 0));

        while let Some((dir, depth)) = queue.pop_front() {
            if out.len() >= config.max_roots {
                truncated = true;
                break;
            }

            if is_git_repo(&dir) {
                out.push(DiscoveredRepo::from_path(dir));
                continue;
            }

            if depth >= config.max_depth {
                continue;
            }

            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let mut children: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| !is_hidden_non_git(p))
                .collect();
            children.sort_by(|a, b| compare_paths(a, b));

            for child in children {
                queue.push_back((child, depth + 1));
            }
        }

        if truncated {
            break;
        }
    }

    out.sort_by(|a, b| compare_paths(&a.path, &b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(DiscoveryOutcome {
        repos: out,
        truncated,
    })
}

fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists() || path.join("HEAD").is_file()
}

fn is_hidden_non_git(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|name| name != ".git" && name.starts_with('.'))
        .unwrap_or(false)
}

fn compare_paths(a: &Path, b: &Path) -> Ordering {
    a.as_os_str().cmp(b.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path).unwrap();
    }

    fn make_repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        touch(&dir.join(".git/HEAD"));
    }

    #[test]
    fn discovery_finds_repos_and_ignores_non_repos() {
        let root = tempdir().unwrap();
        let base = root.path();

        make_repo(&base.join("alpha"));
        make_repo(&base.join("beta"));
        make_repo(&base.join("deep/nested/gamma"));
        std::fs::create_dir_all(base.join("not-a-repo")).unwrap();
        std::fs::create_dir_all(base.join("also-not")).unwrap();

        let outcome = discover_repos(
            &[base.to_path_buf()],
            DiscoveryConfig {
                max_depth: 6,
                max_roots: 100,
            },
        )
        .unwrap();

        let paths: Vec<_> = outcome.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                base.join("alpha"),
                base.join("beta"),
                base.join("deep/nested/gamma"),
            ]
        );
        assert!(!outcome.truncated);
    }

    #[test]
    fn discovery_is_stable_across_runs() {
        let root = tempdir().unwrap();
        let base = root.path();
        for name in ["zeta", "alpha", "mu", "beta"] {
            make_repo(&base.join(name));
        }

        let a = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let b = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        assert_eq!(a.repos, b.repos);
    }

    #[test]
    fn discovery_truncates_at_max_roots() {
        let root = tempdir().unwrap();
        let base = root.path();
        for i in 0..10 {
            make_repo(&base.join(format!("r{i}")));
        }

        let outcome = discover_repos(
            &[base.to_path_buf()],
            DiscoveryConfig {
                max_depth: 6,
                max_roots: 3,
            },
        )
        .unwrap();

        assert_eq!(outcome.repos.len(), 3);
        assert!(outcome.truncated);
    }

    #[test]
    fn discovery_respects_max_depth() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("a/b/c/d/e/deep"));

        let shallow = discover_repos(
            &[base.to_path_buf()],
            DiscoveryConfig {
                max_depth: 2,
                max_roots: 100,
            },
        )
        .unwrap();
        assert!(
            shallow.repos.is_empty(),
            "depth 2 should not reach the repo"
        );

        let deep = discover_repos(
            &[base.to_path_buf()],
            DiscoveryConfig {
                max_depth: 6,
                max_roots: 100,
            },
        )
        .unwrap();
        assert_eq!(deep.repos.len(), 1);
    }

    #[test]
    fn discovery_rejects_missing_scan_root() {
        let err = discover_repos(
            &[PathBuf::from("/definitely/does/not/exist/dayseam-test")],
            DiscoveryConfig::default(),
        )
        .expect_err("missing root");
        match err {
            DayseamError::Io { code, .. } => {
                assert_eq!(code, error_codes::LOCAL_GIT_REPO_NOT_FOUND)
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn discovery_skips_hidden_directories_but_not_dot_git() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("visible"));
        std::fs::create_dir_all(base.join(".hidden/nested")).unwrap();
        touch(&base.join(".hidden/nested/.git/HEAD"));

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(paths, vec![base.join("visible")]);
    }

    #[test]
    fn discovery_dedupes_if_scan_roots_overlap() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("shared/repo"));

        let out = discover_repos(
            &[base.to_path_buf(), base.join("shared")],
            DiscoveryConfig::default(),
        )
        .unwrap();
        assert_eq!(out.repos.len(), 1);
    }
}
