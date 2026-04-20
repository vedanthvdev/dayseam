#!/usr/bin/env bash
# build-dmg.sh — produce a universal Dayseam .dmg under a
# deterministic output path the release workflow can reference
# without globbing.
#
# What happens:
#
#   1. Ensure both macOS rustup targets are installed
#      (aarch64-apple-darwin + x86_64-apple-darwin). `rustup target
#      add` is idempotent so re-running on a pre-configured host is a
#      fast no-op.
#   2. Run `pnpm --filter @dayseam/desktop tauri build --target
#      universal-apple-darwin`. Tauri's bundler emits both a .app
#      bundle and a .dmg wrapping it; the `bundle.targets` in
#      `apps/desktop/src-tauri/tauri.conf.json` are set to
#      `["app","dmg"]` so the DMG lands without any post-processing
#      step.
#   3. Copy the bundler output to a stable, version-stamped filename
#      inside `dist/release/` so the workflow's upload step has a
#      single path to reference, and so a human running this locally
#      doesn't have to dig through deeply nested `target/` paths to
#      find the artefact.
#
# Usage:
#   build-dmg.sh [version]
#
# `version` defaults to the VERSION file contents. The workflow
# passes the resolved target from bump-version.sh explicitly so the
# two scripts cannot drift mid-run.
#
# Exit codes:
#   0  success; final DMG path is printed to stdout.
#   1  toolchain or build failure (bubbled up from cargo/tauri).

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"
VERSION_FILE="${REPO_ROOT}/VERSION"

if [[ $# -gt 1 ]]; then
  echo "usage: build-dmg.sh [version]" >&2
  exit 1
fi

if [[ $# -eq 1 ]]; then
  VERSION="$1"
else
  VERSION="$(tr -d '[:space:]' <"$VERSION_FILE")"
fi

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "build-dmg.sh: resolved version '$VERSION' is not a valid semver triple" >&2
  exit 1
fi

echo "==> Installing rustup targets for universal-apple-darwin"
rustup target add aarch64-apple-darwin x86_64-apple-darwin

echo "==> Building Dayseam v${VERSION} as a universal .app + .dmg"
# --target universal-apple-darwin triggers Tauri's own lipo-wrapped
# build that invokes cargo twice (once per arch) and fuses the
# resulting binaries. No manual lipo step; the release.yml
# universal-assert step sanity-checks it after the fact.
cd "$REPO_ROOT"
pnpm --filter @dayseam/desktop exec tauri build --target universal-apple-darwin

BUNDLE_DIR="${REPO_ROOT}/apps/desktop/src-tauri/target/universal-apple-darwin/release/bundle"
SRC_DMG="$(ls -t "${BUNDLE_DIR}/dmg/"*.dmg 2>/dev/null | head -n 1 || true)"

if [[ -z "$SRC_DMG" || ! -f "$SRC_DMG" ]]; then
  echo "build-dmg.sh: Tauri bundler did not produce a .dmg under ${BUNDLE_DIR}/dmg/" >&2
  exit 1
fi

OUT_DIR="${REPO_ROOT}/dist/release"
OUT_DMG="${OUT_DIR}/Dayseam-v${VERSION}.dmg"
mkdir -p "$OUT_DIR"
cp "$SRC_DMG" "$OUT_DMG"

# Emit a SHA-256 alongside the DMG so the release workflow can upload
# both artefacts together; this gives downloaders a trivial integrity
# check against release-note-embedded checksums (the v0.1.0 release
# notes quote the sha256 so a user can verify their download without
# trusting the HTML surface).
(cd "$OUT_DIR" && shasum -a 256 "Dayseam-v${VERSION}.dmg") >"${OUT_DMG}.sha256"

echo "==> Built ${OUT_DMG}"
echo "$OUT_DMG"
