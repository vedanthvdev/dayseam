#!/usr/bin/env bash
# close-changelog.sh — rename `[Unreleased]` → `[$VERSION] - <today>`
# in CHANGELOG.md and insert a fresh empty `[Unreleased]` block
# above it, so the next release does not re-read the just-published
# notes.
#
# Why this script exists
# ======================
#
# Before DAY-155, release.yml bumped VERSION / Cargo.toml /
# tauri.conf.json in its `chore(release)` commit and left
# CHANGELOG.md completely alone. The paired helper
# `extract-release-notes.sh` prefers an explicit `[$VERSION]` section
# but falls back to `[Unreleased]` when none exists, which is the
# normal `semver:patch` authoring pattern. Leaving `[Unreleased]`
# populated across two consecutive releases therefore re-published
# the same notes twice — happened in the v0.7.0 → v0.8.0 pair (no
# `[0.7.0]` block exists on master at all) and again in the v0.8.1 →
# v0.8.2 pair when DAY-161 and DAY-159 landed back-to-back. The
# convention until now was "the next PR manually renames the block";
# DAY-155 moves that step into the workflow itself so the contract
# is enforced by code, not by contributor etiquette.
#
# Usage
# =====
#
#   close-changelog.sh <version> [<changelog_path>] [<today>]
#
#   <version>         The version being released, e.g. "0.8.3". Must
#                     parse as a semver triple `X.Y.Z`.
#   <changelog_path>  Optional; defaults to CHANGELOG.md at the repo
#                     root (resolved from the script's own location,
#                     matching bump-version.sh / extract-release-
#                     notes.sh).
#   <today>           Optional; defaults to today's UTC date in
#                     YYYY-MM-DD. The test harness passes an explicit
#                     value so assertions are deterministic.
#
# Behaviour
# =========
#
#   - If `[$version]` already exists in the CHANGELOG — the capstone-
#     PR authoring pattern where the PR itself pre-renamed the block
#     for a reviewable diff — the script is a no-op and exits 0. This
#     matches the priority `extract-release-notes.sh` applies on the
#     way in and lets a Task-9-style release still use this workflow
#     step without a redundant second rename.
#   - If `[Unreleased]` is absent, or contains only blank lines and
#     `### …` subheaders (the same "empty" filter the preflight helper
#     uses), the script is a no-op and exits 0. The preflight would
#     normally have refused to publish in that case; the no-op here is
#     defensive so a workflow_dispatch dry-run cannot silently erase
#     an empty-by-design block.
#   - Otherwise, the `## [Unreleased]` header is rewritten to
#     `## [$version] - $today` and a fresh empty `## [Unreleased]`
#     block is inserted above it. The bullets under the original
#     `[Unreleased]` stay in place — they move into `[$version]` by
#     virtue of the header rename, preserving every subsection and
#     ordering exactly as the contributor authored them.
#
# Idempotency contract
# ====================
#
# Running the script twice with the same inputs leaves the working
# tree byte-for-byte unchanged on the second run. The first run
# creates `[$version]`; the second run sees it already exists and
# falls into the no-op path above. This mirrors `bump-version.sh`'s
# idempotency guarantee so a manual retry after a transient CI
# failure is safe.
#
# Exit codes
# ==========
#   0  success (tree modified, or no-op because already-closed /
#      empty)
#   1  bad usage / invalid version string
#   2  CHANGELOG path not a file

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"

if [[ $# -lt 1 || $# -gt 3 ]]; then
  echo "usage: close-changelog.sh <version> [<changelog_path>] [<today>]" >&2
  exit 1
fi

VERSION="$1"
CHANGELOG="${2:-${REPO_ROOT}/CHANGELOG.md}"
TODAY="${3:-$(date -u +%Y-%m-%d)}"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "close-changelog.sh: version '$VERSION' is not a valid semver triple" >&2
  exit 1
fi

if [[ ! -f "$CHANGELOG" ]]; then
  echo "close-changelog.sh: changelog not found at $CHANGELOG" >&2
  exit 2
fi

# Fast path: the target version already has an explicit `[$version]`
# section. That is the capstone-PR authoring pattern
# `extract-release-notes.sh` preserves priority for; if the PR
# pre-closed the block, the release workflow must not rename it a
# second time. `grep -E` with escaped dots matches the same regex
# shape extract-release-notes.sh uses.
version_header_re="^## \\[${VERSION//./\\.}\\]"
if grep -qE "$version_header_re" "$CHANGELOG"; then
  printf '==> close-changelog.sh: [%s] already exists in %s; no-op.\n' "$VERSION" "$CHANGELOG" >&2
  exit 0
fi

# Detect whether `[Unreleased]` has any real content using the same
# "drop blank lines and `###` subheaders" semantics the preflight
# helper uses. Keeping the two scripts in lockstep on the definition
# of "empty" means a contributor who puts a `### Added` header with
# no bullets won't see the preflight fail the run but this step
# rename an empty block.
unreleased_body="$(awk '
  /^## \[Unreleased\]/ { in_block = 1; next }
  /^## \[/ { in_block = 0 }
  in_block { print }
' "$CHANGELOG")"

filtered="$(printf '%s' "$unreleased_body" | sed -E '/^[[:space:]]*$/d' | sed -E '/^###/d')"

if [[ -z "$filtered" ]]; then
  printf '==> close-changelog.sh: [Unreleased] in %s is empty; no-op.\n' "$CHANGELOG" >&2
  exit 0
fi

# Rewrite in one awk pass. On the `## [Unreleased]` header line, emit
# a fresh `## [Unreleased]` + blank line + the renamed
# `## [$VERSION] - $TODAY` header, then continue as normal — the
# subsection headers and bullets that followed the original
# `[Unreleased]` line are preserved verbatim and now belong to the
# renamed block.
#
# awk's `next` on the match line drops the original header from the
# output (we've already emitted its replacement above). This keeps
# the rewrite a single logical pass with no intermediate sed/awk
# chaining; BSD awk on macOS and GNU awk on Linux behave identically
# here (the matrix leg on ci.yml's `shell-scripts` catches any
# portability skew).
tmp="$(mktemp)"
awk -v ver="$VERSION" -v today="$TODAY" '
  $0 ~ /^## \[Unreleased\]/ {
    print "## [Unreleased]"
    print ""
    print "## [" ver "] - " today
    next
  }
  { print }
' "$CHANGELOG" >"$tmp"

# Belt-and-braces idempotency guard. We already checked the version
# isn't present, so this branch should not normally fire, but
# comparing the rewritten output against the source before committing
# it to disk keeps the "second run is byte-for-byte no-op" contract
# intact even if a future refactor changes the matching logic.
if cmp -s "$tmp" "$CHANGELOG"; then
  rm -f "$tmp"
  printf '==> close-changelog.sh: rewrite produced identical bytes to %s; no write.\n' "$CHANGELOG" >&2
  exit 0
fi

mv "$tmp" "$CHANGELOG"
printf '==> close-changelog.sh: closed [Unreleased] → [%s] - %s in %s\n' "$VERSION" "$TODAY" "$CHANGELOG" >&2
