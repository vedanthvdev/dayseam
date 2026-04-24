#!/usr/bin/env bash
# check-entitlements.sh — DAY-122 / T-3 CI gate.
#
# Fails the build if `apps/desktop/src-tauri/entitlements.plist` is
# missing, fails `plutil -lint`, or contains an XML comment
# (`<!-- … -->`). Extracted verbatim from `scripts/release/build-dmg.sh`
# (DAY-120) so the same checks run at PR time on `ci.yml` rather than
# only on release day — v0.5's capstone flagged this as T-3 because
# a PR that re-introduces a comment passes the per-PR CI clean and
# only explodes on release, 4 minutes into the universal cargo build.
#
# Why this exists (copied from build-dmg.sh so the context travels
# with the gate):
#
#   `codesign` invokes macOS's `AMFIUnserializeXML` parser on the
#   entitlements file, and that parser is stricter than either
#   `plutil` or CoreFoundation: it rejects XML comments outright.
#   v0.6.2 shipped with a heavily-commented entitlements file that
#   `plutil -lint` happily approved but that broke the release job
#   with `AMFIUnserializeXML: syntax error near line 30`. Mirroring
#   what `codesign` enforces later means a regression fails fast
#   (under a second) at PR time.
#
# Portability:
#
#   - `plutil` is macOS-only. On non-macOS runners (the `frontend` job
#     on ubuntu-latest, or a contributor's Linux box) the lint step
#     is *skipped* rather than hard-failed, but the XML-comment check
#     always runs because it's plain `grep`. This matches the
#     release-time behaviour: `codesign` never runs on Linux, so a
#     Linux-only lint mismatch would be noise; the comment gate, by
#     contrast, is the exact thing we can't afford to regress.
#
# Exit codes:
#   0  entitlements.plist is well-formed and comment-free.
#   1  plutil -lint failed, or an XML comment was found, or the file
#      is missing.
#   2  invocation error.

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"
ENTITLEMENTS_FILE="${ENTITLEMENTS_FILE:-${REPO_ROOT}/apps/desktop/src-tauri/entitlements.plist}"

if [[ ! -f "$ENTITLEMENTS_FILE" ]]; then
  echo "check-entitlements.sh: entitlements file missing at ${ENTITLEMENTS_FILE}; tauri.conf.json references it and codesign will fail without it." >&2
  exit 1
fi

if command -v plutil >/dev/null 2>&1; then
  if ! plutil -lint "$ENTITLEMENTS_FILE" >/dev/null; then
    echo "check-entitlements.sh: ${ENTITLEMENTS_FILE} failed plutil -lint" >&2
    exit 1
  fi
else
  echo "check-entitlements.sh: plutil not available on this runner (non-macOS); skipping lint. The XML-comment check below still runs." >&2
fi

# The AMFI comment-rejection check. This is the regression the gate
# exists for: `plutil -lint` returns 0 on commented plists but
# `codesign --entitlements` rejects them at release time.
if grep -q '<!--' "$ENTITLEMENTS_FILE"; then
  echo "check-entitlements.sh: ${ENTITLEMENTS_FILE} contains XML comments; macOS's AMFI parser rejects them (see apps/desktop/src-tauri/entitlements.md for context)." >&2
  exit 1
fi

echo "check-entitlements.sh: ${ENTITLEMENTS_FILE} passes plutil -lint and comment gate."
