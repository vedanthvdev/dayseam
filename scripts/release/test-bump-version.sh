#!/usr/bin/env bash
# test-bump-version.sh — bash test suite for bump-version.sh.
#
# Run locally (`scripts/release/test-bump-version.sh`) or from CI
# (`.github/workflows/ci.yml` shell-script job). Exits 0 on success;
# non-zero on the first failing assertion with a diagnostic.
#
# What the suite proves:
#
#   1. semver:none leaves the working tree unchanged (byte-for-byte).
#   2. semver:patch bumps 0.0.0 -> 0.0.1 in all three tracked files.
#   3. semver:minor bumps 0.0.0 -> 0.1.0 in all three tracked files.
#   4. semver:major bumps 0.0.0 -> 1.0.0 in all three tracked files.
#   5. Running semver:minor twice in a row leaves the second run a
#      byte-for-byte no-op (idempotency contract the release workflow
#      depends on for retries).
#   6. When the working tree is already at the target version (Task 9's
#      capstone pre-bump case — VERSION pre-flipped to 0.1.0 in the
#      PR, then semver:minor runs post-merge), the script prints the
#      target and makes no file changes.
#   7. When VERSION is pre-bumped to target but Cargo.toml and
#      tauri.conf.json are still stale (the real capstone-PR pattern
#      the v0.3 and v0.4 release arcs actually follow), the script
#      repairs Cargo.toml and tauri.conf.json to match instead of
#      silently shipping them stale. This is DOGFOOD-v0.4-01 — both
#      v0.3.0 and v0.4.0 DMGs shipped with Info.plist stamped 0.2.1
#      because the old script-level `TARGET == CURRENT` gate skipped
#      the writer block whenever a capstone PR only pre-flipped
#      VERSION.
#
# Each test runs against a freshly staged scratch directory so tests
# cannot leak state into each other or into the real repo.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/release/bump-version.sh"

if [[ ! -x "$SCRIPT" ]]; then
  echo "test-bump-version.sh: bump-version.sh is not executable at $SCRIPT" >&2
  exit 1
fi

TESTS_RUN=0
TESTS_FAILED=0

# The three files every test asserts about. Hashing these by path
# (instead of walking the scratch dir with `find`) keeps the harness
# robust against sandbox environments where `find` is restricted.
tracked_files() {
  local root="$1"
  printf '%s\n' \
    "${root}/VERSION" \
    "${root}/Cargo.toml" \
    "${root}/apps/desktop/src-tauri/tauri.conf.json"
}

hash_tracked() {
  local root="$1"
  tracked_files "$root" | xargs shasum 2>/dev/null | sort
}

# Stage a scratch repo containing just enough files for bump-version.sh
# to operate on. Writes VERSION, a minimal Cargo.toml with a
# [workspace.package] block, and a minimal tauri.conf.json.
stage_scratch_repo() {
  local root="$1"
  local start_version="$2"
  mkdir -p "${root}/apps/desktop/src-tauri"
  printf '%s\n' "$start_version" >"${root}/VERSION"
  cat >"${root}/Cargo.toml" <<EOF
[workspace]
resolver = "2"
members = []

[workspace.package]
version = "${start_version}"
edition = "2021"
EOF
  cat >"${root}/apps/desktop/src-tauri/tauri.conf.json" <<EOF
{
  "\$schema": "https://schema.tauri.app/config/2",
  "productName": "Dayseam",
  "version": "${start_version}",
  "identifier": "dev.dayseam.desktop"
}
EOF
}

# Assert that VERSION, Cargo.toml, and tauri.conf.json all pin the
# given version. Fails the test on the first mismatch so the
# diagnostic names the file.
assert_all_files_match() {
  local root="$1"
  local expected="$2"
  local version_file_contents cargo_match tauri_match

  version_file_contents="$(tr -d '[:space:]' <"${root}/VERSION")"
  if [[ "$version_file_contents" != "$expected" ]]; then
    echo "  FAIL: VERSION file is '$version_file_contents', expected '$expected'" >&2
    return 1
  fi

  cargo_match="$(sed -n '/^\[workspace\.package\]/,/^\[/{ s/^version = "\([0-9.]*\)"$/\1/p; }' "${root}/Cargo.toml" | head -n 1)"
  if [[ "$cargo_match" != "$expected" ]]; then
    echo "  FAIL: Cargo.toml [workspace.package].version is '$cargo_match', expected '$expected'" >&2
    return 1
  fi

  tauri_match="$(sed -n 's/.*"version": "\([0-9.]*\)".*/\1/p' "${root}/apps/desktop/src-tauri/tauri.conf.json" | head -n 1)"
  if [[ "$tauri_match" != "$expected" ]]; then
    echo "  FAIL: tauri.conf.json version is '$tauri_match', expected '$expected'" >&2
    return 1
  fi

  return 0
}

# Wrapper so the per-test bookkeeping (pass/fail count, scratch dir
# cleanup) stays in one place. Each test function below is
# self-contained: stage, invoke, assert.
run_test() {
  local name="$1"
  local fn="$2"
  local scratch
  scratch="$(mktemp -d)"
  trap "rm -rf '$scratch'" RETURN

  TESTS_RUN=$((TESTS_RUN + 1))
  echo "• $name"
  if (set +e; "$fn" "$scratch"; exit $?); then
    echo "  ok"
  else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    echo "  FAILED"
  fi
  trap - RETURN
  rm -rf "$scratch"
}

test_semver_none_is_noop() {
  local root="$1"
  stage_scratch_repo "$root" "0.3.7"
  local before_hash
  before_hash="$(hash_tracked "$root")"
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.3.7" "$SCRIPT" none)"
  if [[ "$out" != "0.3.7" ]]; then
    echo "  FAIL: expected stdout '0.3.7', got '$out'" >&2
    return 1
  fi
  local after_hash
  after_hash="$(hash_tracked "$root")"
  if [[ "$before_hash" != "$after_hash" ]]; then
    echo "  FAIL: semver:none mutated the working tree" >&2
    return 1
  fi
  return 0
}

test_patch_bumps_to_next_patch() {
  local root="$1"
  stage_scratch_repo "$root" "0.0.0"
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" patch)"
  if [[ "$out" != "0.0.1" ]]; then
    echo "  FAIL: expected stdout '0.0.1', got '$out'" >&2
    return 1
  fi
  assert_all_files_match "$root" "0.0.1"
}

test_minor_bumps_to_next_minor() {
  local root="$1"
  stage_scratch_repo "$root" "0.0.0"
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" minor)"
  if [[ "$out" != "0.1.0" ]]; then
    echo "  FAIL: expected stdout '0.1.0', got '$out'" >&2
    return 1
  fi
  assert_all_files_match "$root" "0.1.0"
}

test_major_bumps_to_next_major() {
  local root="$1"
  stage_scratch_repo "$root" "0.0.0"
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" major)"
  if [[ "$out" != "1.0.0" ]]; then
    echo "  FAIL: expected stdout '1.0.0', got '$out'" >&2
    return 1
  fi
  assert_all_files_match "$root" "1.0.0"
}

test_idempotent_double_invoke() {
  local root="$1"
  stage_scratch_repo "$root" "0.0.0"
  REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" minor >/dev/null
  local after_first
  after_first="$(hash_tracked "$root")"
  REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" minor >/dev/null
  local after_second
  after_second="$(hash_tracked "$root")"
  if [[ "$after_first" != "$after_second" ]]; then
    echo "  FAIL: second invocation mutated the working tree" >&2
    return 1
  fi
  assert_all_files_match "$root" "0.1.0"
}

test_prebumped_tree_is_noop() {
  # Simulates Task 9's capstone when *all three* files are
  # pre-bumped in lock-step: the PR manually flipped VERSION,
  # Cargo.toml, and tauri.conf.json to 0.1.0 for reviewer
  # visibility, then post-merge release.yml runs bump-version.sh
  # minor. Previous tag is still v0.0.0, so the target computed
  # from prev is 0.1.0 — which already matches every file.
  # Expected: no file changes, stdout is 0.1.0.
  local root="$1"
  stage_scratch_repo "$root" "0.1.0"
  local before_hash
  before_hash="$(hash_tracked "$root")"
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" minor)"
  if [[ "$out" != "0.1.0" ]]; then
    echo "  FAIL: expected stdout '0.1.0', got '$out'" >&2
    return 1
  fi
  local after_hash
  after_hash="$(hash_tracked "$root")"
  if [[ "$before_hash" != "$after_hash" ]]; then
    echo "  FAIL: pre-bumped tree was mutated" >&2
    return 1
  fi
  return 0
}

test_stale_cargo_and_tauri_are_repaired() {
  # DOGFOOD-v0.4-01 regression guard. Simulates the *real* capstone-PR
  # shape the v0.3 and v0.4 arcs actually followed: VERSION is
  # manually pre-flipped to the target so reviewers see the version
  # bump, but Cargo.toml and tauri.conf.json are left at the
  # previous release's value. Post-merge release.yml runs
  # `bump-version.sh minor` with prev=v0.0.0, so the computed
  # target is 0.1.0. Before DAY-102 this silently skipped the
  # writer block because VERSION already matched target, and the
  # stale tauri.conf.json flowed straight into Info.plist
  # (CFBundleShortVersionString). After DAY-102 each writer runs
  # unconditionally and reconciles Cargo.toml + tauri.conf.json
  # up to the target.
  local root="$1"
  mkdir -p "${root}/apps/desktop/src-tauri"
  printf '0.1.0\n' >"${root}/VERSION"
  cat >"${root}/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = []

[workspace.package]
version = "0.0.0"
edition = "2021"
EOF
  cat >"${root}/apps/desktop/src-tauri/tauri.conf.json" <<'EOF'
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Dayseam",
  "version": "0.0.0",
  "identifier": "dev.dayseam.desktop"
}
EOF
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.0.0" "$SCRIPT" minor)"
  if [[ "$out" != "0.1.0" ]]; then
    echo "  FAIL: expected stdout '0.1.0', got '$out'" >&2
    return 1
  fi
  assert_all_files_match "$root" "0.1.0"
}

test_all_files_parity_after_every_invoke() {
  # DAY-102 standing invariant: after any successful invocation,
  # VERSION, Cargo.toml, and tauri.conf.json must all pin the same
  # version string. Covers the 2x2 grid of {stale Cargo.toml ∨
  # stale tauri.conf.json} × {semver:patch ∨ semver:minor} as one
  # representative cross-check.
  local root="$1"
  # stale tauri.conf.json, VERSION + Cargo.toml at 0.4.0, bump patch.
  mkdir -p "${root}/apps/desktop/src-tauri"
  printf '0.4.0\n' >"${root}/VERSION"
  cat >"${root}/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = []

[workspace.package]
version = "0.4.0"
edition = "2021"
EOF
  cat >"${root}/apps/desktop/src-tauri/tauri.conf.json" <<'EOF'
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Dayseam",
  "version": "0.2.1",
  "identifier": "dev.dayseam.desktop"
}
EOF
  local out
  out="$(REPO_ROOT="$root" DAYSEAM_PREV_VERSION="0.4.0" "$SCRIPT" patch)"
  if [[ "$out" != "0.4.1" ]]; then
    echo "  FAIL: expected stdout '0.4.1', got '$out'" >&2
    return 1
  fi
  assert_all_files_match "$root" "0.4.1"
}

run_test "semver:none is a byte-for-byte no-op" test_semver_none_is_noop
run_test "semver:patch bumps 0.0.0 -> 0.0.1" test_patch_bumps_to_next_patch
run_test "semver:minor bumps 0.0.0 -> 0.1.0" test_minor_bumps_to_next_minor
run_test "semver:major bumps 0.0.0 -> 1.0.0" test_major_bumps_to_next_major
run_test "double invoke with same inputs is idempotent" test_idempotent_double_invoke
run_test "pre-bumped tree matching target is a no-op" test_prebumped_tree_is_noop
run_test "stale Cargo.toml + tauri.conf.json are repaired (DOGFOOD-v0.4-01)" test_stale_cargo_and_tauri_are_repaired
run_test "all three files agree after every invocation" test_all_files_parity_after_every_invoke

echo
echo "Ran $TESTS_RUN tests, $TESTS_FAILED failed"

if [[ "$TESTS_FAILED" -gt 0 ]]; then
  exit 1
fi
