#!/usr/bin/env bash
# test-extract-release-notes.sh — bash test suite for
# extract-release-notes.sh.
#
# What the suite proves:
#
#   1. Populated `[$VERSION]` wins: when the CHANGELOG has an
#      explicit `[$VERSION]` section the helper uses it, even if
#      `[Unreleased]` also has content (which would be the
#      "accumulating for the *next* release" case). This is the
#      behaviour `workflow_dispatch`-driven re-releases depend on.
#   2. Normal semver-bump shape: `[$VERSION]` is absent (no
#      capstone pre-close), `[Unreleased]` has the PR's notes,
#      helper falls back to `[Unreleased]` and emits its body.
#   3. Both empty: helper exits 2 with an error so the workflow
#      refuses to release a vacuous version.
#   4. Subheader-only is "empty": a section that contains only a
#      `### Added` subheader with no bullets does not count as
#      content — it signals the contributor forgot to fill in the
#      details.
#   5. Dot-insensitivity: a CHANGELOG that lists `[0X1X0]` (a
#      deliberately garbage header) does not spuriously match
#      `[0.1.0]`. Guards against a regex that forgot to escape
#      dots.
#   6. Next-section boundary: the helper stops at the next `## [`
#      heading and does not bleed the following version's body
#      into the current release's notes.
#   7. Large body survives the pipefail+grep-q SIGPIPE race that
#      misread a real 73 KB v0.1.0 release body as empty.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/release/extract-release-notes.sh"

if [[ ! -x "$SCRIPT" ]]; then
  echo "test-extract-release-notes.sh: helper not executable at $SCRIPT" >&2
  exit 1
fi

TESTS_RUN=0
TESTS_FAILED=0

write_changelog() {
  # Writes a CHANGELOG to the given path with the heredoc body the
  # caller provides on stdin. Using `cat` instead of `tee` keeps
  # the path arg explicit and the trace clean.
  local path="$1"
  cat >"$path"
}

run_helper() {
  local changelog="$1"
  local version="$2"
  REPO_ROOT="$(mktemp -d)" "$SCRIPT" "$version" "$changelog" 2>/dev/null
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

test_explicit_version_wins_over_unreleased() {
  # [$VERSION] is the explicit answer for what goes into the
  # release being published. [Unreleased] is for the *next* release.
  # When both exist and carry different content, the explicit block
  # is authoritative. This is what makes a workflow_dispatch
  # re-release of v0.1.0 publish the v0.1.0 notes even after
  # [Unreleased] has started accumulating v0.1.1 work.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- next-release entry for v0.1.1

## [0.1.0] - 2026-04-20

### Added

- v0.1.0 release entry

## [0.0.1] - 2026-04-15

### Added

- old entry
EOF
  local out
  out="$(run_helper "$cl" "0.1.0")"
  if ! printf '%s' "$out" | grep -q 'v0.1.0 release entry'; then
    echo "  FAIL: expected [0.1.0] body, got:" >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  if printf '%s' "$out" | grep -q 'next-release entry for v0.1.1'; then
    echo "  FAIL: published [Unreleased] (v0.1.1 prep) as v0.1.0" >&2
    return 1
  fi
  if printf '%s' "$out" | grep -q 'old entry'; then
    echo "  FAIL: bled into [0.0.1] body" >&2
    return 1
  fi
  return 0
}

test_falls_back_to_unreleased_when_version_absent() {
  # Normal semver-bump shape: no `[$VERSION]` block exists yet
  # (the release workflow does not rewrite the CHANGELOG on the
  # way out); `[Unreleased]` carries the PR's notes; helper falls
  # back to [Unreleased].
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- patch-release entry

## [0.1.0] - 2026-04-20

### Added

- older release entry
EOF
  # Ask for 0.1.1 — no [0.1.1] block exists, so helper must fall
  # back to [Unreleased].
  local out
  out="$(run_helper "$cl" "0.1.1")"
  if ! printf '%s' "$out" | grep -q 'patch-release entry'; then
    echo "  FAIL: expected [Unreleased] body, got:" >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  if printf '%s' "$out" | grep -q 'older release entry'; then
    echo "  FAIL: bled into [0.1.0] body" >&2
    return 1
  fi
  return 0
}

test_both_empty_fails() {
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

## [0.1.0] - 2026-04-20

## [0.0.1] - 2026-04-15

### Added

- old entry
EOF
  local exit_code
  exit_code="$(run_helper_expecting_failure "$cl" "0.1.0")"
  if [[ "$exit_code" != "2" ]]; then
    echo "  FAIL: expected exit 2, got '$exit_code'" >&2
    return 1
  fi
  return 0
}

test_subheader_only_is_empty() {
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

### Added

- real entry

## [0.1.0] - 2026-04-20

### Added

### Changed
EOF
  # [0.1.0] has subheaders but no bullets — should NOT count as
  # content, so the helper falls back to [Unreleased]. This
  # exercises the subheader filter under the new `[$VERSION]`-first
  # priority: the explicit section is present but vacuous, and the
  # helper must recognise that vacuity rather than publishing an
  # empty release.
  local out
  out="$(run_helper "$cl" "0.1.0")"
  if ! printf '%s' "$out" | grep -q 'real entry'; then
    echo "  FAIL: expected [Unreleased] fallback, got:" >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  return 0
}

test_dots_are_literal() {
  local root="$1"
  local cl="$root/CHANGELOG.md"
  # Ask for version 0.1.0 in a CHANGELOG that has [0X1X0] (fictional
  # garbage header). The helper must NOT match `0X1X0` when asked
  # for `0.1.0`, and since [Unreleased] is also empty, it should
  # exit 2.
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

## [0X1X0]

### Added

- garbage entry
EOF
  local exit_code
  exit_code="$(run_helper_expecting_failure "$cl" "0.1.0")"
  if [[ "$exit_code" != "2" ]]; then
    echo "  FAIL: expected exit 2 (no match), got '$exit_code'" >&2
    return 1
  fi
  return 0
}

test_stops_at_next_version_header() {
  local root="$1"
  local cl="$root/CHANGELOG.md"
  write_changelog "$cl" <<'EOF'
# Changelog

## [Unreleased]

## [0.1.0] - 2026-04-20

- in version 0.1.0

## [0.0.1] - 2026-04-15

- in version 0.0.1
EOF
  local out
  out="$(run_helper "$cl" "0.1.0")"
  if ! printf '%s' "$out" | grep -q 'in version 0.1.0'; then
    echo "  FAIL: expected 'in version 0.1.0', got:" >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  if printf '%s' "$out" | grep -q 'in version 0.0.1'; then
    echo "  FAIL: bled past the next ## [ heading" >&2
    return 1
  fi
  return 0
}

test_large_body_is_not_misread_as_empty() {
  # Regression test for the `set -o pipefail` + `grep -q` race the
  # v0.1.0 capstone surfaced: when a section body is large enough
  # (tens of KB) the upstream sed is still writing when grep -q
  # closes its input on the first match, and pipefail used to
  # surface that SIGPIPE as a non-zero exit — making has_content
  # report "empty" for a valid, content-rich release. The fixture
  # below writes a ~40KB [0.1.0] body (500 bullets × ~80 chars)
  # which is comfortably over the buffer boundary.
  local root="$1"
  local cl="$root/CHANGELOG.md"
  {
    cat <<'EOF'
# Changelog

## [Unreleased]

## [0.1.0] - 2026-04-20

### Added

EOF
    awk 'BEGIN { for (i = 1; i <= 500; i++) printf "- fixture bullet %04d: lorem ipsum dolor sit amet consectetur adipiscing elit\n", i }'
    cat <<'EOF'

## [0.0.1] - 2026-04-15

### Added

- sentinel from the next section
EOF
  } >"$cl"
  local out
  out="$(run_helper "$cl" "0.1.0")"
  if ! printf '%s' "$out" | grep -q 'fixture bullet 0001:'; then
    echo "  FAIL: expected body to start with 'fixture bullet 0001:', got:" >&2
    printf '%s\n' "$out" | head -5 >&2
    return 1
  fi
  if ! printf '%s' "$out" | grep -q 'fixture bullet 0500:'; then
    echo "  FAIL: expected body to contain last bullet 'fixture bullet 0500:'" >&2
    return 1
  fi
  if printf '%s' "$out" | grep -q 'sentinel from the next section'; then
    echo "  FAIL: bled into next [0.0.1] section" >&2
    return 1
  fi
  return 0
}

run_test "populated [\$VERSION] wins over [Unreleased]" test_explicit_version_wins_over_unreleased
run_test "normal-bump shape falls back to [Unreleased]" test_falls_back_to_unreleased_when_version_absent
run_test "both sections empty exits 2" test_both_empty_fails
run_test "subheader-only section counts as empty" test_subheader_only_is_empty
run_test "dots in version are treated as literals" test_dots_are_literal
run_test "extraction stops at next ## [ heading" test_stops_at_next_version_header
run_test "large body survives pipefail+grep-q SIGPIPE race" test_large_body_is_not_misread_as_empty

echo
echo "Ran $TESTS_RUN tests, $TESTS_FAILED failed"

if [[ "$TESTS_FAILED" -gt 0 ]]; then
  exit 1
fi
