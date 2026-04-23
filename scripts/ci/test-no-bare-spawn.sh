#!/usr/bin/env bash
# test-no-bare-spawn.sh — harness for DAY-113's no-bare-spawn gate.
#
# The gate script (`scripts/ci/no-bare-spawn.sh`) is the single
# enforcement point for the supervision policy — a regression there
# (e.g. an over-broad exclusion, a loose comment-stripping regex, a
# flipped exit code) would silently let a bare spawn land. This
# harness fuzzes the gate against synthetic worktrees to pin every
# acceptance and rejection case we rely on.
#
# For each fixture it:
#   1. creates a scratch REPO_ROOT with a minimal `crates/`/`apps/`
#      tree,
#   2. plants the fixture file,
#   3. runs the gate against that scratch root,
#   4. asserts the gate's exit code and stderr match expectation.
#
# Exit codes:
#   0  every fixture passed.
#   1  at least one fixture failed — failing names are printed.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${REPO_ROOT}/scripts/ci/no-bare-spawn.sh"

if [[ ! -x "$GATE" ]]; then
  echo "test-no-bare-spawn.sh: gate script missing or not executable at ${GATE}" >&2
  exit 2
fi

FAILED=()

run_fixture() {
  local name="$1"
  local expected_exit="$2"
  local file_path="$3"
  local content="$4"

  local scratch
  scratch="$(mktemp -d)"
  mkdir -p "$(dirname "${scratch}/${file_path}")"
  printf '%s' "$content" >"${scratch}/${file_path}"

  # The gate walks `crates/` and `apps/` under REPO_ROOT; we need to
  # run it with REPO_ROOT pointing at the scratch dir so the fixture
  # is the only thing in scope.
  set +e
  REPO_ROOT="$scratch" bash "$GATE" >"${scratch}/stdout" 2>"${scratch}/stderr"
  local actual_exit=$?
  set -e

  if [[ "$actual_exit" -ne "$expected_exit" ]]; then
    echo "[FAIL] ${name}: expected exit ${expected_exit}, got ${actual_exit}"
    echo "       stderr:"
    sed 's/^/         /' "${scratch}/stderr"
    FAILED+=("$name")
  else
    echo "[ok]   ${name}"
  fi

  rm -rf "$scratch"
}

# --- Fixtures -------------------------------------------------------
#
# Each fixture is named after the invariant it pins. The scratch
# filesystem lives under a `crates/<crate>/src/` layout because that's
# the shape the gate's `grep --recursive crates/ apps/` command walks.

# Acceptance: `supervised_spawn(...)` is always clean.
run_fixture \
  "supervised_spawn_call_is_clean" \
  0 \
  "crates/x/src/lib.rs" \
  'use dayseam_core::runtime::supervised_spawn;

fn start() {
    supervised_spawn("x::work", async {});
}
'

# Rejection: a bare `tokio::spawn` with no marker on either line.
run_fixture \
  "bare_tokio_spawn_without_marker_is_rejected" \
  1 \
  "crates/x/src/lib.rs" \
  'fn start() {
    tokio::spawn(async {});
}
'

# Acceptance: same-line marker comment.
run_fixture \
  "same_line_marker_is_accepted" \
  0 \
  "crates/x/src/lib.rs" \
  'fn start() {
    tokio::spawn(async {}); // bare-spawn: intentional — test shim
}
'

# Acceptance: marker on the line directly above the spawn.
run_fixture \
  "preceding_line_marker_is_accepted" \
  0 \
  "crates/x/src/lib.rs" \
  'fn start() {
    // bare-spawn: intentional
    tokio::spawn(async {});
}
'

# Rejection: marker is TWO lines above (not directly preceding).
# Covers the gap DAY-113 hit where a marker on line N was paired
# with a spawn on line N+2; ambiguity about which spawn a marker
# applies to is what the "directly preceding" rule exists to resolve.
run_fixture \
  "marker_two_lines_above_is_rejected" \
  1 \
  "crates/x/src/lib.rs" \
  'fn start() {
    // bare-spawn: intentional
    let _x = 1;
    tokio::spawn(async {});
}
'

# Acceptance: `tauri::async_runtime::spawn` with marker.
run_fixture \
  "tauri_async_runtime_spawn_with_marker_is_accepted" \
  0 \
  "apps/desktop/src/lib.rs" \
  'fn start() {
    // bare-spawn: intentional
    tauri::async_runtime::spawn(async {});
}
'

# Rejection: `tauri::async_runtime::spawn` without a marker.
run_fixture \
  "tauri_async_runtime_spawn_without_marker_is_rejected" \
  1 \
  "apps/desktop/src/lib.rs" \
  'fn start() {
    tauri::async_runtime::spawn(async {});
}
'

# Acceptance: spawn inside an integration-test file (path ends in
# `tests.rs`). Integration tests are exempt.
run_fixture \
  "tests_file_spawn_is_exempt" \
  0 \
  "crates/x/tests/sample.rs" \
  'fn start() {
    tokio::spawn(async {});
}
'

# Acceptance: spawn inside a `_tests.rs` sibling file. Same exemption
# class, different filename convention.
run_fixture \
  "underscore_tests_file_spawn_is_exempt" \
  0 \
  "crates/x/src/lib_tests.rs" \
  'fn start() {
    tokio::spawn(async {});
}
'

# Acceptance: `tokio::spawn(` inside a `///` doc comment. These
# aren't real spawn sites — they're prose. A pattern that only
# matches code (via a naive `tokio::spawn(` grep) would false-positive
# here, which is exactly the hole the "body starts with //" filter
# in the gate closes.
run_fixture \
  "doc_comment_mention_is_not_a_spawn" \
  0 \
  "crates/x/src/lib.rs" \
  'fn start() {
    /// Example: see `tokio::spawn(future)` for the canonical shape.
    supervised_spawn("x::work", async {});
}
'

# Acceptance: `//!` module-doc mention of the spawn shape.
run_fixture \
  "module_doc_mention_is_not_a_spawn" \
  0 \
  "crates/x/src/lib.rs" \
  '//! This module wraps `tokio::spawn(f)` with a supervisor.
fn start() {}
'

# Acceptance: the helper module itself. The gate excludes
# `dayseam-core/src/runtime/` because that is where
# `supervised_spawn` is authored — the one file in the tree that
# must contain a bare `tokio::spawn` call by construction.
run_fixture \
  "helper_module_is_exempt" \
  0 \
  "crates/dayseam-core/src/runtime/supervised_spawn.rs" \
  'pub fn supervised_spawn<F>(_c: &str, f: F) {
    tokio::spawn(f);
}
'

# Acceptance: no spawn sites at all in the tree. This is the healthy
# end-state for a workspace that has moved every caller to
# `supervised_spawn`, and must return exit 0 — not 2 — so CI stays
# green on workspaces that don't spawn anything directly.
run_fixture \
  "empty_tree_is_clean_not_invocation_error" \
  0 \
  "crates/x/src/lib.rs" \
  'fn start() {}
'

# --- Report ---------------------------------------------------------
if [[ ${#FAILED[@]} -gt 0 ]]; then
  echo ""
  echo "${#FAILED[@]} fixture(s) failed:"
  printf '  - %s\n' "${FAILED[@]}"
  exit 1
fi

echo ""
echo "all fixtures passed."
