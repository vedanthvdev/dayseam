#!/usr/bin/env bash
# extract-release-notes.sh — slice a single version's section out of
# CHANGELOG.md and print it to stdout.
#
# Why this helper exists (vs. inlining the awk in release.yml):
#
# The release workflow uses the extracted slice in two places —
# once as the pre-flight "are there any release notes?" gate, and
# once as the body of the published GitHub Release. Doing the
# extraction twice with inline awk drifted into two slightly
# different filters during Phase 3 Task 6, and the v0.1.0 capstone
# then surfaced a third subtlety: Task 9's CHANGELOG pattern closes
# `[Unreleased]` → `[$VERSION] - YYYY-MM-DD` inside the PR itself,
# so `[Unreleased]` is deliberately empty on release day. The
# workflow needs to prefer `[Unreleased]` (the normal
# contributor-authored shape) but fall back to `[$VERSION]` when
# the PR has already moved the notes into a version-named block.
# Centralising that policy in one place — and unit-testing it —
# is the clearest way to keep the gate and the publish step in sync.
#
# Usage:
#   extract-release-notes.sh <version> [<changelog_path>]
#
#   <version>         The target release version, e.g. "0.1.0". The
#                     helper tries `[Unreleased]` first; if that
#                     section is empty it falls back to
#                     `[$version]`.
#   <changelog_path>  Optional; defaults to CHANGELOG.md in the repo
#                     root.
#
# Output:
#   On success, prints the section body to stdout (without the
#   `## [X]` header line itself) and prints the selected source
#   (`Unreleased` or the version string) to stderr.
#
# Exit codes:
#   0  success (non-empty body printed to stdout)
#   1  bad usage
#   2  no non-empty section found in either location

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: extract-release-notes.sh <version> [<changelog_path>]" >&2
  exit 1
fi

VERSION="$1"
CHANGELOG="${2:-${REPO_ROOT}/CHANGELOG.md}"

if [[ ! -f "$CHANGELOG" ]]; then
  echo "extract-release-notes.sh: changelog not found at $CHANGELOG" >&2
  exit 1
fi

# Slice the body between `## [<label>]` and the next top-level `## [`
# heading. `label` is a literal string; dots in a version number are
# treated as literal characters because the awk regex escapes them.
#
# The `next` on the opening match deliberately swallows the header
# line itself so the printed body starts with the entry's first
# content line.
extract_section() {
  local label="$1"
  if [[ "$label" == "Unreleased" ]]; then
    awk '
      /^## \[Unreleased\]/ { in_block = 1; next }
      /^## \[/ { in_block = 0 }
      in_block { print }
    ' "$CHANGELOG"
  else
    # Match `## [<version>]` at the very start of a line, with the
    # version's dots escaped so `0.1.0` doesn't spuriously match
    # `0X1X0`.
    awk -v v="$label" '
      BEGIN {
        gsub(/\./, "\\.", v)
        pattern = "^## \\[" v "\\]"
      }
      $0 ~ pattern { in_block = 1; next }
      /^## \[/ { in_block = 0 }
      in_block { print }
    ' "$CHANGELOG"
  fi
}

# A section is "non-empty" if, after dropping blank lines and
# top-level `###` subheaders, there's at least one line of content
# left. That matches what humans see as "does this release say
# anything useful" and catches a common failure mode: a PR that
# added a subheader (`### Changed`) but forgot to write any bullets
# under it.
#
# The filter deliberately captures sed's full output into a
# variable instead of piping into `grep -q .`: the shell runs with
# `set -o pipefail`, and `grep -q` closes its input on the first
# match, which sends SIGPIPE to the upstream sed. On bodies large
# enough that sed is still writing when grep exits (the real
# CHANGELOG at release time is tens of KB), pipefail surfaces that
# SIGPIPE as a non-zero pipeline exit — making has_content falsely
# report "empty" for perfectly valid release notes. Buffering the
# filtered output side-steps that entirely.
has_content() {
  local filtered
  filtered="$(printf '%s' "$1" | sed -E '/^[[:space:]]*$/d' | sed -E '/^###/d')"
  [[ -n "$filtered" ]]
}

unreleased_body="$(extract_section "Unreleased")"
if has_content "$unreleased_body"; then
  printf '%s\n' "==> Using [Unreleased]" >&2
  printf '%s' "$unreleased_body"
  exit 0
fi

versioned_body="$(extract_section "$VERSION")"
if has_content "$versioned_body"; then
  printf '%s\n' "==> [Unreleased] empty, using [$VERSION]" >&2
  printf '%s' "$versioned_body"
  exit 0
fi

echo "extract-release-notes.sh: no non-empty section in [Unreleased] or [$VERSION]" >&2
exit 2
