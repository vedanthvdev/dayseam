//! Discover git repositories under the configured scan roots.
//!
//! The walk is deliberately simple: breadth-first, bounded by
//! [`DiscoveryConfig::max_depth`] (default 6) and
//! [`DiscoveryConfig::max_roots`] (default 512), and deterministic in
//! its output order. Hidden directories other than `.git` itself are
//! skipped so we don't dive into editor caches, `node_modules`, etc.
//!
//! # Filesystem hygiene (DOGFOOD-v0.4-02, DOGFOOD-v0.4-04)
//!
//! Two additional guards keep the walker from tripping macOS TCC
//! prompts for unrelated apps (Photos, Music, …) and from over-
//! counting spurious `.git` folders:
//!
//! 1. **Symlinks are never followed.** Discovery uses
//!    [`std::fs::DirEntry::file_type`] (the non-chasing stat) and
//!    skips any entry whose type is a symlink. A symlink under the
//!    user's scan root could otherwise escape into `~/Library`,
//!    `~/Pictures/Photos Library.photoslibrary`, or similar and
//!    surface a TCC pop-up that has nothing to do with the
//!    user-selected folder. DOGFOOD-v0.4-02.
//! 2. **Known macOS media bundles and TCC-protected sibling names are
//!    skipped by name.** See [`is_macos_protected_name`] for the
//!    exact list. These are directories the user will essentially
//!    never version-control inside, and where any `read_dir` produces
//!    the dogfood-reported "allow access to Photos" / "allow access
//!    to Music" prompts.
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

            // DAY-103 F-3: `read_dir` failures on the scan root
            // itself used to be swallowed, so a transient permission
            // denial (APFS FileVault unlock race, paused network
            // mount, Spotlight holding a lock, a `chmod` mistake)
            // produced an empty outcome that the caller's reconcile
            // pass would then commit as "nuke every tracked repo".
            // We now surface scan-root-level errors as
            // [`DayseamError::Io`]; deeper `read_dir` failures are
            // still tolerated (files the user can't stat mid-walk
            // are normal enough that failing the whole sync would
            // be the worse default).
            let is_scan_root = depth == 0;
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(err) if is_scan_root => {
                    return Err(DayseamError::Io {
                        code: error_codes::LOCAL_GIT_REPO_NOT_FOUND.to_string(),
                        path: Some(dir.clone()),
                        message: format!("cannot read scan root {}: {err}", dir.display()),
                    });
                }
                Err(_) => continue,
            };

            // DOGFOOD-v0.4-02/04: we deliberately check three cheap
            // predicates *before* recursing into a directory. Each
            // filter avoids a class of bug the v0.4 dogfood hit:
            //
            // * `file_type().is_dir()` uses the non-chasing stat from
            //   the `DirEntry` itself — so symlinks are excluded
            //   (symlinks to dirs otherwise read `is_dir() == true`
            //   via `Path::is_dir` which *does* chase). Excluding
            //   symlinks also means a user-controlled symlink inside
            //   the scan root cannot escape into TCC-protected
            //   siblings like `~/Pictures/Photos Library.photoslibrary`.
            // * `is_hidden_non_git` keeps editor caches, `node_modules`
            //   shadows, and similar clutter out of the walk.
            // * `is_macos_protected_name` skips the known-bad directory
            //   names that trigger Photos/Music/Movies TCC prompts
            //   — see the module-level comment for the rationale.
            let mut children: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter_map(|e| {
                    let ft = e.file_type().ok()?;
                    if !ft.is_dir() || ft.is_symlink() {
                        return None;
                    }
                    let path = e.path();
                    if is_hidden_non_git(&path) {
                        return None;
                    }
                    if is_macos_protected_name(&path) {
                        // DAY-103 F-5: leave a breadcrumb so a user
                        // who expected a repo to show up but didn't
                        // see it can ask "why?" and a log export
                        // answers the question. `debug!` keeps this
                        // off the default log drawer UI and inside
                        // `RUST_LOG=debug` traces only.
                        tracing::debug!(
                            path = %path.display(),
                            "pruned macos-protected directory from local-git discovery"
                        );
                        return None;
                    }
                    Some(path)
                })
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

/// Return `true` if `path` is the root of a git repository.
///
/// DOGFOOD-v0.4-04: the previous implementation was
/// `path.join(".git").exists() || path.join("HEAD").is_file()`, which
/// over-counted in two ways: any empty `.git/` directory that lacked
/// `HEAD`/`objects`/`refs` still registered as a repo, and any
/// directory that happened to contain a top-level `HEAD` text file
/// (common in protocol sample trees, go module caches, etc.) was
/// misclassified as a bare repo. Users dogfooding v0.4 saw the UI
/// chip report "`12 repos`" for a tree that actually had seven real
/// checkouts because several `.git/` stubs in stale clones still
/// existed. This function now requires one of three well-formed
/// layouts:
///
/// 1. **Worktree** — `.git` is a regular file (contents usually read
///    `gitdir: /path/to/real/.git/worktrees/<name>`). `git worktree
///    add` creates exactly this shape. We accept any `.git` file
///    without parsing the pointer because a readable pointer file is
///    a strong enough signal and the walker does not need to resolve
///    it.
/// 2. **Regular repo** — `.git/` is a directory that contains a
///    regular `HEAD` file. This is the default layout from `git
///    init`.
/// 3. **Bare repo** — the path itself carries `HEAD` plus `objects/`
///    *and* `refs/`. The triad is the minimum a real bare repo
///    always has; requiring all three eliminates the false-positive
///    "random directory with a `HEAD` file" class.
fn is_git_repo(path: &Path) -> bool {
    // DAY-103 F-6: `Path::is_file()` / `Path::is_dir()` chase
    // symlinks. A directory with a `.git` symlink pointing at
    // `~/Pictures/Photos Library.photoslibrary/internals/HEAD` would
    // otherwise still pass the worktree check below via the chase —
    // reintroducing the exact TCC-prompt class DOGFOOD-v0.4-02
    // closed. We unconditionally use the non-chasing
    // `symlink_metadata` and bail if the seal is a symlink.
    fn is_plain_file(p: &Path) -> bool {
        std::fs::symlink_metadata(p)
            .map(|m| m.file_type().is_file())
            .unwrap_or(false)
    }
    fn is_plain_dir(p: &Path) -> bool {
        std::fs::symlink_metadata(p)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false)
    }

    let dot_git = path.join(".git");
    // Worktree shape (`.git` is a regular file, not a directory).
    if is_plain_file(&dot_git) {
        return true;
    }
    // Regular repo shape (`.git/` dir with a `HEAD` file).
    if is_plain_dir(&dot_git) && is_plain_file(&dot_git.join("HEAD")) {
        return true;
    }
    // Bare repo shape — require the full triad so a directory with a
    // spurious `HEAD` file does not qualify.
    if is_plain_file(&path.join("HEAD"))
        && is_plain_dir(&path.join("objects"))
        && is_plain_dir(&path.join("refs"))
    {
        return true;
    }
    false
}

fn is_hidden_non_git(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|name| name != ".git" && name.starts_with('.'))
        .unwrap_or(false)
}

/// Return `true` if `path`'s final component is one of the macOS
/// directories known to host TCC-protected content or media package
/// bundles. Walking into any of these is how the v0.4 dogfood report
/// hit "Photos app wants permission" / "Music app wants permission"
/// pop-ups that have nothing to do with git discovery.
///
/// The list is deliberately conservative — it only skips directories
/// that have **very clear non-code semantics** on macOS. A user who
/// wants to version-control something called `Music` inside their
/// code tree can still do so by scanning one level deeper; this filter
/// only catches the exact `~/Music`, `~/Pictures`, `~/Movies`, etc.
/// names at the top of a user's home, plus the well-known Apple media
/// package extensions.
///
/// Kept as a name-based check (rather than an absolute-path check) so
/// it works on the synthetic `tempdir()` paths the test suite uses.
/// DOGFOOD-v0.4-02.
///
/// DAY-119: expanded the list to cover the four modern TCC-protected
/// top-level `$HOME` folders (`Desktop`, `Documents`, `Downloads`,
/// `Public`). Starting in macOS Mojave the operating system prompts
/// the user the first time an app reads any of these, and every
/// additional protected folder the walker touches is a *separate*
/// prompt. A user who picked `~/Code` as a scan root and whose
/// `~/Code` tree contained symlinks or named children pointing into
/// Desktop/Documents/Downloads would see exactly the "5 or 6
/// pop-ups" cascade reported against v0.6.1. These names are also
/// essentially never the interesting roots for a code tree — a
/// scan root that legitimately lives inside `~/Documents` is picked
/// explicitly via the folder picker and the filter does not apply
/// there (it only skips *children* named this way, not the scan
/// root itself).
fn is_macos_protected_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    // Exact-name matches: these are the common macOS TCC-protected
    // top-level folders in `$HOME`, plus the two internals that
    // reliably produce runaway walks. `Library` is where Apple keeps
    // application support / caches / Mail downloads / etc.; scanning
    // it is never what a git-repo-discovery user wants.
    matches!(
        name,
        "Library"
            | "Pictures"
            | "Music"
            | "Movies"
            // DAY-119: modern macOS TCC-protected homes. Apple
            // started prompting for each of these in Mojave
            // (10.14). Walking into any of them from a scan root
            // the user did not explicitly pick produces a
            // per-folder TCC prompt cascade.
            | "Desktop"
            | "Documents"
            | "Downloads"
            | "Public"
            // System/network mounts that are not in a user's home
            // but can be crossed via symlinks or by selecting `/` as
            // a scan root.
            | "Volumes"
            | ".Trash"
            // Performance sink, not TCC — but a `node_modules` tree
            // is never the right place to discover a distinct repo.
            | "node_modules"
    ) || has_protected_bundle_extension(name)
}

fn has_protected_bundle_extension(name: &str) -> bool {
    // macOS "package" bundles whose internals are private to the
    // owning app. We don't descend into any of them. The extensions
    // are matched case-insensitively because macOS is case-preserving
    // but not case-sensitive by default.
    const EXTS: &[&str] = &[
        ".photoslibrary",
        ".musiclibrary",
        ".tvlibrary",
        ".imovielibrary",
        ".app",
        ".bundle",
        ".framework",
    ];
    let lower = name.to_ascii_lowercase();
    EXTS.iter().any(|ext| lower.ends_with(ext))
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

    // ---- DOGFOOD-v0.4-04: tightened `is_git_repo` predicate ----

    /// An empty `.git/` directory (no `HEAD`, no `objects`, no `refs`)
    /// used to be enough to count a path as a repo. It no longer is.
    /// The fixture matches "user deleted contents of `.git/` manually"
    /// and "stale clone interrupted mid-init" — both real v0.4 dogfood
    /// shapes.
    #[test]
    fn discovery_ignores_empty_dot_git_without_head() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("real"));
        // `.git/` exists but is empty — prior logic counted it.
        std::fs::create_dir_all(base.join("junk/.git")).unwrap();

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![base.join("real")],
            "empty .git/ directory must not count as a repo",
        );
    }

    /// A lone `HEAD` file at a directory root is not a bare repo —
    /// the bare-repo shape is the full `HEAD + objects/ + refs/`
    /// triad. A go module cache, a vendored protocol sample, or
    /// arbitrary content that happens to contain a `HEAD` text file
    /// should not show up in discovery.
    #[test]
    fn discovery_ignores_bare_head_without_objects_and_refs() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("real"));
        // Only `HEAD` file, no `objects/` or `refs/`.
        touch(&base.join("fake-bare/HEAD"));

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(paths, vec![base.join("real")]);
    }

    /// A proper bare repo (`HEAD + objects/ + refs/`) IS still a
    /// repo. This test pins the positive side of the predicate so a
    /// future tightening doesn't regress bare-repo support.
    #[test]
    fn discovery_recognises_proper_bare_repo_triad() {
        let root = tempdir().unwrap();
        let base = root.path();
        let bare = base.join("proper-bare");
        std::fs::create_dir_all(bare.join("objects")).unwrap();
        std::fs::create_dir_all(bare.join("refs")).unwrap();
        touch(&bare.join("HEAD"));

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        assert_eq!(out.repos.len(), 1);
        assert_eq!(out.repos[0].path, bare);
    }

    /// `git worktree add` creates a linked checkout where `.git` is a
    /// **file** containing `gitdir: …`, not a directory. We must
    /// still count that as a repo.
    #[test]
    fn discovery_recognises_worktree_with_dot_git_file() {
        let root = tempdir().unwrap();
        let base = root.path();
        let wt = base.join("linked-worktree");
        std::fs::create_dir_all(&wt).unwrap();
        // `.git` is a regular file — matches the worktree shape.
        std::fs::write(
            wt.join(".git"),
            "gitdir: /tmp/synthetic/.git/worktrees/linked-worktree\n",
        )
        .unwrap();

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        assert_eq!(out.repos.len(), 1);
        assert_eq!(out.repos[0].path, wt);
    }

    // ---- DOGFOOD-v0.4-02: walker hygiene ----

    /// Symlinks — even to valid repos — are not followed. If the
    /// user has a symlink from `~/code/offsite -> ~/Pictures/Photos
    /// Library.photoslibrary`, discovery must not cross into that
    /// tree (which would trigger a macOS Photos TCC prompt).
    ///
    /// This test is gated on Unix symlink support; the walker does
    /// the right thing on Windows for free because symlinks there
    /// are typically privileged to create.
    #[cfg(unix)]
    #[test]
    fn discovery_does_not_follow_symlinks() {
        let root = tempdir().unwrap();
        let base = root.path();
        // Real repo outside the scan root.
        let offsite = tempdir().unwrap();
        make_repo(offsite.path());
        // Symlink inside the scan root pointing at the offsite repo.
        std::os::unix::fs::symlink(offsite.path(), base.join("offsite-link")).unwrap();
        // And a real repo inside, so we can confirm the walker
        // didn't just trip on some unrelated error.
        make_repo(&base.join("here"));

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![base.join("here")],
            "symlinked directory must not be followed during discovery",
        );
    }

    /// DAY-103 F-6: a contrived but constructible attack on the
    /// walker — a directory whose `.git` entry is *itself* a
    /// symlink into a TCC-protected tree. The walker's entry-time
    /// symlink check doesn't see it (because `.git` is pointed at
    /// during the repo predicate, not during the directory walk),
    /// so the repo predicate has to use `symlink_metadata` directly.
    #[cfg(unix)]
    #[test]
    fn is_git_repo_rejects_dot_git_that_is_a_symlink_to_elsewhere() {
        let root = tempdir().unwrap();
        let base = root.path();
        // Real `.git` content in an unrelated location.
        let offsite = tempdir().unwrap();
        touch(&offsite.path().join("HEAD"));
        // A candidate directory whose `.git` is a symlink into that
        // offsite content. Under the old `is_file()` / `is_dir()`
        // chase, the repo predicate would return true here.
        let candidate = base.join("candidate");
        std::fs::create_dir_all(&candidate).unwrap();
        std::os::unix::fs::symlink(offsite.path(), candidate.join(".git")).unwrap();

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        assert!(
            out.repos.is_empty(),
            "a candidate whose `.git` is a symlink must not count as a repo"
        );
    }

    /// Directories whose names are known macOS media / TCC-protected
    /// locations are pruned before `read_dir` recurses into them.
    /// This is the structural fix for the Photos/Music pop-ups users
    /// reported when scanning broad roots.
    #[test]
    fn discovery_skips_macos_protected_directory_names() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("code/dayseam"));
        // A `Music` dir at the same level must be skipped, even if
        // it contains a `.git/` child (the user should not be
        // accidentally scanning their iTunes library, and walking
        // into this name is what surfaces the macOS Music TCC
        // prompt).
        make_repo(&base.join("Music/should-not-appear"));
        make_repo(&base.join("Pictures/should-not-appear"));
        // Package-bundle extension.
        make_repo(&base.join("MyLibrary.photoslibrary/should-not-appear"));
        // `Library` (user's ~/Library equivalent).
        make_repo(&base.join("Library/should-not-appear"));

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![base.join("code/dayseam")],
            "macOS TCC/media protected names must prune the walk",
        );
    }

    /// DAY-119: users on v0.6.1 reported a cascade of 5-6 macOS
    /// "allow access to Documents/Desktop/Downloads" pop-ups after
    /// picking a scan root — one prompt per TCC-protected folder
    /// the walker touched. The modern TCC protection list includes
    /// `Desktop`, `Documents`, `Downloads`, and `Public` (in
    /// addition to the Mojave-era `Pictures/Music/Movies`), so the
    /// walker must prune all of them by name or every v0.6.x ship
    /// will re-introduce the same prompt cascade.
    ///
    /// This test fails on a fix-revert: removing any one of those
    /// four names from the `matches!` arm immediately lets a
    /// child with that name appear in the results and the
    /// assertion below flags it.
    #[test]
    fn discovery_skips_modern_macos_tcc_protected_names() {
        let root = tempdir().unwrap();
        let base = root.path();
        make_repo(&base.join("code/dayseam"));
        // Each of the four modern-TCC names hosts a `.git` child
        // that WOULD be discovered if the walker descended into it.
        for protected in ["Desktop", "Documents", "Downloads", "Public"] {
            make_repo(&base.join(protected).join("should-not-appear"));
        }

        let out = discover_repos(&[base.to_path_buf()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![base.join("code/dayseam")],
            "modern macOS TCC-protected names (Desktop/Documents/Downloads/Public) must prune the walk to avoid the 5-6 pop-up cascade reported in v0.6.1",
        );
    }

    /// DAY-119 companion: when the user *explicitly* picks a scan
    /// root named one of the protected TCC names (e.g. they keep
    /// their code in `~/Documents/Code`), the filter applies only
    /// to children of the walk — not to the scan root itself.
    /// This preserves the "user knows what they picked" contract.
    #[test]
    fn discovery_accepts_protected_name_as_explicit_scan_root() {
        let root = tempdir().unwrap();
        let base = root.path();
        // The scan root itself is named `Documents`.
        let scan_root = base.join("Documents");
        make_repo(&scan_root.join("repo"));

        let out = discover_repos(&[scan_root.clone()], DiscoveryConfig::default()).unwrap();
        let paths: Vec<_> = out.repos.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![scan_root.join("repo")],
            "explicitly selecting a scan root whose name matches a protected name must still discover repos under it",
        );
    }
}
