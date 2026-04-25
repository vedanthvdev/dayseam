#!/usr/bin/env bash
# test-assert-master-descendant.sh — bash test suite for
# assert-master-descendant.sh.
#
# What the suite proves:
#
#   1. Happy path: our chore commit sits on top of origin/master's
#      current tip. This is the normal `semver:patch` release shape
#      (serialized runs, no concurrent merges) and the assertion
#      must accept it with exit 0.
#   2. Drift detected: origin/master has advanced beyond our HEAD^
#      (another release pushed, or a direct push happened while we
#      were building). This is the exact scenario the v0.8.1 →
#      v0.8.2 pair hit and the whole reason the script exists; the
#      assertion must reject with exit 1 so the release workflow
#      fails loudly instead of silently orphaning the intervening
#      commit via a force-push-with-lease.
#   3. Missing origin remote: the checkout step is broken or the
#      runner image started without a remote. The script must exit
#      2 (distinct from the drift exit 1 so the workflow log can
#      distinguish "couldn't check" from "checked and failed").
#   4. Fresh origin with no master branch: similar to #3 but the
#      remote exists and has no master ref yet (e.g. brand-new
#      mirror). Also exit 2.
#   5. HEAD is the root commit: the commit step is broken (shouldn't
#      be possible in practice because the workflow checks out a
#      merge commit before committing), but failing loudly here
#      keeps the script from mis-reporting a root commit as "drift".
#      Exit 2.
#
# The fixtures use two local bare repos (origin + local clone) so
# `git fetch origin master` exercises real ref refresh behavior;
# mocking would be cheaper but wouldn't catch a regression in how
# the script spells the fetch / rev-parse invocations.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/release/assert-master-descendant.sh"

if [[ ! -x "$SCRIPT" ]]; then
  echo "test-assert-master-descendant.sh: helper not executable at $SCRIPT" >&2
  exit 1
fi

TESTS_RUN=0
TESTS_FAILED=0

# Mint a fresh scratch area with a bare origin + a working clone.
# Each test gets its own so state doesn't leak.
new_scratch() {
  local scratch="$1"
  mkdir -p "$scratch/local" "$scratch/origin.git"
  git init --bare --quiet --initial-branch=master "$scratch/origin.git" >/dev/null
  git init --quiet --initial-branch=master "$scratch/local" >/dev/null
  git -C "$scratch/local" config user.email "test@example.com"
  git -C "$scratch/local" config user.name "test"
  git -C "$scratch/local" remote add origin "$scratch/origin.git"
}

# Commit an arbitrary file in the local clone. Keeps each commit
# distinct so `git rev-parse` returns distinguishable SHAs.
commit_in_local() {
  local scratch="$1"
  local msg="$2"
  local file="$3"
  local content="$4"
  printf '%s\n' "$content" >"$scratch/local/$file"
  git -C "$scratch/local" add "$file"
  git -C "$scratch/local" commit --quiet -m "$msg"
}

# Simulate a third-party push to origin/master by cloning the bare
# origin into a second working tree, committing there, and pushing.
# This is closer to the real-world failure mode (another GitHub
# Actions run pushed) than directly writing to the bare repo.
push_other_commit_to_origin() {
  local scratch="$1"
  local msg="$2"
  local other
  other="$(mktemp -d)"
  git clone --quiet "$scratch/origin.git" "$other/other" >/dev/null
  git -C "$other/other" config user.email "other@example.com"
  git -C "$other/other" config user.name "other"
  printf '%s\n' "drift" >"$other/other/drift.txt"
  git -C "$other/other" add drift.txt
  git -C "$other/other" commit --quiet -m "$msg"
  git -C "$other/other" push --quiet origin master
  rm -rf "$other"
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

# Run the helper against a given working directory. Returns the
# exit code so tests can assert on it. stdout+stderr are suppressed
# because the helper's error messages are part of the log stream we
# already assert on in-workflow; here we only care about the exit
# contract.
run_helper() {
  local cwd="$1"
  local exit_code
  set +e
  (cd "$cwd" && "$SCRIPT" >/dev/null 2>&1)
  exit_code=$?
  set -e
  echo "$exit_code"
}

test_happy_path() {
  local scratch="$1"
  new_scratch "$scratch"
  commit_in_local "$scratch" "init" "a.txt" "1"
  git -C "$scratch/local" push --quiet origin master
  commit_in_local "$scratch" "chore(release): v0.0.1" "VERSION" "0.0.1"
  # HEAD^ now equals origin/master (init commit); the push would
  # be a fast-forward.
  local exit_code
  exit_code="$(run_helper "$scratch/local")"
  if [[ "$exit_code" != "0" ]]; then
    echo "  FAIL: expected exit 0 on happy path, got $exit_code" >&2
    return 1
  fi
  return 0
}

test_drift_detected() {
  local scratch="$1"
  new_scratch "$scratch"
  commit_in_local "$scratch" "init" "a.txt" "1"
  git -C "$scratch/local" push --quiet origin master
  commit_in_local "$scratch" "chore(release): v0.0.1" "VERSION" "0.0.1"
  # Someone else pushed a different commit to origin/master while
  # our chore commit was sitting locally. Classic v0.8.1 → v0.8.2
  # shape.
  push_other_commit_to_origin "$scratch" "concurrent release"
  local exit_code
  exit_code="$(run_helper "$scratch/local")"
  if [[ "$exit_code" != "1" ]]; then
    echo "  FAIL: expected exit 1 on drift, got $exit_code" >&2
    return 1
  fi
  return 0
}

test_missing_origin_remote() {
  local scratch="$1"
  # No 'origin' remote at all. The `git fetch origin master` inside
  # the helper must fail and the helper must exit 2 (distinct from
  # the drift exit 1).
  mkdir -p "$scratch/local"
  git init --quiet --initial-branch=master "$scratch/local" >/dev/null
  git -C "$scratch/local" config user.email "t@e.com"
  git -C "$scratch/local" config user.name "t"
  commit_in_local "$scratch" "init" "a.txt" "1"
  local exit_code
  exit_code="$(run_helper "$scratch/local")"
  if [[ "$exit_code" != "2" ]]; then
    echo "  FAIL: expected exit 2 on missing origin, got $exit_code" >&2
    return 1
  fi
  return 0
}

test_origin_without_master() {
  local scratch="$1"
  # Origin remote exists but has no master branch yet. This is the
  # "brand-new mirror" shape; the helper must exit 2 because it
  # can't rev-parse origin/master.
  mkdir -p "$scratch/local" "$scratch/origin.git"
  git init --bare --quiet --initial-branch=master "$scratch/origin.git" >/dev/null
  git init --quiet --initial-branch=master "$scratch/local" >/dev/null
  git -C "$scratch/local" config user.email "t@e.com"
  git -C "$scratch/local" config user.name "t"
  git -C "$scratch/local" remote add origin "$scratch/origin.git"
  commit_in_local "$scratch" "init" "a.txt" "1"
  commit_in_local "$scratch" "chore" "b.txt" "2"
  # Do NOT push to origin. The helper's fetch will succeed (empty)
  # but rev-parse origin/master will fail.
  local exit_code
  exit_code="$(run_helper "$scratch/local")"
  if [[ "$exit_code" != "2" ]]; then
    echo "  FAIL: expected exit 2 on missing origin/master, got $exit_code" >&2
    return 1
  fi
  return 0
}

test_root_commit_fails_cleanly() {
  local scratch="$1"
  new_scratch "$scratch"
  commit_in_local "$scratch" "init" "a.txt" "1"
  git -C "$scratch/local" push --quiet origin master
  # HEAD is a root commit (no parent). `git rev-parse HEAD^` must
  # fail and the helper must exit 2 rather than misinterpret the
  # empty parent as matching origin/master.
  local exit_code
  exit_code="$(run_helper "$scratch/local")"
  if [[ "$exit_code" != "2" ]]; then
    echo "  FAIL: expected exit 2 on root commit, got $exit_code" >&2
    return 1
  fi
  return 0
}

run_test "happy path: HEAD^ == origin/master tip" test_happy_path
run_test "drift: origin/master advanced beyond our parent" test_drift_detected
run_test "missing origin remote fails cleanly" test_missing_origin_remote
run_test "origin without master branch fails cleanly" test_origin_without_master
run_test "root commit (no parent) fails cleanly" test_root_commit_fails_cleanly

echo
echo "Ran $TESTS_RUN tests, $TESTS_FAILED failed"

if [[ "$TESTS_FAILED" -gt 0 ]]; then
  exit 1
fi
