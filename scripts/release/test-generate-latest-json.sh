#!/usr/bin/env bash
# test-generate-latest-json.sh — pins the shape of the Tauri
# updater manifest produced by generate-latest-json.sh.
#
# Why a bash test and not a TS test: the shape lives in a
# workflow-time artefact the frontend never compiles against.
# Tauri's plugin parses it at runtime on every user's machine, so
# a drifted field is silent — every installed app quietly stops
# seeing new updates until someone downloads a fresh DMG from
# GitHub. The test here is the nearest thing to a contract the
# manifest has, and it runs in the same CI step as the rest of
# the release-script suite so a future "let me just rename pub_date
# to publishedAt" change fails in red before it merges.
#
# What it proves:
#
#   1. Rejects bad arguments (wrong argc, bad semver, missing sig,
#      missing notes) with exit 1 and a useful message — so the
#      workflow fails fast instead of publishing a manifest with
#      an empty `version` string or an unescaped signature.
#   2. Emits valid JSON with exactly the keys the Rust plugin
#      parses: top-level `version` / `notes` / `pub_date` /
#      `platforms`, and BOTH `darwin-aarch64` and `darwin-x86_64`
#      children with matching `signature` / `url`. `tauri-plugin-
#      updater` 2.x dropped the v1-era `darwin-universal` fallback;
#      an installed client on Apple Silicon now probes
#      `darwin-aarch64-app` then `darwin-aarch64` and errors out
#      ("None of the fallback platforms ... were found in the
#      response platforms object") if neither is present — even
#      though the binary we ship is lipo-fused and would run on
#      either arch. A regression that drops one of the two arch
#      keys or reintroduces `darwin-universal` must fail here
#      before it publishes a manifest that bricks installed
#      clients' update check.
#   3. Embeds the signature file contents verbatim, including
#      the newline-separated trusted-comment + base64 payload
#      the minisign format uses. A previous draft `tr -d '\n'`'d
#      the payload (thinking it was junk whitespace) and every
#      subsequent verify() failed with "invalid signature" until
#      the bug was noticed.
#   4. Emits an RFC3339 UTC `pub_date` (ending in `Z`), which
#      matches what the plugin's `chrono`-based parser accepts.
#      A naive `date +%s` epoch would parse but would confuse
#      any ops dashboard reading this file.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/release/generate-latest-json.sh"

if [[ ! -x "$SCRIPT" ]]; then
  echo "test-generate-latest-json.sh: $SCRIPT not executable" >&2
  exit 1
fi

TESTS_RUN=0
TESTS_FAILED=0

run_test() {
  local name="$1"
  shift
  TESTS_RUN=$((TESTS_RUN + 1))
  echo "• $name"
  if (set +e; "$@"; exit $?); then
    echo "  ok"
  else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    echo "  FAILED"
  fi
}

test_rejects_wrong_argc() {
  if "$SCRIPT" 0.6.0 >/dev/null 2>&1; then
    echo "  FAIL: accepted a call with 1 arg instead of 4" >&2
    return 1
  fi
  return 0
}

test_rejects_bad_semver() {
  local scratch
  scratch="$(mktemp -d)"
  trap "rm -rf '$scratch'" RETURN
  printf 'sig content\n' >"$scratch/sig"
  printf 'notes\n' >"$scratch/notes"
  if "$SCRIPT" "not-a-version" "https://example.com/x.tar.gz" "$scratch/sig" "$scratch/notes" >/dev/null 2>&1; then
    echo "  FAIL: accepted 'not-a-version' as semver" >&2
    return 1
  fi
  return 0
}

test_rejects_missing_sig_file() {
  local scratch
  scratch="$(mktemp -d)"
  trap "rm -rf '$scratch'" RETURN
  printf 'notes\n' >"$scratch/notes"
  if "$SCRIPT" "0.6.0" "https://example.com/x.tar.gz" "$scratch/nope.sig" "$scratch/notes" >/dev/null 2>&1; then
    echo "  FAIL: accepted a missing signature file" >&2
    return 1
  fi
  return 0
}

test_rejects_missing_notes_file() {
  local scratch
  scratch="$(mktemp -d)"
  trap "rm -rf '$scratch'" RETURN
  printf 'sig\n' >"$scratch/sig"
  if "$SCRIPT" "0.6.0" "https://example.com/x.tar.gz" "$scratch/sig" "$scratch/nope.md" >/dev/null 2>&1; then
    echo "  FAIL: accepted a missing notes file" >&2
    return 1
  fi
  return 0
}

test_happy_path_emits_expected_shape() {
  local scratch
  scratch="$(mktemp -d)"
  trap "rm -rf '$scratch'" RETURN
  # Realistic minisign-format signature: a trusted-comment line
  # followed by the base64 payload. `tauri signer sign` emits
  # this exact two-line format.
  printf 'untrusted comment: signature from tauri secret key\nRUTrustedBase64BlobGoesHere==\ntrusted comment: timestamp:1712345678\tfile:Dayseam.app.tar.gz\nSignatureBase64Here==\n' >"$scratch/sig"
  printf '### Highlights\n- DAY-108 in-app updater\n' >"$scratch/notes"

  local out
  out="$("$SCRIPT" "0.6.0" "https://github.com/dayseam/dayseam/releases/download/v0.6.0/Dayseam-v0.6.0.app.tar.gz" "$scratch/sig" "$scratch/notes")"

  # Parse with jq and assert every field matches.
  local version notes pub_date
  version="$(jq -r '.version' <<<"$out")"
  notes="$(jq -r '.notes' <<<"$out")"
  pub_date="$(jq -r '.pub_date' <<<"$out")"

  if [[ "$version" != "0.6.0" ]]; then
    echo "  FAIL: version: expected 0.6.0 got '$version'" >&2
    return 1
  fi
  if ! [[ "$notes" == *"DAY-108 in-app updater"* ]]; then
    echo "  FAIL: notes did not preserve body content; got '$notes'" >&2
    return 1
  fi
  # RFC3339 UTC: YYYY-MM-DDTHH:MM:SSZ
  if ! [[ "$pub_date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]]; then
    echo "  FAIL: pub_date not RFC3339-UTC; got '$pub_date'" >&2
    return 1
  fi

  # Both arch keys must be present — a manifest that publishes
  # only one of them bricks update checks for every installed
  # client on the other arch with exactly the error v0.6.0 users
  # surfaced ("None of the fallback platforms [...] were found in
  # the response platforms object"). Asserting each one explicitly
  # (instead of a `keys | length == 2` check) is what pins the
  # exact string the plugin composes from `{os}-{arch}` at runtime.
  local expected_url="https://github.com/dayseam/dayseam/releases/download/v0.6.0/Dayseam-v0.6.0.app.tar.gz"
  local arch
  for arch in darwin-aarch64 darwin-x86_64; do
    local sig_val url_val
    sig_val="$(jq -r --arg k "$arch" '.platforms[$k].signature' <<<"$out")"
    url_val="$(jq -r --arg k "$arch" '.platforms[$k].url' <<<"$out")"
    if [[ "$sig_val" == "null" || -z "$sig_val" ]]; then
      echo "  FAIL: platforms.${arch}.signature missing; manifest would break updates for that arch" >&2
      return 1
    fi
    # Signature must preserve the trusted-comment + base64 payload
    # lines verbatim. A collapsed/normalised signature would make
    # every verify() fail on the installed client.
    if ! [[ "$sig_val" == *"untrusted comment"* && "$sig_val" == *"trusted comment"* ]]; then
      echo "  FAIL: platforms.${arch}.signature content was stripped; got '$sig_val'" >&2
      return 1
    fi
    if [[ "$url_val" != "$expected_url" ]]; then
      echo "  FAIL: platforms.${arch}.url mismatch; got '$url_val'" >&2
      return 1
    fi
  done

  # Guard against a future "let me just add darwin-universal back
  # as a belt-and-braces fallback" change. The plugin silently
  # ignores it, so keeping it around is dead weight that pretends
  # to be coverage. If the real fix is ever "add a third arch"
  # (e.g. Windows), that key is named by the arch the plugin
  # composes at runtime, not `universal`.
  local universal
  universal="$(jq -r '.platforms["darwin-universal"] // empty' <<<"$out")"
  if [[ -n "$universal" ]]; then
    echo "  FAIL: manifest still publishes the dropped-by-2.x 'darwin-universal' key; drop it — the plugin does not resolve it" >&2
    return 1
  fi
  return 0
}

run_test "rejects wrong argc" test_rejects_wrong_argc
run_test "rejects non-semver version string" test_rejects_bad_semver
run_test "rejects missing signature file" test_rejects_missing_sig_file
run_test "rejects missing notes file" test_rejects_missing_notes_file
run_test "happy path emits expected manifest shape" test_happy_path_emits_expected_shape

echo
echo "Tests run: $TESTS_RUN, failed: $TESTS_FAILED"

if (( TESTS_FAILED > 0 )); then
  exit 1
fi
