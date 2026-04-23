#!/usr/bin/env bash
# generate-latest-json.sh — emit the Tauri v2 updater manifest.
#
# The `tauri-plugin-updater` client (see
# apps/desktop/src/features/updater/useUpdater.ts) polls
# `plugins.updater.endpoints[0]` from `tauri.conf.json`, which
# resolves to:
#
#   https://github.com/vedanthvdev/dayseam/releases/latest/download/latest.json
#
# That URL is served by GitHub's stable "latest release asset"
# redirect; the asset it returns is whatever this script produced
# for the latest tagged release. The plugin expects a specific
# JSON shape (see docs/updater/2026-04-20-macos-unsigned-updater-
# caveat.md for the full schema); this helper assembles it from
# four inputs: the resolved version, the public download URL of
# the `.app.tar.gz`, the minisign signature content (NOT the path
# — the signature is embedded verbatim in the JSON), and the
# release notes body extracted by `extract-release-notes.sh`.
#
# Why a separate script instead of an inline `jq` in release.yml:
#
#   1. `jq` composing a multiline string with embedded newlines
#      from a file on a Bash 3.2 runner is awkward and fragile;
#      keeping the composition here means the workflow step stays
#      a one-liner.
#   2. The sidecar unit test (test-generate-latest-json.sh) pins
#      the manifest shape so a future "let me just tweak one
#      field" refactor that drifts the JSON away from what the
#      Rust plugin parses fails loudly in red CI instead of
#      silently bricking auto-updates for every installed client.
#
# Usage:
#   generate-latest-json.sh <version> <download_url> <sig_file> <notes_file>
#
# Arguments:
#   version       — the resolved target semver (e.g. "0.6.0"), used
#                   as the `version` key and for the manifest's
#                   self-reported build date (pub_date is the UTC
#                   timestamp at generation).
#   download_url  — the absolute URL the updater should fetch the
#                   signed `.app.tar.gz` from. This is the GitHub
#                   Releases asset URL (not the `/latest/download/`
#                   alias) so the plugin always pulls the exact
#                   binary the manifest describes.
#   sig_file      — path to the `.app.tar.gz.sig` minisign signature
#                   file produced by `tauri build` when signing
#                   env vars are set. The file's contents (base64
#                   payload preceded by a trusted-comment line)
#                   are embedded verbatim into the JSON; the
#                   plugin verifies it against
#                   `plugins.updater.pubkey` before swapping any
#                   bytes onto disk.
#   notes_file    — path to the release notes file produced by
#                   `extract-release-notes.sh`. Embedded into the
#                   `notes` key so the banner can surface a
#                   one-line summary without the frontend having
#                   to fetch the GitHub Release API.
#
# The resulting JSON is printed to stdout. The workflow captures
# it with `> latest.json`.
#
# Exit codes:
#   0  success; JSON manifest written to stdout.
#   1  argument or IO error (missing files, bad semver, etc.).

set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: generate-latest-json.sh <version> <download_url> <sig_file> <notes_file>" >&2
  exit 1
fi

VERSION="$1"
DOWNLOAD_URL="$2"
SIG_FILE="$3"
NOTES_FILE="$4"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "generate-latest-json.sh: version '$VERSION' is not a valid semver triple" >&2
  exit 1
fi

if [[ ! -f "$SIG_FILE" ]]; then
  echo "generate-latest-json.sh: signature file missing at $SIG_FILE" >&2
  exit 1
fi

if [[ ! -f "$NOTES_FILE" ]]; then
  echo "generate-latest-json.sh: notes file missing at $NOTES_FILE" >&2
  exit 1
fi

# The pub_date must be RFC3339 in UTC. `date -u` on both macOS
# and Linux produces a format compatible with the plugin's parser.
PUB_DATE="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

# Read the full signature content (the plugin parses the whole
# file, trusted-comment line included, not just the base64 blob).
SIG_CONTENT="$(cat "$SIG_FILE")"
NOTES_CONTENT="$(cat "$NOTES_FILE")"

# `jq -n` builds the JSON from named inputs; feeding the payload
# through --arg (not --slurpfile or heredoc) is what keeps
# arbitrary characters in the notes/sig safe — `jq` handles the
# JSON escaping for us.
#
# Platform keys — why both `darwin-aarch64` and `darwin-x86_64`
# instead of one `darwin-universal`:
#
# `tauri-plugin-updater` v2 resolves the platforms entry by
# composing `{os}-{arch}` from the running binary. On Apple
# Silicon it probes `darwin-aarch64-app` and then `darwin-aarch64`;
# on Intel it probes `darwin-x86_64-app` and then `darwin-x86_64`.
# The v1-era `darwin-universal` fallback was dropped in 2.x (see
# tauri-plugin-updater 2.10.x — the error every v0.6.0 user saw,
# "None of the fallback platforms [...] were found in the response
# platforms object", is the plugin's literal message when it
# looks for `darwin-aarch64-app` / `darwin-aarch64` and finds
# only a `darwin-universal` key we wrote on its behalf).
#
# We publish one signed `.app.tar.gz` (the lipo-fused universal
# binary) and list it under both arch keys. That is cheaper than
# producing two thin tarballs and keeps the release-workflow
# single-build path intact; the bytes on the wire are identical,
# the only difference is which platform key the plugin looks up
# before downloading them. Adding Windows / Linux flavours later
# means appending sibling keys here (e.g. `windows-x86_64`,
# `linux-x86_64`) with their own signed archive URLs.
jq -n \
  --arg version "$VERSION" \
  --arg pub_date "$PUB_DATE" \
  --arg notes "$NOTES_CONTENT" \
  --arg signature "$SIG_CONTENT" \
  --arg url "$DOWNLOAD_URL" \
  '{
    version: $version,
    notes: $notes,
    pub_date: $pub_date,
    platforms: {
      "darwin-aarch64": {
        signature: $signature,
        url: $url
      },
      "darwin-x86_64": {
        signature: $signature,
        url: $url
      }
    }
  }'
