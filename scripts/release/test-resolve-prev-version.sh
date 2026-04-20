#!/usr/bin/env bash
# test-resolve-prev-version.sh — bash test suite for
# resolve-prev-version.sh.
#
# Each test stages a scratch git repo with a carefully controlled
# history (tags, parent commits, VERSION content) and asserts the
# helper's output. The scratch repos are full `git init` repos so
# the helper's real git plumbing is exercised — there's no mock
# layer between the harness and what CI will run.
#
# What the suite proves:
#
#   1. Tag-present case: a repo with `v0.5.2` tag prints `0.5.2`,
#      regardless of what VERSION at HEAD says.
#   2. First-release, pre-bumped tree: no v* tag, HEAD has
#      VERSION=0.1.0, HEAD^ has VERSION=0.0.0 (the capstone shape).
#      Helper prints 0.0.0 so `bump-version.sh minor` produces
#      0.1.0 and not 0.2.0.
#   3. First-release, no parent commit: fresh repo with only one
#      commit and VERSION=0.0.0. Helper prints 0.0.0 (the bootstrap
#      fallback).
#   4. Tag wins over HEAD^: repo has both a `v0.1.0` tag AND a
#      history where HEAD^ has VERSION=0.0.9. Helper prints 0.1.0
#      (steady state), never the parent VERSION.
#   5. Corrupt VERSION at HEAD^: repo has no tag, HEAD^ has a
#      garbage VERSION file. Helper falls through to 0.0.0 without
#      exploding.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/release/resolve-prev-version.sh"

if [[ ! -x "$SCRIPT" ]]; then
  echo "test-resolve-prev-version.sh: resolve-prev-version.sh not executable at $SCRIPT" >&2
  exit 1
fi

TESTS_RUN=0
TESTS_FAILED=0

# Stage a git repo at $root with a minimal commit that sets VERSION
# to $1. Silences `git init` / `git commit` output so the test log
# stays readable.
init_repo_at() {
  local root="$1"
  local version="$2"
  (
    cd "$root"
    git init --quiet --initial-branch=master
    git config user.email "test@dayseam.local"
    git config user.name "dayseam-test"
    printf '%s\n' "$version" >VERSION
    git add VERSION
    git commit --quiet -m "init VERSION=$version"
  )
}

# Add a second commit that bumps VERSION to $1, so HEAD^ is the
# first commit and HEAD is the bump. Used by the "first release,
# pre-bumped tree" case.
bump_repo_version() {
  local root="$1"
  local version="$2"
  (
    cd "$root"
    printf '%s\n' "$version" >VERSION
    git add VERSION
    git commit --quiet -m "bump VERSION to $version"
  )
}

# Run the helper against a scratch repo and compare its stdout to
# the expected value. Fails the test on any mismatch, including a
# non-zero exit.
assert_prev_is() {
  local root="$1"
  local expected="$2"
  local out
  if ! out="$(REPO_ROOT="$root" "$SCRIPT" 2>/dev/null)"; then
    echo "  FAIL: helper exited non-zero" >&2
    return 1
  fi
  if [[ "$out" != "$expected" ]]; then
    echo "  FAIL: expected '$expected', got '$out'" >&2
    return 1
  fi
  return 0
}

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

test_tag_present() {
  local root="$1"
  init_repo_at "$root" "0.5.2"
  (cd "$root" && git tag v0.5.2)
  # Bump the tree past the tag to prove the helper ignores the
  # current VERSION when a tag is available.
  bump_repo_version "$root" "0.9.9"
  assert_prev_is "$root" "0.5.2"
}

test_capstone_prebumped_tree() {
  local root="$1"
  init_repo_at "$root" "0.0.0"
  bump_repo_version "$root" "0.1.0"
  # No tag, HEAD^=0.0.0, HEAD=0.1.0. Helper should return 0.0.0 so
  # bump-version.sh minor produces 0.1.0 and not 0.2.0.
  assert_prev_is "$root" "0.0.0"
}

test_bootstrap_no_parent() {
  local root="$1"
  init_repo_at "$root" "0.0.0"
  # Only one commit; HEAD^ does not exist. No tag. Helper should
  # still print a valid semver (the 0.0.0 last-ditch default).
  assert_prev_is "$root" "0.0.0"
}

test_tag_wins_over_parent() {
  local root="$1"
  init_repo_at "$root" "0.0.9"
  (cd "$root" && git tag v0.1.0)
  # HEAD^ VERSION is 0.0.9 (stale); tag v0.1.0 is the truth.
  assert_prev_is "$root" "0.1.0"
}

test_corrupt_parent_version_falls_through() {
  local root="$1"
  (
    cd "$root"
    git init --quiet --initial-branch=master
    git config user.email "test@dayseam.local"
    git config user.name "dayseam-test"
    printf 'not-a-version\n' >VERSION
    git add VERSION
    git commit --quiet -m "init with garbage VERSION"
    printf '%s\n' "0.1.0" >VERSION
    git add VERSION
    git commit --quiet -m "first real VERSION"
  )
  # No tag, HEAD^ has a garbage VERSION. Helper should fall through
  # to the 0.0.0 default rather than printing 'not-a-version'.
  assert_prev_is "$root" "0.0.0"
}

run_test "tag-present case returns the tag" test_tag_present
run_test "capstone pre-bumped tree returns HEAD^ VERSION" test_capstone_prebumped_tree
run_test "bootstrap with no parent returns 0.0.0" test_bootstrap_no_parent
run_test "tag wins over parent VERSION" test_tag_wins_over_parent
run_test "corrupt parent VERSION falls through to 0.0.0" test_corrupt_parent_version_falls_through

echo
echo "Ran $TESTS_RUN tests, $TESTS_FAILED failed"

if [[ "$TESTS_FAILED" -gt 0 ]]; then
  exit 1
fi
