#!/usr/bin/env bash
# no-bare-spawn.sh — DAY-113 CI gate.
#
# Fails the build if any non-test production file contains a bare
# `tokio::spawn` or `tauri::async_runtime::spawn` call that is not
# either:
#
#   - a call to `supervised_spawn(context, future)` from
#     `dayseam-core::runtime` (the canonical shape), OR
#   - a bare `tokio::spawn(...)` with a `// bare-spawn: intentional`
#     marker comment on the same line or the immediately preceding
#     line explaining why supervision is deliberately opted out.
#
# The gate's job is structural: v0.4's F-10 discovered a panic-eating
# fire-and-forget spawn in `startup.rs` that stayed invisible for
# weeks. Rather than patch each site one-by-one as new findings drop,
# DAY-113 lands `supervised_spawn` + this gate so every future spawn
# inherits supervision-by-default. If a future change legitimately
# needs a bare spawn (typed `JoinHandle<T>` for a channel consumer,
# reaper-of-supervisors, trivial drain), the marker-comment escape
# hatch documents the decision at the call site where a reviewer will
# see it.
#
# Portability: uses GNU/BSD `grep -rnB 1` so the script runs on
# ubuntu-latest, macos-latest, and any developer laptop without a
# ripgrep dependency. `awk` is POSIX and available everywhere.
#
# Exit codes:
#   0  no bare, unannotated spawns in non-test production code.
#   1  at least one forbidden site — offending lines are printed.
#   2  invocation error.

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$REPO_ROOT"

# Files to scan. Restricting to `*.rs` keeps the gate fast and
# deterministic; we never want to match `tokio::spawn` inside a
# markdown code block in `docs/`.
#
# Exclusions:
#   - `**/tests/**`                integration tests
#   - `**/*tests.rs`               `#[cfg(test)] mod tests { … }` blocks
#                                  also matches `foo_tests.rs` files
#   - `**/test-utils/**`           shared test helpers
#   - `scripts/`                   this script's own pattern prose
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

# `grep -rnB 1` emits one paragraph per match separated by `--`.
# Within a paragraph the context line (if any) comes first, match
# line last. We prefer to ignore the filename-matches `grep --include`
# only includes files matching the glob, and `--exclude-dir` prunes
# whole directories.
grep \
  --recursive \
  --line-number \
  --with-filename \
  --before-context 1 \
  --include='*.rs' \
  --exclude-dir='tests' \
  --exclude-dir='test-utils' \
  --exclude-dir='scripts' \
  -e 'tokio::spawn(' \
  -e 'tauri::async_runtime::spawn(' \
  crates/ apps/ >"$tmp" 2>/dev/null || true

# Drop any paragraph whose match line comes from a `*tests.rs` file.
# `--exclude-dir` can't filter filename globs, and `--exclude` does
# not accept patterns with `*` on every grep flavour, so we do the
# filename filter here where awk has full control.
raw="$(cat "$tmp")"

# Empty result is the healthy end-state: either the tree legitimately
# has zero bare spawn calls (everything went through
# `supervised_spawn`), or every match lived in an excluded directory.
# Treat that as success and return 0 before invoking awk on empty
# input.
if [[ -z "$raw" ]]; then
  echo "no-bare-spawn: no unsupervised spawn sites found."
  exit 0
fi

bad="$(printf '%s\n' "$raw" | awk '
  # Paragraph separator that grep -B emits between non-contiguous
  # match groups. Resetting `ctx` here makes sure a context line from
  # one paragraph is not carried into the next.
  $0 == "--" { ctx = ""; next }

  {
    line = $0

    # grep formats:
    #   match line:   PATH:LINE:CONTENT
    #   context line: PATH-LINE-CONTENT
    # Split on ":" for a match line and on "-" for a context line.
    # The 2nd field is the line number in either case; the 3rd onward
    # is the file body.

    # Strip "PATH:LINE:" for a match line.
    body = line
    sub(/^[^:]+:[0-9]+:/, "", body)
    is_match = (body != line)

    if (!is_match) {
      ctx = line
      next
    }

    # Extract the path (everything before the first ":LINE:").
    path = line
    sub(/:[0-9]+:.*$/, "", path)

    # Skip test files matched by filename glob that --exclude-dir
    # cannot reach (e.g. `foo_tests.rs` next to production code).
    if (path ~ /tests\.rs$/ || path ~ /_tests\.rs$/ || path ~ /\/test_/) {
      ctx = ""
      next
    }

    # Skip the `supervised_spawn` helper module itself. The helper is
    # by definition the only place that legitimately writes
    # `tokio::spawn(...)` twice without a marker comment — it IS the
    # marker. Excluding the directory here (rather than at the grep
    # step) keeps the exclusion visible to a reader of the gate.
    if (path ~ /dayseam-core\/src\/runtime\//) { ctx = ""; next }

    # Skip match lines that are actually inside `//` / `///` / `//!`
    # comments. `grep -B 1` naturally matches inside doc-comment
    # example blocks (e.g. a doc comment that mentions
    # `tokio::spawn(future)` as prose); those are not real spawn
    # sites. A pure-whitespace prefix followed by `//` is the
    # classical Rust line-comment shape.
    if (body ~ /^[[:space:]]*\/\//) { ctx = ""; next }

    # Canonical shape: a call to supervised_spawn is implicitly
    # fine (the helper IS a supervised tokio::spawn wrapper, so
    # grep matches its internals, but the production call site
    # does not).
    if (body ~ /supervised_spawn/) { ctx = ""; next }

    # Same-line escape hatch.
    if (body ~ /\/\/[[:space:]]*bare-spawn:[[:space:]]*intentional/) {
      ctx = ""; next
    }

    # Previous-line escape hatch.
    if (ctx != "" && ctx ~ /\/\/[[:space:]]*bare-spawn:[[:space:]]*intentional/) {
      ctx = ""; next
    }

    print line
    ctx = ""
  }
' || true)"

if [[ -n "$bad" ]]; then
  echo "Error: bare tokio::spawn / tauri::async_runtime::spawn in production code." >&2
  echo "" >&2
  echo "Use runtime::supervised_spawn(\"context\", future) from dayseam-core," >&2
  echo "or add a '// bare-spawn: intentional — <reason>' comment on the spawn" >&2
  echo "line (or the line directly above it) justifying the opt-out." >&2
  echo "" >&2
  echo "Offending sites:" >&2
  echo "$bad" >&2
  exit 1
fi

echo "no-bare-spawn: all production spawns are supervised or explicitly opted out."
