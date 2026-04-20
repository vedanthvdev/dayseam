#!/usr/bin/env bash
# resolve-prev-version.sh — print the version that the next release
# should bump *from*.
#
# The release workflow calls `bump-version.sh <level>` to compute the
# next version, but that script needs to know what "previous version"
# to apply the bump to. In steady state the answer is the most recent
# `v*` git tag. On the first-ever release (v0.1.0) no tag exists yet,
# and the capstone PR may have pre-bumped the tree for reviewer
# visibility — so naively reading the VERSION file at HEAD gives
# `0.1.0`, and `bump-version.sh minor` computes `0.2.0`, which is
# nonsense. This helper encapsulates the resolution order so the
# workflow doesn't have to, and so the logic is unit-testable:
#
#   1. Most recent `v[0-9]*.[0-9]*.[0-9]*` git tag (stripped of `v`).
#   2. Contents of VERSION at HEAD^ — the state of this branch's
#      merge base *before* any version flip in the current PR. Only
#      reached when no `v*` tag exists (i.e. the first release).
#   3. `0.0.0` as a last-ditch default for repos that have neither
#      a tag nor a parent commit (initial bootstrap).
#
# The result is printed to stdout. Callers export it as
# DAYSEAM_PREV_VERSION, which bump-version.sh honours as its highest-
# priority PREV source.
#
# Usage:
#   resolve-prev-version.sh            # operates on $PWD's repo
#   REPO_ROOT=/tmp/x resolve-prev-version.sh  # overrides the repo root (tests)
#
# Exit codes:
#   0  success (prints the resolved previous version to stdout)
#   1  resolved value does not look like a semver triple (should
#      never happen in practice; defensive guard)

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"

prev=""
if git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  prev="$(git -C "$REPO_ROOT" tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -n 1 | sed 's/^v//')"
  if [[ -z "$prev" ]]; then
    if git -C "$REPO_ROOT" rev-parse HEAD^ >/dev/null 2>&1; then
      prev="$(git -C "$REPO_ROOT" show HEAD^:VERSION 2>/dev/null | tr -d '[:space:]' || true)"
    fi
  fi
fi

# If any of the lookups produced something that isn't a semver
# triple (empty, garbage contents in a parent VERSION file, etc.)
# fall through to the 0.0.0 default. Surfacing corrupt data here
# as a CI failure would be a foot-gun: the helper is only probing
# for the best guess at a starting point, and bump-version.sh will
# still validate the *target* version before writing anything.
if [[ -z "$prev" || ! "$prev" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  prev="0.0.0"
fi

printf '%s\n' "$prev"
