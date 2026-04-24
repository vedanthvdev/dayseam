#!/usr/bin/env bash
# test-check-entitlements.sh — self-test for `check-entitlements.sh`
# (DAY-122 / T-3).
#
# The real gate is one-shot on a checked-in file, so a regression in
# the gate logic itself (swapped exit code, inverted comment check,
# `plutil` branch dropped) would not be caught by running it against
# the healthy committed plist. This suite fuzzes the gate with four
# synthetic plists:
#
#   1. A clean, well-formed plist -> gate passes (exit 0).
#   2. A well-formed plist containing an XML comment -> gate fails.
#   3. A malformed plist (broken XML) -> gate fails on `plutil -lint`
#      when `plutil` is available; otherwise the comment check still
#      passes but `plutil` returning 0 is the only failure mode this
#      sub-test guards against, so we skip it on non-macOS runners.
#   4. A missing file -> gate fails with the "missing" message.
#
# Running this script under `bash -u` would trip the `ENTITLEMENTS_FILE`
# env override the real gate expects, so we explicitly export it per
# sub-test.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${REPO_ROOT}/scripts/ci/check-entitlements.sh"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

fail() {
  echo "test-check-entitlements.sh: FAIL — $1" >&2
  exit 1
}

run_gate() {
  # Run the gate with the given ENTITLEMENTS_FILE override; capture
  # exit code without tripping `set -e`.
  local file="$1"
  ENTITLEMENTS_FILE="$file" bash "$GATE" >/dev/null 2>&1 && echo 0 || echo $?
}

# 1. Clean plist — expect exit 0.
CLEAN="$TMP_DIR/clean.plist"
cat >"$CLEAN" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.app-sandbox</key>
  <false/>
</dict>
</plist>
PLIST
rc="$(run_gate "$CLEAN")"
[[ "$rc" == 0 ]] || fail "clean plist: expected 0, got $rc"

# 2. Commented plist — expect non-zero.
COMMENTED="$TMP_DIR/commented.plist"
cat >"$COMMENTED" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <!-- AMFI parser rejects comments; this must fail. -->
  <key>com.apple.security.app-sandbox</key>
  <false/>
</dict>
</plist>
PLIST
rc="$(run_gate "$COMMENTED")"
[[ "$rc" != 0 ]] || fail "commented plist: expected non-zero, got 0"

# 3. Malformed plist (only tested when `plutil` is present).
if command -v plutil >/dev/null 2>&1; then
  BROKEN="$TMP_DIR/broken.plist"
  printf 'this is not xml\n' >"$BROKEN"
  rc="$(run_gate "$BROKEN")"
  [[ "$rc" != 0 ]] || fail "broken plist: expected non-zero, got 0"
fi

# 4. Missing file — expect non-zero.
MISSING="$TMP_DIR/does-not-exist.plist"
rc="$(run_gate "$MISSING")"
[[ "$rc" != 0 ]] || fail "missing plist: expected non-zero, got 0"

echo "test-check-entitlements.sh: all sub-tests passed."
