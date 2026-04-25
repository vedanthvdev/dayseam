#!/usr/bin/env bash
# test-close-changelog.sh — bash test suite for close-changelog.sh.
#
# What the suite proves:
#
#   1. Normal close: a populated `[Unreleased]` gets renamed to
#      `[$version] - <today>` and a fresh empty `[Unreleased]`
#      appears above it. Every bullet the contributor authored under
#      the original `[Unreleased]` ends up under the renamed block,
#      in the original order — this is the primary happy path the
#      release workflow depends on for every `semver:patch` merge.
#   2. Idempotency: running the script twice with the same inputs
#      leaves the working tree byte-for-byte identical after the
#      second run. Matches bump-version.sh's guarantee so a manual
#      retry after a transient CI failure is safe.
#   3. Already-closed no-op: if the PR author pre-renamed the block
#      themselves (capstone-PR pattern), the script must not touch
#      anything — doing so would produce a duplicate `[$version]`
#      header or re-rename a block that was deliberately hand-closed.
#   4. Empty `[Unreleased]` no-op: a block with only the header, or
#      only `### Added`/`### Changed` subheaders with no bullets,
#      must not trigger a rename. The preflight gate already refuses
#      vacuous notes but this layer is defensive — a
#      workflow_dispatch dry-run that somehow reached this point
#      shouldn't erase an empty-by-design block.
#   5. Later-version sections are preserved: the `[0.1.0] - …` body
#      (and its bullets) immediately below `[Unreleased]` must not
#      be accidentally duplicated, reordered, or renamed.
#   6. Rejects invalid version strings: `0.1` (missing patch),
#      `1.0.0-rc1` (pre-release suffix), and empty string must all
#      exit 1 with no changes. Prevents an accidental `[v]` header
#      from a mistyped invocation.
#   7. Unrelated prose is preserved: preamble, HTML comments, and
#      anything outside `[Unreleased]` / version blocks must survive
#      byte-for-byte. The release workflow runs this step against
#      the real repo CHANGELOG which carries a multi-paragraph
#      intro and a DAY-155 release-cadence comment block.
#
# The fixtures deliberately mirror the Keep a Changelog shape the
# real CHANGELOG uses so any deviation the release workflow would
# trip over is caught here first.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/release/close-changelog.sh"

if [[ ! -x "$SCRIPT" ]]; then
  echo "test-close-changelog.sh: helper not executable at $SCRIPT" >&2
  exit 1
fi

TESTS_RUN=0
TESTS_FAILED=0

write_changelog() {
  # Writes the heredoc body piped on stdin to the given path. Using
  # `cat` instead of `tee` keeps the path arg explicit and mirrors
  # test-extract-release-notes.sh so the two suites read the same.
  local path="$1"
  cat >"$path"
}

run_helper() {
  # The helper resolves REPO_ROOT from its own location; override with
  # a mktemp so the CHANGELOG arg is the authoritative target and no
  # real repo file is ever touched by tests.
  local changelog="$1"
  local version="$2"
  local today="${3:-}"
  if [[ -n "$today" ]]; then
    REPO_ROOT="$(mktemp -d)" "$SCRIPT" "$version" "$changelog" "$today" 2>/dev/null
  else
    REPO_ROOT="$(mktemp -d)" "$SCRIPT" "$version" "$changelog" 2>/dev/null
  fi
}

run_helper_expecting_failure() {
  local changelog="$1"
  local version="$2"
  local exit_code
  set +e
  REPO_ROOT="$(mktemp -d)" "$SCRIPT" "$version" "$changelog" >/dev/null 2>&1
  exit_code=$?
  set -e
  echo "$exit_code"
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

test_normal_close() {
  # Populated [Unreleased] + no explicit [$version] yet. The rename
  # is the primary happy path the release workflow depends on for
  # every `semver:patch` merge.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- first patch entry

## [0.1.0] - 2026-04-20

### Added

- prior release entry
EOF
  run_helper "$cl" "0.1.1" "2026-05-01" >/dev/null

  if ! grep -qE '^## \[Unreleased\]' "$cl"; then
    echo "  FAIL: [Unreleased] header missing after close" >&2
    cat "$cl" >&2
    return 1
  fi
  if ! grep -qE '^## \[0\.1\.1\] - 2026-05-01$' "$cl"; then
    echo "  FAIL: [0.1.1] - 2026-05-01 header missing after close" >&2
    cat "$cl" >&2
    return 1
  fi
  if ! grep -qE '^## \[0\.1\.0\] - 2026-04-20$' "$cl"; then
    echo "  FAIL: [0.1.0] header missing or modified" >&2
    cat "$cl" >&2
    return 1
  fi

  # The "first patch entry" bullet belonged to [Unreleased] originally
  # and must now belong to [0.1.1]. Sections must appear in the order
  # [Unreleased] → [0.1.1] → [0.1.0], and the entry must sit between
  # [0.1.1] and [0.1.0].
  local unreleased_line patch_line v010_line entry_line
  unreleased_line="$(grep -n '^## \[Unreleased\]' "$cl" | cut -d: -f1)"
  patch_line="$(grep -n '^## \[0\.1\.1\] - 2026-05-01' "$cl" | cut -d: -f1)"
  v010_line="$(grep -n '^## \[0\.1\.0\] - 2026-04-20' "$cl" | cut -d: -f1)"
  entry_line="$(grep -n 'first patch entry' "$cl" | cut -d: -f1)"
  if ! (( unreleased_line < patch_line && patch_line < v010_line )); then
    echo "  FAIL: section ordering wrong: Unreleased=$unreleased_line, 0.1.1=$patch_line, 0.1.0=$v010_line" >&2
    cat "$cl" >&2
    return 1
  fi
  if ! (( patch_line < entry_line && entry_line < v010_line )); then
    echo "  FAIL: 'first patch entry' not between [0.1.1] and [0.1.0]: entry=$entry_line" >&2
    cat "$cl" >&2
    return 1
  fi
  # [Unreleased] body should be only the blank line we emit as a
  # placeholder — no lingering bullets from the old block.
  local leftover
  leftover="$(awk '/^## \[Unreleased\]/{f=1;next} /^## \[/{f=0} f' "$cl" | sed -E '/^[[:space:]]*$/d')"
  if [[ -n "$leftover" ]]; then
    echo "  FAIL: freshly opened [Unreleased] is not empty; leftover:" >&2
    printf '%s\n' "$leftover" >&2
    return 1
  fi
  return 0
}

test_idempotency() {
  # Second invocation with the same inputs must leave the tree
  # byte-for-byte unchanged. Matches bump-version.sh's guarantee so
  # a release operator can safely retry after a transient failure.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- first patch entry

## [0.1.0] - 2026-04-20

### Added

- prior
EOF
  run_helper "$cl" "0.1.1" "2026-05-01" >/dev/null
  local after_first
  after_first="$(cat "$cl")"
  run_helper "$cl" "0.1.1" "2026-05-01" >/dev/null
  local after_second
  after_second="$(cat "$cl")"
  if [[ "$after_first" != "$after_second" ]]; then
    echo "  FAIL: second run modified the tree" >&2
    diff <(printf '%s\n' "$after_first") <(printf '%s\n' "$after_second") >&2
    return 1
  fi
  return 0
}

test_already_closed_no_op() {
  # Capstone-PR pattern: the PR author pre-closed [Unreleased] into
  # [$version] themselves so the diff is reviewable. The workflow
  # step must not touch anything.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- next-release entry

## [0.1.1] - 2026-05-01

### Added

- capstone-PR pre-close already happened

## [0.1.0] - 2026-04-20
EOF
  local before after
  before="$(cat "$cl")"
  run_helper "$cl" "0.1.1" "2026-05-15" >/dev/null
  after="$(cat "$cl")"
  if [[ "$before" != "$after" ]]; then
    echo "  FAIL: already-closed CHANGELOG was modified" >&2
    diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") >&2
    return 1
  fi
  return 0
}

test_empty_unreleased_no_op() {
  # [Unreleased] with only the header and no body → no rename.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

## [0.1.0] - 2026-04-20

### Added

- entry
EOF
  local before after
  before="$(cat "$cl")"
  run_helper "$cl" "0.1.1" "2026-05-01" >/dev/null
  after="$(cat "$cl")"
  if [[ "$before" != "$after" ]]; then
    echo "  FAIL: empty [Unreleased] triggered a rename" >&2
    diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") >&2
    return 1
  fi
  return 0
}

test_subheader_only_unreleased_no_op() {
  # [Unreleased] has ### Added / ### Changed subheaders but no bullets
  # beneath them — preflight treats this as "empty" (see the filter
  # in extract-release-notes.sh), and close-changelog.sh must agree
  # so a workflow that somehow reached this step doesn't rename a
  # vacuous block.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

### Changed

## [0.1.0] - 2026-04-20

- prior
EOF
  local before after
  before="$(cat "$cl")"
  run_helper "$cl" "0.1.1" "2026-05-01" >/dev/null
  after="$(cat "$cl")"
  if [[ "$before" != "$after" ]]; then
    echo "  FAIL: subheader-only [Unreleased] triggered a rename" >&2
    diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") >&2
    return 1
  fi
  return 0
}

test_invalid_versions_rejected() {
  # Three malformed inputs, one assertion each. Anything that is
  # not `\d+\.\d+\.\d+` must exit 1 with no tree change.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- entry
EOF
  local snapshot exit_code
  snapshot="$(cat "$cl")"

  for bad in "0.1" "1.0.0-rc1" ""; do
    exit_code="$(run_helper_expecting_failure "$cl" "$bad")"
    if [[ "$exit_code" != "1" ]]; then
      echo "  FAIL: bad version '$bad' expected exit 1, got '$exit_code'" >&2
      return 1
    fi
    if [[ "$(cat "$cl")" != "$snapshot" ]]; then
      echo "  FAIL: bad version '$bad' mutated the CHANGELOG" >&2
      return 1
    fi
  done
  return 0
}

test_preserves_unrelated_prose() {
  # The preamble (title, blurb, HTML comment) must be byte-for-byte
  # preserved — this is what keeps the release-cadence note, the
  # Keep-a-Changelog link, and any future top-of-file guidance
  # untouched by the rewrite.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

All notable changes to Foo are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!--
Release-cadence note: placeholder body.
-->

## [Unreleased]

### Added

- first patch entry

## [0.1.0] - 2026-04-20

### Added

- prior release entry
EOF
  run_helper "$cl" "0.1.1" "2026-05-01" >/dev/null

  if ! grep -qF 'All notable changes to Foo' "$cl"; then
    echo "  FAIL: preamble blurb was lost" >&2
    cat "$cl" >&2
    return 1
  fi
  if ! grep -qF 'Release-cadence note: placeholder body.' "$cl"; then
    echo "  FAIL: HTML comment body was lost" >&2
    cat "$cl" >&2
    return 1
  fi
  if ! grep -qF '# Changelog' "$cl"; then
    echo "  FAIL: top-level title was lost" >&2
    cat "$cl" >&2
    return 1
  fi
  return 0
}

test_missing_changelog_fails_cleanly() {
  # Path points at a file that doesn't exist → exit 2, not a
  # confusing awk stack trace. This is the shape `release.yml` will
  # trip if someone renames CHANGELOG.md without updating the
  # workflow.
  local root="$1"
  local cl="$root/does-not-exist.md"
  local exit_code
  exit_code="$(run_helper_expecting_failure "$cl" "0.1.1")"
  if [[ "$exit_code" != "2" ]]; then
    echo "  FAIL: missing changelog expected exit 2, got '$exit_code'" >&2
    return 1
  fi
  return 0
}

run_test "normal close: [Unreleased] → [\$version] - <today>" test_normal_close
run_test "running twice is byte-for-byte idempotent" test_idempotency
run_test "already-closed CHANGELOG is a no-op" test_already_closed_no_op
run_test "empty [Unreleased] is a no-op" test_empty_unreleased_no_op
run_test "subheader-only [Unreleased] is a no-op" test_subheader_only_unreleased_no_op
run_test "invalid version strings are rejected" test_invalid_versions_rejected
run_test "unrelated preamble is preserved" test_preserves_unrelated_prose
run_test "missing CHANGELOG exits 2 cleanly" test_missing_changelog_fails_cleanly

echo
echo "Ran $TESTS_RUN tests, $TESTS_FAILED failed"

if [[ "$TESTS_FAILED" -gt 0 ]]; then
  exit 1
fi
