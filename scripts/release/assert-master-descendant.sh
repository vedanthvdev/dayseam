#!/usr/bin/env bash
# assert-master-descendant.sh — verify the `chore(release)` commit on
# HEAD is a direct descendant of origin/master's current tip, so a
# subsequent `git push origin HEAD:master` is guaranteed to be a
# fast-forward. Exits 0 on success, 1 on drift, 2 on unusable repo
# state.
#
# Why this script exists
# ======================
#
# The release workflow checks out a PR's `merge_commit_sha` at job
# start, builds the universal DMG (5–10 minutes on macos-latest),
# and only then commits the version bump on top of the checked-out
# SHA. If a second release landed on master during the build, the
# commit we are about to push has a parent that is stale relative
# to current master. The previous implementation used
# `git push --force-with-lease=master:<captured-sha>` which only
# asserts tip equality — it does NOT require the commit being pushed
# to be a descendant of the tip. As a result, Run B's chore commit
# (parented on Run A's pre-push master SHA) could force-push
# through the lease and silently orphan Run A's commit from master's
# linear history. v0.8.1's `chore(release)` 56f0c2a was orphaned
# exactly this way during the v0.8.1 → v0.8.2 pair — the tag still
# points at the commit but `git log origin/master` skips it
# entirely. Issue tracked as DAY-164.
#
# This script is the belt-and-braces assertion that runs BEFORE the
# push. Paired with a plain `git push origin HEAD:master` (no force,
# no lease), it guarantees the failure mode is a loud workflow
# failure with an operator-actionable message instead of a silent
# commit loss. The workflow-level `concurrency: release-master-push`
# group is the primary guard; this script is the secondary check
# that catches the residual race where Run B's `merge_commit_sha`
# (stable at PR merge time) is stale relative to current master.
#
# Usage
# =====
#
#   assert-master-descendant.sh
#
# The script takes no arguments. It assumes cwd is a git checkout
# with an `origin` remote that has a `master` branch reachable. In
# CI this is guaranteed by `actions/checkout@v4` plus the explicit
# `git fetch` below.
#
# Exit codes
# ==========
#
#   0  HEAD^ == origin/master tip; push will be fast-forward
#   1  drift detected; push would be non-fast-forward
#   2  git repo state unusable (no origin/master, or HEAD has no
#      parent) — can't make the assertion, refuse to proceed

set -euo pipefail

# Refresh the remote-tracking ref. Without this, origin/master
# reflects whatever the runner's initial fetch captured, which may
# be stale if the concurrency gate released us behind a different
# run that pushed a few seconds ago. `--quiet` keeps the log clean;
# a real fetch failure (network, auth) still surfaces because
# `set -e` catches the non-zero exit.
if ! git fetch --quiet origin master 2>/dev/null; then
  echo "::error::assert-master-descendant.sh: 'git fetch origin master' failed; cannot assert push safety." >&2
  exit 2
fi

if ! current_tip="$(git rev-parse origin/master 2>/dev/null)"; then
  echo "::error::assert-master-descendant.sh: could not resolve origin/master. Is the repo missing an 'origin' remote with a 'master' branch?" >&2
  exit 2
fi

# HEAD^ is the first parent of our chore(release) commit. A root
# commit (no parent) cannot be asserted against; the release
# workflow should never produce one, but we fail loudly here rather
# than miscount '^' as some implicit sentinel.
if ! our_parent="$(git rev-parse HEAD^ 2>/dev/null)"; then
  echo "::error::assert-master-descendant.sh: HEAD has no parent (root commit?). The release workflow checks out a PR merge and commits on top of it; seeing a root commit here means the checkout or commit step is broken." >&2
  exit 2
fi

echo "==> origin/master tip: $current_tip"
echo "==> our chore parent:  $our_parent"

if [[ "$current_tip" != "$our_parent" ]]; then
  cat >&2 <<EOF
::error::Master has advanced since this release run checked out. Our chore commit's parent ($our_parent) is not the current origin/master tip ($current_tip). Refusing to push — doing so would silently orphan the intervening commit(s) from master's linear history (the same failure mode that lost v0.8.1's chore(release) 56f0c2a during the v0.8.1 → v0.8.2 pair, tracked as DAY-164).
::error::Remediation: wait for the concurrent run to finish, confirm master's state with 'git log origin/master', and if this release still needs to ship, re-trigger via 'gh workflow run release.yml' on master's current tip. The extract-release-notes preflight will fail fast if [Unreleased] is empty, which is the correct signal that the release has already been cut.
EOF
  exit 1
fi

echo "==> Parent check passed: push will be a fast-forward."
