# Phase 3 hardening + cross-cutting review

**Task:** [Phase 3, Task 8 — Phase-3 hardening + cross-cutting review](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#task-8-phase-3-hardening--cross-cutting-review)
**Branch:** `DAY-68-phase-3-task-8-v0.1.0-smoke`
**Semver label:** `semver:none`
**Review date:** 2026-04-20
**Release under review:** [`v0.1.0`](https://github.com/vedanthvdev/dayseam/releases/tag/v0.1.0)

This document is the written artefact of the Phase 3 capstone review. It
enumerates what was reviewed, how it was reviewed, every finding that
surfaced, and the resolution (fix in this PR, follow-up PR, or explicit
deferral with a linked tracking issue). Its shape mirrors
[`phase-2-review.md`](./phase-2-review.md); only the phase-specific
lenses differ.

Phase 3 shipped the second connector (`connector-gitlab`), the
cross-source `CommitAuthored` dedup + `rolled_into_mr` rollup that the
design has carried since day one, the per-source error-card UI surface
that GitLab's richer failure modes finally made load-bearing, a
Playwright-BDD E2E gate on the happy path, the universal-macOS release
pipeline, and — uniquely for this phase — the first published
[`v0.1.0`](https://github.com/vedanthvdev/dayseam/releases/tag/v0.1.0)
release on GitHub. Task 8 is what closes Phase 3 against the *published
DMG*, not a dry-run artefact, per the plan's Task 8 / Task 9 ordering
swap recorded on 2026-04-20.

---

## 1. Inventory

### 1.1 Baseline and head

| | Commit | Label |
|---|---|---|
| Baseline | `577fcee` | Merge of [PR #50 `DAY-50-phase-2-hardening`](https://github.com/vedanthvdev/dayseam/pull/50) — last commit of Phase 2 |
| Head     | `dd54601` | Merge of [PR #68 `DAY-67-release-binary-glob-main-binary`](https://github.com/vedanthvdev/dayseam/pull/68) — last commit before this review, and the one that unblocked the actual `v0.1.0` build |

### 1.2 PRs merged in scope (first-parent, excluding the baseline)

| # | PR | Branch | Summary |
|---|----|--------|---------|
|  1 | [#53](https://github.com/vedanthvdev/dayseam/pull/53) | `DAY-53-phase-3-plan`                            | Phase 3 implementation plan |
|  2 | [#54](https://github.com/vedanthvdev/dayseam/pull/54) | `DAY-54-connector-gitlab`                        | `connector-gitlab` — PAT auth, Events API spine, seven `gitlab.*` error codes |
|  3 | [#55](https://github.com/vedanthvdev/dayseam/pull/55) | `DAY-55-gitlab-enrichment-dedup`                 | GitLab enrichment + cross-source `CommitAuthored` dedup + `rolled_into_mr` rollup |
|  4 | [#56](https://github.com/vedanthvdev/dayseam/pull/56) | `DAY-56-gitlab-ui-error-cards`                   | GitLab admin UI + per-source error cards + Reconnect deep link |
|  5 | [#57](https://github.com/vedanthvdev/dayseam/pull/57) | `DAY-57-phase-2-deferral-cleanup`                | Phase 2 deferral cleanup (ARC-03, MNT-02, PERF-14, TST-05) |
|  6 | [#58](https://github.com/vedanthvdev/dayseam/pull/58) | `DAY-58-e2e-playwright`                          | Playwright-BDD E2E + `@dayseam/e2e` package + `pnpm e2e` CI job |
|  7 | [#60](https://github.com/vedanthvdev/dayseam/pull/60) | `DAY-59-release-engineering`                     | Release engineering — universal DMG, `release.yml`, `UNSIGNED-FIRST-RUN.md`, Phase 3.5 codesign doc |
|  8 | [#62](https://github.com/vedanthvdev/dayseam/pull/62) | `DAY-60-graphify-decision`                       | `graphify` adopt-or-defer — deferred to v0.2 with scoring |
|  9 | [#63](https://github.com/vedanthvdev/dayseam/pull/63) | `DAY-62-v0.1.0-capstone`                         | v0.1.0 capstone — `VERSION` flip, CHANGELOG close-out, README install link |
| 10 | [#64](https://github.com/vedanthvdev/dayseam/pull/64) | `DAY-63-release-workflow-v0.1.0-fix`             | `release.yml` hotfix #1 — preflight + PREV inference for the first release |
| 11 | [#65](https://github.com/vedanthvdev/dayseam/pull/65) | `DAY-64-release-notes-prefer-target-section`     | `release.yml` hotfix #2 — prefer `[$TARGET]` over `[Unreleased]` for release notes |
| 12 | [#66](https://github.com/vedanthvdev/dayseam/pull/66) | `DAY-65-release-tauri-ci-and-pipefail`           | `release.yml` hotfix #3 — unmask tauri build errors (`set -o pipefail`, drop `CI=1`) |
| 13 | [#67](https://github.com/vedanthvdev/dayseam/pull/67) | `DAY-66-release-target-workspace-root`           | `release.yml` hotfix #4 — resolve cargo workspace target dir |
| 14 | [#68](https://github.com/vedanthvdev/dayseam/pull/68) | `DAY-67-release-binary-glob-main-binary`         | `release.yml` hotfix #5 — glob the actual `.app/Contents/MacOS/*` binary |

PRs #64–#68 (DAY-63 through DAY-67) are the five-step hotfix chain that
took the release pipeline from "green dry-run, red real run" to a
published `v0.1.0`. Every one of them merged with regression tests in
`scripts/release/` so the same failure modes do not recur. They are
in-scope for this review because they are the difference between
"hypothetical release" and "actual release a stranger can download."

### 1.3 Surface under review

```
$ git diff --shortstat 577fcee..dd54601
 144 files changed, 12290 insertions(+), 419 deletions(-)
```

Rough distribution:

| Directory                        | Why it changed |
|----------------------------------|----------------|
| `crates/connectors/connector-gitlab/` | New crate (Task 1 + 2) |
| `crates/dayseam-report/`              | Cross-source dedup + `rolled_into_mr` + template-version bump (Task 2) |
| `apps/desktop/`                       | `AddGitlabSourceDialog`, `SourceErrorCard`, `gitlab_validate_pat` IPC + capabilities (Task 3) |
| `crates/dayseam-orchestrator/`        | Registry wiring + `RunStreams::with_progress` convergence (Task 1.9 + Task 4) |
| `crates/dayseam-db/`                  | PERF-14 additive index migration `0004_*.sql` (Task 4) |
| `e2e/`                                | New Playwright-BDD package (Task 5) |
| `scripts/release/`                    | `bump-version.sh`, `build-dmg.sh`, `extract-release-notes.sh`, `resolve-prev-version.sh`, bash test suites (Task 6 + hotfixes) |
| `.github/workflows/`                  | `release.yml`, `e2e.yml`, minor CI splits (Tasks 5 + 6 + hotfixes) |
| `docs/release/` + `docs/decisions/`   | `UNSIGNED-FIRST-RUN.md`, `PHASE-3-5-CODESIGN.md`, graphify decision (Tasks 6 + 7) |

---

## 2. Hardening battery

All commands run from a clean workspace on `master` @ `dd54601` with
`CARGO_TARGET_DIR` **unset** (sandbox cache wiped — see §4 MNT-01 for
why that matters). Exit 0 on every row.

| # | Command | Result |
|---|---------|--------|
|  1 | `cargo fmt --check` | ✅ clean |
|  2 | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ clean |
|  3 | `cargo test --workspace` | ✅ **379 passed, 0 failed, 1 ignored** (376 before the two CORR-01/02 fixes + three new regression tests added by this PR) |
|  4 | `cargo test -p connector-gitlab --all-features` | ✅ all green |
|  5 | `cargo test -p dayseam-report --test dedup` | ✅ all green |
|  6 | `cargo test -p dayseam-report --test rollup_mr` | ✅ all green |
|  7 | `cargo test -p dayseam-report --test golden -- dev_eod_dedups_commitauthored_across_sources` | ✅ green |
|  8 | `cargo test -p dayseam-orchestrator --test happy_path -- multi_source_dedups_commitauthored` | ✅ green |
|  9 | `pnpm -r lint` | ✅ clean |
| 10 | `pnpm -r typecheck` | ✅ clean |
| 11 | `pnpm --filter @dayseam/desktop test` | ✅ **152 passed (31 files)** |
| 12 | `pnpm --filter @dayseam/e2e e2e` | ✅ **1 passed** (`happy-path/generate-and-save-report.feature`) |
| 13 | `cargo deny check` | ✅ clean |
| 14 | `cargo audit` | ✅ clean |
| 15 | `cargo machete` | ✅ no unused dependencies |
| 16 | `scripts/release/test-bump-version.sh` | ✅ all cases green |
| 17 | `scripts/release/test-extract-release-notes.sh` | ✅ all cases green |
| 18 | `scripts/release/test-resolve-prev-version.sh` | ✅ all cases green |

> **Plan doc drift (minor).** The plan's §8.3 battery lists
> `pnpm --filter @dayseam/desktop e2e`, but the `e2e` script lives in the
> `@dayseam/e2e` package, not `@dayseam/desktop` (the E2E suite was
> promoted out of the desktop app into its own root-level workspace
> package in Task 5). Filed as **MNT-03** below; fixed in this PR's plan
> doc close-out rather than a separate doc-drift PR.

---

## 3. Fresh-DMG smoke test (the Task 8 ↔ Task 9 ordering swap)

This is the test the plan's ordering swap exists for: we run the smoke
against the actual published DMG, not a dry-run artefact.

**Artefact under test:**
- `Dayseam-v0.1.0.dmg` downloaded from the
  [v0.1.0 release page](https://github.com/vedanthvdev/dayseam/releases/tag/v0.1.0)
- `Dayseam-v0.1.0.dmg.sha256` downloaded from the same page
- Local verification: `shasum -a 256 -c Dayseam-v0.1.0.dmg.sha256` → ✅ OK

**DMG + bundle inspection:**

| Check | Expected | Observed |
|---|---|---|
| Mounts via `hdiutil attach` without error | yes | ✅ |
| `.app` bundle name | `Dayseam.app` | ✅ |
| `CFBundleShortVersionString` | `0.1.0` | ✅ |
| `CFBundleIdentifier` | `app.dayseam.desktop` | ✅ (design §13) |
| `lipo -archs Contents/MacOS/<bin>` | `x86_64 arm64` | ✅ universal |
| `codesign -dv` | ad-hoc (`Signature=adhoc`) | ✅ as designed; Phase 3.5 closes this |
| `nm Contents/MacOS/<bin> \| grep -E 'dev_start_demo_run\|dev_emit_toast'` | no match | ✅ dev-commands feature gate held across the release path (Task 6 invariant 6) |
| `xattr -l /path/to/downloaded.dmg` | includes `com.apple.quarantine` | ✅ Gatekeeper sees it as a download |

**Gatekeeper first-run walk** per
[`docs/release/UNSIGNED-FIRST-RUN.md`](../release/UNSIGNED-FIRST-RUN.md):

1. Drag `Dayseam.app` to `~/Applications/` (the personal
   `Applications/`, not the system-wide one — the doc's screenshots use
   the system one, which requires sudo; `~/Applications/` is equally
   valid and is what a non-admin user would land on). **See MNT-04
   below.**
2. `spctl --assess --verbose=4 ~/Applications/Dayseam.app` →
   `rejected (the code is unsigned)`. ✅ matches the doc's expectation.
3. Simulated first-run: `xattr -w com.apple.quarantine` to mimic a
   fresh download, then right-click → Open, accept Gatekeeper's
   warning. App launches, creates its `state.db` under
   `~/Library/Application Support/app.dayseam.desktop/`, presents the
   first-run sidebar from DAY-47. ✅

**Golden-path smoke (add local-git source → pick date → generate →
save-to-markdown):** programmatically exercised by the Playwright E2E in
row 12 of §2 against the *built* desktop frontend with the IPC boundary
mocked; *also* manually walked in the GUI against a throwaway tempdir
state. No regressions observed. A richer "real GitLab instance +
real scan root + real Keychain-persisted PAT" run is tracked as part of
the ongoing dogfood sweep (§5) because it depends on the author's
self-hosted GitLab and is not something the review PR itself can
automate without leaking host details.

**PAT / secret-leak grep:**

```
$ rg -n --hidden --no-ignore-vcs 'glpat-[A-Za-z0-9_-]{20,}' \
     -g '!target/**' -g '!.git/**' -g '!node_modules/**'
(no results)

$ rg -n --hidden --no-ignore-vcs 'glpat-[A-Za-z0-9_-]{20,}' \
     -- e2e/ scripts/ docs/ .github/
(no results)
```

The wiremock fixtures in `crates/connectors/connector-gitlab/tests/fixtures/`
use synthetic PAT strings that intentionally do not match the
`glpat-*` prefix pattern (they are clearly labelled test data;
`error_taxonomy_matches_design` and `pat_never_leaks_to_logs_or_ipc`
hold the line that the wrapper never unwraps into a log).

---

## 4. Findings

Aggregated from three review-persona subagents (correctness, security,
maintainability) plus the phase-specific lenses named in the plan's
Task 8 intro. Fifteen findings total; dispositioned as **Fix in this
PR (2)**, **Deferred to v0.1.1 (1)**, **Deferred to v0.2 (7)**,
**Noted, no action needed (5)**.

| ID | Lens | Severity | Title | Disposition |
|---|---|---|---|---|
| **CORR-01** | Correctness | **High** | `HttpClient::send` collapses 401/403 into `DayseamError::Network { code: "http.transport" }`, silently breaking the Reconnect-error-card contract the UI depends on | **Fix in this PR** |
| **CORR-02** | Correctness | **High** | GitLab evidence links are composed as `{base}/-/api/v4/projects/{id}/merge_requests/{iid}`, mixing the UI routing prefix `/-/` with the API prefix `api/v4/`; the resulting URL 404s on every real GitLab host | **Fix in this PR** |
| CORR-03 | Correctness | Medium | `day_bounds_utc` treats the timezone-offset edge at `America/Nuuk`-style DST-and-offset-skip hours as a plain addition; a real user in a DST-jumping zone sees a 1-hour gap in the window on the transition day | Deferred to v0.2 |
| CORR-04 | Correctness | Medium | The per-source error card does not surface a "Retry last sync" button for `gitlab.rate_limited`, so a user who paused mid-rate-limit has to wait for the next automatic trigger rather than resuming on demand | Deferred to v0.1.1 |
| CORR-05 | Correctness | Low | `dev_eod_dedups_commitauthored_across_sources` golden snapshot locks in a specific sort order for `(rolled into !N)` suffixes; the dedup helper's output is order-independent by property test but the template pass is order-sensitive | Noted, no action |
| SEC-01 | Security | Medium | `release.yml` interpolates the git tag directly into a shell string in the "resolve previous version" step; a maliciously named tag could inject commands. Current tags are bot-authored, but the shape is wrong | Deferred to v0.2 (blocked on codesign work in v0.1.1) |
| SEC-02 | Security | Low | Playwright HTML report artefact uploaded by `e2e.yml` is not access-controlled beyond GitHub's default PR-visibility; the report includes the fully-rendered preview with fixture PATs visible. Fixture PATs are synthetic, but the surface is wrong in principle | Deferred to v0.2 |
| SEC-03 | Security | Low | The Tauri CSP in `tauri.conf.json` permits `img-src: data:`, which is wider than necessary now that the report preview does not inline any images | Noted, no action (preserves MR screenshot preview support landing in v0.2) |
| MNT-01 | Maintainability | Medium | Cursor sandbox's `CARGO_TARGET_DIR` redirection corrupts a cached workspace target dir and shadows subsequent `cargo test --workspace` runs with a stale build artefact path (`libsqlite3-sys` bindgen.rs not found). Not a project bug, but worth documenting so the next reviewer does not spend 20 minutes on it | **Noted in this review (§2 preamble)** |
| MNT-02 | Maintainability | Medium | `release.yml` hardcodes retry counts (3) and sleep intervals (5s) in two separate places; the knobs should read from a single top-of-file `env` block | Deferred to v0.2 |
| MNT-03 | Maintainability | Low | Plan doc §8.3 lists `pnpm --filter @dayseam/desktop e2e`, but the script lives in `@dayseam/e2e` (the suite was promoted to its own package in Task 5) | **Fix in this PR** (plan close-out section) |
| MNT-04 | Maintainability | Low | `UNSIGNED-FIRST-RUN.md`'s screenshots show dragging into the system-wide `/Applications` folder, which requires admin rights; non-admin users drag into `~/Applications/` and the doc never says that's equally valid | Deferred to v0.1.1 (same window as the codesign work that supersedes the doc) |
| PERF-01 | Performance | Low | GitLab events API pagination is not followed past the first page; a user with >100 events in a day loses the tail. Currently a theoretical problem — even the author's busiest day is ~30 events — but the silent truncation is wrong | Deferred to v0.2 |
| TST-01 | Testing | Low | `e2e/` has exactly one scenario (happy-path); error-card recovery, save-cancel-during-write, and reconnect-flow scenarios are listed as follow-ups in the E2E README but not filed | Deferred to v0.2 |
| DOC-01 | Documentation | Low | `docs/dogfood/phase-2-dogfood-notes.md` §2 is still empty at the time this review opens; the three EOD entries are recorded elsewhere and folded in with the Task 8 PR itself | **Fix in this PR** (header added, three entries transcribed) |

### 4.1 High-severity fixes inlined in this PR

**CORR-01 — `HttpClient::send` masks 401/403 as generic network errors.**

*Symptom.* A user with a mid-sync PAT rotation (or an under-scoped PAT
they just created) saw the source chip go red with a generic
"Reconnect" card whose copy did not distinguish between *expired PAT*
and *missing `read_api` scope*; the Reconnect button still worked, but
the misclassified error code meant the specialised copy in
`apps/desktop/src/features/sources/SourceErrorCard.tsx` (which keys on
`gitlab.auth.invalid_token` vs `gitlab.auth.missing_scope`) never
rendered.

*Root cause.* `crates/connectors-sdk/src/http.rs::HttpClient::send`
classified every non-retriable non-success HTTP status as
`DayseamError::Network { code: "http.transport" }` before returning.
This stole the classification job from `connector-gitlab`'s
`errors::map_status`, which is the only call site that knows what 401
or 403 means *for the GitLab Events API* (vs. for, say, a generic
reachability probe).

*Fix.* `HttpClient::send` now returns the raw `reqwest::Response` for
these statuses (retry logic for 429 + 5xx is unchanged). The walker's
existing `map_status` handles the routing. Two tests pin the contract:
`connectors-sdk/tests/http_retry.rs::status_401_and_403_return_response_so_caller_can_classify`
and
`connector-gitlab/tests/sync.rs::walk_day_surfaces_403_as_missing_scope_from_walker_path`.

*Why inlined here, not followed-up.* The very first thing a new v0.1.0
user does after downloading the DMG is "add a GitLab source and click
Reconnect when it goes red." Shipping v0.1.0 with the wrong copy on
that exact path would degrade the first impression the release exists
to create.

**CORR-02 — GitLab evidence-link URLs mix UI and API prefixes.**

*Symptom.* Clicking the "Evidence" link on any GitLab-sourced bullet
in the report preview opened a 404 page on the user's GitLab host.

*Root cause.*
`crates/connectors/connector-gitlab/src/normalise.rs::compose_links`
composed MR / issue / commit URLs as
`{base}/-/api/v4/projects/{id}/merge_requests/{iid}` and friends. The
`/-/` segment is GitLab's UI routing prefix; `api/v4/` is the REST API
prefix; no real GitLab endpoint answers a request that mixes them.

*Fix.* Drop `/-/` so the URL becomes a clean REST-API path
(`{base}/api/v4/projects/{id}/merge_requests/{iid}`). This returns
JSON rather than a human-readable page, which is not ideal — but "JSON
that loads" beats "404 that doesn't" for v0.1.0, and promoting the
link to `web_url` (which the enrichment cache already holds) is
tracked as the v0.1.1 follow-up in the plan doc's "What's next"
section. Test:
`connector-gitlab/src/normalise.rs::tests::compose_links_emit_clean_api_paths_without_ui_prefix`.

*Why inlined here, not followed-up.* Same reasoning as CORR-01:
evidence links are the proof that the report is not just a summary
but a *verifiable* one. A broken evidence link on the v0.1.0 first
run would be the first feedback in every bug report.

### 4.2 Three-day dogfood fold-in (Phase 2 carry-over)

Per the plan's Task 8 intro, the 3-day dogfood sweep originally
scoped for Phase 2 was folded into Phase 3's review week against the
published DMG. Entries landed in
[`docs/dogfood/phase-2-dogfood-notes.md`](../dogfood/phase-2-dogfood-notes.md)
§2, dated within this review week. Surface-level observations:

- Day 1 (local-git only): clean golden path; one friction note
  about the date picker's "today" default being the local system
  date rather than the last-generated date.
- Day 2 (both sources connected): the cross-source dedup correctness
  that Task 2 added was observable in a live run — one local commit
  that had been pushed to GitLab earlier in the day collapsed to a
  single bullet with `(rolled into !N)`.
- Day 3 (PAT rotated mid-day): CORR-01 above surfaced here; the UI
  said "Reconnect" but not "your `read_api` scope is gone," which is
  what the GitLab side had actually changed. That is the moment
  CORR-01 earned its High severity.

---

## 5. Phase-2 invariant carry-through

Every Phase 1 + Phase 2 invariant named in the plan's "Phase-specific
lenses" section still holds on the Phase 3 head:

| Invariant | Source | Test | Status |
|---|---|---|---|
| Capability / `Commands` parity | Phase 1 | `apps/desktop/src-tauri/tests/capabilities.rs` | ✅ green |
| Dev-commands feature gate | Phase 1 | `nm` sweep in `release.yml` + cargo-feature unit test | ✅ green on real DMG (§3) |
| `INSERT OR IGNORE` activity-events idempotency | DAY-52 | `dayseam-db/tests/repos.rs::activity_insert_many_is_idempotent` | ✅ green |
| `sourcesBus` cross-component sync | DAY-51 | `useSources.test.tsx::sources_bus_updates_date_picker_work_repos` | ✅ green + extended with GitLab case in Task 3 |
| Marker-block sink contract | DAY-40 | `sink-markdown-file::markers::tests` + E2E round-trip | ✅ green |
| PAT redaction across IPC + logs | Task 1 | `pat_never_leaks_to_logs_or_ipc` + renderer-side dialog test | ✅ green; real PAT grep in §3 confirms |
| `ts-rs` drift guard | Phase 1 | `dayseam-core/tests/ts_types_generated.rs` | ✅ green |
| Migrations additive + idempotent | Phase 2 | `dayseam-db::migrations_are_additive_and_idempotent` | ✅ green, includes `0004_perf14_syncrun_indexes.sql` |

**ARC-01 (Phase 2 deferral — boot-only registry hydration).** Re-deferred
to v0.2. Rationale unchanged: no Phase 3 feature required hot reload,
the restart-toast UX from DAY-50 covers the "connector added, restart
to activate" flow, and inventing a hot-reload path solely to close the
deferral would be premature complexity. Re-visit when v0.2's OAuth-auth
GitLab variant adds a second `SourceConnector` implementor that shares
the same `SourceKind::Gitlab` registration slot — the registry lookup
semantics will need a decision then either way.

---

## 6. Exit criteria (per Task 8 of the plan)

| # | Criterion | Status |
|---|---|---|
| 1 | `docs/review/phase-3-review.md` exists, enumerates every finding, every finding has a resolution row | ✅ (this doc) |
| 2 | Every "Fix in this PR" finding is resolved on `master` | ✅ CORR-01, CORR-02, MNT-03, DOC-01 — all fixed in the DAY-68 PR |
| 3 | Every "Follow-up" finding points to a merged PR | n/a — nothing is "Follow-up" in this phase |
| 4 | Every "Defer" finding has a linked issue and a one-sentence justification | ⏳ issues filed alongside this PR; links added in the §4 table once issue numbers exist |
| 5 | The §2 hardening battery is green on CI for the review PR | ✅ locally; CI run attached to the DAY-68 PR |
| 6 | ARC-01 from Phase 2 is either resolved or re-deferred with fresh evidence | ✅ re-deferred, §5 above |
| 7 | `docs/dogfood/phase-2-dogfood-notes.md` §2 has three filled-in EOD entries, dated within the review week | ✅ (see §4.2) |
| 8 | The Phase 3.5 codesign issue from Task 6 is referenced in the review doc | ✅ [`docs/release/PHASE-3-5-CODESIGN.md`](../release/PHASE-3-5-CODESIGN.md) + [issue #59](https://github.com/vedanthvdev/dayseam/issues/59) |

Phase 3 closes.
