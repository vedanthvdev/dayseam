#!/usr/bin/env bash
# bump-version.sh — single-source-of-truth VERSION writer for the
# Dayseam release workflow.
#
# What it does (and does not do):
#
#   - DOES read the current VERSION file and the most recent git tag
#     matching `v*` (or the `DAYSEAM_PREV_VERSION` env override for
#     tests that run outside a git repo).
#   - DOES compute the target version by applying the requested semver
#     level (patch / minor / major) to that *previous* version.
#   - DOES rewrite VERSION, the workspace `[workspace.package].version`
#     in root Cargo.toml, and the `"version"` field in
#     apps/desktop/src-tauri/tauri.conf.json when the target differs
#     from what's currently in the tree.
#   - DOES print the resolved target version to stdout so the
#     workflow can capture it via `$(bump-version.sh minor)`.
#   - DOES NOT run `git commit`, `git tag`, or `git push`. Those
#     happen in the release workflow so the script stays testable in
#     isolation (the test harness exercises it outside a real git
#     repo).
#
# Idempotency contract: running twice with the same inputs leaves the
# working tree unchanged on the second run. The release workflow
# relies on this so a manual retry after a transient CI failure is
# safe. Task 9's capstone PR also relies on it — the PR flips VERSION
# to 0.1.0 manually for a reviewable diff, and the post-merge
# workflow's `bump-version.sh minor` call sees the tree is already at
# the target and makes no further changes.
#
# DAY-102 (v0.4.1 hotfix for DOGFOOD-v0.4-01): the idempotency check
# used to live at the *script* level — "if TARGET == VERSION file,
# skip all three writers." That fired on every capstone PR that
# pre-bumped only `VERSION` (not `Cargo.toml` / `tauri.conf.json`),
# which is exactly the capstone-PR authoring pattern the v0.3 and
# v0.4 releases both followed. Result: the workflow ran, the writer
# block was skipped, and `tauri build` embedded the stale
# `tauri.conf.json` version into Info.plist. Both `v0.3.0` and
# `v0.4.0` DMGs shipped with `CFBundleShortVersionString = 0.2.1`
# as a consequence. The fix is to always run all three writers and
# move the idempotency check *inside* each writer (compare file
# contents before `mv`, skip the rename if the awk pass produced no
# change). That preserves the "second run is a no-op" contract while
# making it impossible for a stale Cargo.toml or tauri.conf.json to
# survive the release workflow.
#
# Usage:
#   bump-version.sh {patch|minor|major|none}
#
# Exit codes:
#   0  success (prints target version to stdout)
#   1  bad usage / unknown semver level
#   2  VERSION file missing or malformed

set -euo pipefail

# Repo-relative file paths. Resolved from REPO_ROOT so the script can
# be invoked from any cwd (release.yml runs it from the checkout root,
# but test-bump-version.sh runs it against a staged tempdir).
REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"
VERSION_FILE="${REPO_ROOT}/VERSION"
CARGO_TOML="${REPO_ROOT}/Cargo.toml"
TAURI_CONF="${REPO_ROOT}/apps/desktop/src-tauri/tauri.conf.json"

if [[ $# -ne 1 ]]; then
  echo "usage: bump-version.sh {patch|minor|major|none}" >&2
  exit 1
fi

LEVEL="$1"

if [[ ! -f "$VERSION_FILE" ]]; then
  echo "bump-version.sh: VERSION file not found at $VERSION_FILE" >&2
  exit 2
fi

CURRENT="$(tr -d '[:space:]' <"$VERSION_FILE")"
if [[ ! "$CURRENT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "bump-version.sh: VERSION contents '$CURRENT' are not a valid semver triple" >&2
  exit 2
fi

# Resolve the "previous" version we're bumping from. Priority:
#   1. DAYSEAM_PREV_VERSION env var (used by test-bump-version.sh).
#   2. Most recent git tag matching v*.*.* on the current HEAD.
#   3. The version currently in the VERSION file.
#
# The test harness uses the env override because it creates scratch
# directories that aren't git repos; real CI runs inside the checkout
# and reads the tag. If neither is set (e.g. the very first release
# before v0.1.0 is tagged) we fall back to the VERSION file value,
# which on a clean master is `0.0.0`.
if [[ -n "${DAYSEAM_PREV_VERSION:-}" ]]; then
  PREV="$DAYSEAM_PREV_VERSION"
elif git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  PREV="$(git -C "$REPO_ROOT" tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -n 1 | sed 's/^v//')"
  if [[ -z "$PREV" ]]; then
    PREV="$CURRENT"
  fi
else
  PREV="$CURRENT"
fi

if [[ ! "$PREV" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "bump-version.sh: resolved previous version '$PREV' is not a valid semver triple" >&2
  exit 2
fi

IFS='.' read -r PREV_MAJOR PREV_MINOR PREV_PATCH <<<"$PREV"

case "$LEVEL" in
  patch)
    TARGET="${PREV_MAJOR}.${PREV_MINOR}.$((PREV_PATCH + 1))"
    ;;
  minor)
    TARGET="${PREV_MAJOR}.$((PREV_MINOR + 1)).0"
    ;;
  major)
    TARGET="$((PREV_MAJOR + 1)).0.0"
    ;;
  none)
    TARGET="$CURRENT"
    echo "$TARGET"
    exit 0
    ;;
  *)
    echo "bump-version.sh: unknown level '$LEVEL' (expected patch|minor|major|none)" >&2
    exit 1
    ;;
esac

# In-place file rewrites use awk with a temp file. awk's range
# handling is identical on GNU (Linux CI) and BSD (local macOS) awks,
# and sidesteps the BSD-sed `{...}` block-command dialect quirk.
#
# Per-writer idempotency (DAY-102): each writer computes the would-be
# output into a temp file, then `mv`s only if `cmp -s` says the
# content differs from the current on-disk file. That preserves the
# byte-for-byte no-op contract the idempotency test asserts while
# removing the script-level "skip all writers when TARGET == VERSION"
# shortcut that caused DOGFOOD-v0.4-01.
replace_if_different() {
  local tmp="$1"
  local dest="$2"
  if cmp -s "$tmp" "$dest"; then
    rm -f "$tmp"
  else
    mv "$tmp" "$dest"
  fi
}

write_version_file() {
  local tmp
  tmp="$(mktemp)"
  printf '%s\n' "$TARGET" >"$tmp"
  replace_if_different "$tmp" "$VERSION_FILE"
}

write_cargo_toml() {
  # Match `version = "X.Y.Z"` inside the `[workspace.package]` block
  # only. The project's crate Cargo.tomls all use
  # `version.workspace = true`, so this one line is the fan-out
  # point for every crate's published version. The `in_ws` flag opens
  # on the `[workspace.package]` header and closes on the *next*
  # TOML section header, which is exactly the semantic we want.
  local tmp
  tmp="$(mktemp)"
  awk -v target="$TARGET" '
    /^\[workspace\.package\]/ { in_ws = 1; print; next }
    /^\[/ { in_ws = 0 }
    in_ws && /^version = "[0-9]+\.[0-9]+\.[0-9]+"$/ {
      print "version = \"" target "\""
      next
    }
    { print }
  ' "$CARGO_TOML" >"$tmp"
  replace_if_different "$tmp" "$CARGO_TOML"
}

write_tauri_conf() {
  # tauri.conf.json has exactly one top-level `"version": "X.Y.Z"` —
  # no nested keys collide in the current schema, so a single
  # unanchored match on that whole token is safe.
  local tmp
  tmp="$(mktemp)"
  awk -v target="$TARGET" '
    /"version": "[0-9]+\.[0-9]+\.[0-9]+"/ {
      sub(/"version": "[0-9]+\.[0-9]+\.[0-9]+"/, "\"version\": \"" target "\"")
    }
    { print }
  ' "$TAURI_CONF" >"$tmp"
  replace_if_different "$tmp" "$TAURI_CONF"
}

# Always run all three writers. Each is internally no-op when the
# file is already at TARGET, so re-runs remain byte-for-byte
# idempotent, but a tree where only *some* files are at TARGET (the
# capstone-PR pattern that caused DOGFOOD-v0.4-01) gets repaired
# rather than silently skipped.
write_version_file
write_cargo_toml
write_tauri_conf

echo "$TARGET"
