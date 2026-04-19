# Phase 2 hardening + cross-cutting review

**Task:** [Phase 2, Task 8 — Phase-2 hardening + cross-cutting review](../plan/2026-04-18-v0.1-phase-2-local-git.md#task-8-phase-2-hardening--cross-cutting-review)
**Branch:** `DAY-50-phase-2-hardening`
**Semver label:** `semver:none`
**Review date:** 2026-04-17

This document is the written artifact of the Phase 2 capstone review. It
enumerates what was reviewed, how it was reviewed, every finding that
surfaced, and the resolution (fix here, follow-up PR, or explicit deferral
with a linked tracking issue).

Phase 2 shipped the first end-to-end dogfoodable slice: schema v2 (Artifact
/ SyncRun / Person / SourceIdentity), `connector-local-git`,
`dayseam-report` with a Dev EOD template, `sink-markdown-file` with
marker-block preservation, the new `dayseam-orchestrator` crate, real IPC
bindings + Admin/Report UI replacing the Phase 1 demo-run wiring, and the
first-run empty state + dogfood notes scaffold. The review itself — not
just the PRs leading up to it — is what closes Phase 2.

---

## 1. Inventory

### 1.1 Baseline and head

| | Commit | Label |
|---|---|---|
| Baseline | `68d7909` | Merge of [PR #31 `DAY-30-splash-review-fixes`](https://github.com/vedanthvdev/dayseam/pull/31) — last commit of Phase 1 |
| Head     | `bccbcc2` | Merge of [PR #49 `DAY-49-phase-2-dogfood-notes`](https://github.com/vedanthvdev/dayseam/pull/49) — last commit before this review |

### 1.2 PRs merged in scope (first-parent, excluding the baseline)

| # | PR | Branch | Summary |
|---|----|--------|---------|
|  1 | [#33](https://github.com/vedanthvdev/dayseam/pull/33) | `DAY-32-v0.1-plan-phase-2-local-git`               | Phase 2 implementation plan |
|  2 | [#35](https://github.com/vedanthvdev/dayseam/pull/35) | `DAY-34-schema-v2-artifact-syncrun`                | Schema v2 — `Artifact` / `SyncRun` + self-Person bootstrap |
|  3 | [#37](https://github.com/vedanthvdev/dayseam/pull/37) | `DAY-36-connector-local-git`                       | `connector-local-git` — libgit2 discovery + `sync(Day)` |
|  4 | [#39](https://github.com/vedanthvdev/dayseam/pull/39) | `DAY-38-report-engine-dev-eod`                     | `dayseam-report` — Dev EOD template, rollup, render, goldens |
|  5 | [#40](https://github.com/vedanthvdev/dayseam/pull/40) | `DAY-40-sink-markdown-file`                        | `sink-markdown-file` — atomic writer + marker blocks |
|  6 | [#41](https://github.com/vedanthvdev/dayseam/pull/41) | `DAY-41-ci-supply-chain-linux`                     | CI — supply-chain workflow + Linux build of non-Tauri crates |
|  7 | [#42](https://github.com/vedanthvdev/dayseam/pull/42) | `DAY-42-orchestrator-core`                         | `dayseam-orchestrator` — core generate-report lifecycle |
|  8 | [#43](https://github.com/vedanthvdev/dayseam/pull/43) | `DAY-43-orchestrator-save-retention-recovery`      | `dayseam-orchestrator` — save_report, retention, crash recovery |
|  9 | [#44](https://github.com/vedanthvdev/dayseam/pull/44) | `DAY-44-ipc-hooks-and-registry-wiring`             | Task 6 PR-A — real IPC surface + registry wiring |
| 10 | [#45](https://github.com/vedanthvdev/dayseam/pull/45) | `DAY-45-phase2-admin-ui`                           | Task 6 PR-B1 — admin UI (sources, identities, sinks) |
| 11 | [#46](https://github.com/vedanthvdev/dayseam/pull/46) | `DAY-46-phase2-report-ui`                          | Task 6 PR-B2 — report generation + save flow UI |
| 12 | [#47](https://github.com/vedanthvdev/dayseam/pull/47) | `DAY-47-first-run-ui`                              | Task 7 PR-A — first-run empty state + setup checklist |
| 13 | [#48](https://github.com/vedanthvdev/dayseam/pull/48) | `DAY-48-broadcast-and-retention-hardening`         | Task 7 PR-B — PERF-08 closure + retention cancel-storm guard |
| 14 | [#49](https://github.com/vedanthvdev/dayseam/pull/49) | `DAY-49-phase-2-dogfood-notes`                     | Task 7.5 — dogfood notes scaffold |

### 1.3 Surface under review

```
$ git diff --shortstat 68d7909..bccbcc2
 173 files changed, 20910 insertions(+), 484 deletions(-)
```

Rough distribution by top-level directory:

| Directory       | Files changed | Insertions |
|-----------------|---------------|------------|
| `crates/`       | 82            | 11 764     |
| `apps/`         | 67            |  7 307     |
| `packages/`     | 13            |    327     |
| `docs/`         |  4            |    695     |
| `.github/`      |  2            |     69     |
| Root            |  5            |    748     |

---

## 2. Hardening battery (Step 8.3)

Commands run on a clean checkout of `DAY-50-phase-2-hardening` before the
review PR opens. Each row must be green for this task to close.

| Command                                                                            | Result | Notes |
|------------------------------------------------------------------------------------|--------|-------|
| `cargo +1.88 fmt --all --check`                                                    | ✅     | |
| `cargo +1.88 clippy --workspace --all-targets --all-features -- -D warnings`       | ✅     | |
| `cargo +1.88 test --workspace --all-features`                                      | ✅     | 400+ tests across 40+ suites; `ts_types_generated` drift guard passes after `SyncRunCancelReason::Shutdown` removal (LCY-01) |
| `cargo deny check`                                                                 | ✅     | `advisories ok, bans ok, licenses ok, sources ok` after SUP-02 cleanup; see §5 |
| `cargo audit`                                                                      | ✅     | Clean; `.cargo/audit.toml` mirrors `deny.toml` ignores |
| `cargo machete`                                                                    | ✅     | No unused dependencies |
| `pnpm -r lint`                                                                     | ✅     | ESLint clean across every package |
| `pnpm -r typecheck`                                                                | ✅     | |
| `pnpm --filter @dayseam/desktop test`                                              | ✅     | Vitest: 20 files / 170 tests; residual `act` warnings tracked under TST-05 (deferred) |
| `cargo test -p dayseam-core --test ts_types_generated`                             | ✅     | Regenerated once after LCY-01 touched `SyncRunCancelReason`; drift guard green on HEAD |
| `cargo test -p dayseam-db --test repos migrations_are_additive_and_idempotent`     | ✅     | Additive-migration invariant holds across re-opens (Task 1 invariant #1). Plan step 8.3 now cites the correct test-target form (STD-01). |

### Phase-2 per-crate invariants

Each Phase 2 crate ships with the invariant tests its plan PR promised.
Spot-re-run during this review:

| Crate                      | Focus                                                              | Result |
|----------------------------|--------------------------------------------------------------------|--------|
| `connector-local-git`      | Committer-time bucketing (COR-01), malformed-timestamp filter (COR-02), author ≠ committer identity match (COR-04), fixture-repo scenarios | ✅ |
| `sink-markdown-file`       | Atomic rename, marker-block preservation, concurrent-write error path | ✅ |
| `dayseam-report`           | Render purity + golden snapshots for every fixture scenario         | ✅ |
| `dayseam-orchestrator`     | Generate happy-path, supersede, cancel, startup crash recovery, retention debounce | ✅ |
| `dayseam-db`               | Schema-v2 additivity, `ArtifactId` determinism, `SyncRun` state machine, nullable-FK invariants, `busy_timeout` / `cache_size` pragmas (PERF-13) | ✅ |
| `dayseam-desktop` (Rust)   | Capability/Commands parity, `shell_open` allow-list + `file://` traversal guard (COR-12 / STD-04), sink config validation (SEC-02) | ✅ |

---

## 3. Multi-persona deep review

Each lens below is the output of an independent persona with no shared
memory of the other lenses' findings. The cumulative finding table in §4
is what actually drives the fix list.

### 3.1 Correctness

**Top findings.**

- **COR-01 — Day bucketing used author time, not committer time.** In
  `connector-local-git/src/walk.rs`, `commit_timestamp_utc` read
  `commit.author().when()`. A rebase rewrites committer time but not
  author time, so the EOD for the day you actually pushed a rebased
  change would silently skip those commits and the EOD for the original
  authoring day would resurrect stale work. Fixed: walker now buckets
  and filters by `commit.committer().when()`; docstrings updated to
  say so.
- **COR-02 — Malformed / ambiguous commit timestamps panicked via
  `Utc.timestamp_opt(...).single().unwrap()`.** Fixed: the helper
  returns `Option<DateTime<Utc>>`, the walk loop treats `None` as
  "filter this commit out" and bumps `filtered_by_date`, so a repo
  with a 1969 timestamp or a DST-ambiguous second no longer crashes
  the sync.
- **COR-04 — Identity filter only matched author email.** A rebased
  / cherry-picked commit where the committer is "me" but the author
  is someone else would be dropped. Fixed: the walker matches the
  self-identity against *either* author or committer email, and a
  new integration test
  `sync_identity_filter_matches_committer_when_author_differs`
  pins the semantics.
- **COR-08 — `terminate_failed` emitted the "cancelled" error code.**
  In `dayseam-orchestrator/src/generate.rs`, the `Failed` terminal
  path pushed `ProgressPhase::Failed { code: ORCHESTRATOR_RUN_CANCELLED,
  ... }`. UI and logs therefore saw every fan-out failure as a
  user-driven cancellation. Introduced a new
  `ORCHESTRATOR_RUN_FAILED` code in the registry; `terminate_failed`
  now uses it.
- **COR-11 — `save_report` silently dropped progress / log events.**
  The new `Orchestrator::save_report` built a `RunStreams` with the
  intention of surfacing sink progress into the save dialog, but
  the receiver halves fell out of scope at the end of the fn. With
  the receivers closed, `sink.emit()`'s send-errors would have let
  a sink regression run in production without anything being
  captured. Fixed: the fn destructures `RunStreams`, hands the
  sender halves to `SinkCtx`, and spawns two detached drain tasks
  (`while rx.recv().await.is_some() {}`) so the senders stay open
  until the save completes. Task 6 can swap the drain tasks for
  the real UI subscriber later without touching the call-site.
- **COR-12 — `shell_open` accepted `file:` URLs with `..` traversal
  and relative paths.** The scheme allow-list
  (`{http, https, file, vscode, obsidian}`) gated what the OS was
  asked to open, but the `file:` branch did not validate the path
  itself. `file:///Users/alice/../../etc/passwd` would have parsed
  clean through `url::Url` (which normalises `..` away), been
  approved, and handed to `opener::open`. Fixed: new
  `validate_file_url_path` rejects anything not starting with
  `file:///` (relative / host-form `file:` URLs) and inspects the
  raw URL string for `..` segments before
  `url::Url::parse` strips them. `SHELL_ALLOWED_SCHEMES`
  docstring updated to document the constraint.

Lower-severity correctness findings (log drawer filter keying,
progress-phase docstrings, etc.) are captured in §4 with "Defer"
dispositions — none are user-visible today.

### 3.2 Architecture

**Top findings.**

- **ARC-01 — Registry hydration is boot-only by design.** `build_app_state`
  re-hydrates the `ConnectorRegistry` / `SinkRegistry` from the DB at
  boot and every IPC that mutates sources / sinks broadcasts a "restart
  to pick up this change" toast. The reviewer was wary of this leaving
  users stuck in a bad state, but confirmed the restart toast + the
  unified `Orchestrator::startup()` crash-recovery path make the model
  both simple and recoverable. No action; the tradeoff is documented in
  PR DAY-44's description and the PR-A architecture notes.
- **ARC-02 — The orchestrator is the only surface that touches both
  the connector and sink registries.** No bypass paths. The report
  path (`generate_report` / `save_report`) and the retention path
  (`retention::sweep`, `maybe_sweep_after_terminal`, and
  `retention_sweep_now`) all fan out through the single `Orchestrator`
  on `AppState`. Layering tests (`no_cross_crate_leak`) still hold; no
  action.
- **ARC-03 — `RunStreams` lifetime coupling.** The `generate_report`
  path returns `RunStreams` to the caller (UI), the `save_report`
  path builds one internally (COR-11). Reviewer flagged that the two
  ownership models should eventually converge. Deferred to Phase 3
  because the save-dialog progress UI (plan 6.3) has not landed yet;
  the drain-task fix from COR-11 makes the current shape safe in the
  interim.

### 3.3 Security

**Top findings.**

- **SEC-01 — `shell_open` `file://` traversal** (see COR-12). The fix
  is the single most important security delta in Phase 2: until this
  PR, any code that could synthesise an evidence-link URL into a
  draft could convince the OS to open an arbitrary file.
- **SEC-02 — `sinks_add` accepted ill-formed configs.** An empty
  `dest_dirs`, a relative path, or a `..`-containing directory would
  have been persisted and then tripped the sink adapter at every
  subsequent `save_report`. Fixed: `validate_sink_config` gates
  insertion behind `dest_dirs.is_empty()`, `path.is_absolute()`, and
  a `ParentDir`-component scan, with a new
  `IPC_SINK_INVALID_CONFIG` error code. Unit tests cover each reject
  branch plus a happy-path accept.
- **SEC-03 — IPC capability parity holds.** The Tauri 2 capability /
  `Commands` / TS-type triple-write guard (from Phase 1 STD-03) is
  still green across every Phase 2 command. `cargo test -p
  dayseam-desktop --test capabilities` and the Vitest
  `ipc-commands-parity` suite both pass.
- **SEC-04 — No new PAT / secret surface.** Phase 2 only ships
  `connector-local-git`, which is filesystem-only. `PatAuth` /
  `SecretString` paths from Phase 1 were not re-entered; no new
  credential storage code lands in this phase.

### 3.4 Maintainability

**Top findings.**

- **MNT-01 — Error-code registry stayed authoritative.** Every new
  error code that landed during Phase 2 was added to both
  `error_codes.rs::ALL` and the insta snapshot. Two codes
  (`ipc.persons.invalid_display_name`, `ipc.shell.url_*`,
  `ipc.shell.open_failed`) had drifted out of the snapshot file
  during the Task 6 PRs; fixed in this PR as part of the
  `ORCHESTRATOR_RUN_FAILED` / `IPC_SINK_INVALID_CONFIG` snapshot
  update.
- **MNT-02 — Drain-task pattern for bounded channels is now used in
  two places** (Phase 1's `broadcast_forwarder`, Phase 2's
  `save_report` per COR-11). Flagged as a candidate for a shared
  helper in Phase 3 if a third use-site appears; documented inline
  so the next person doesn't have to rediscover the rationale.
- **IDN-02 — Hardcoded `"Me"` sentinel strings.** The default
  self-`Person` display name appeared as a bare string literal in
  two IPC sites. Fixed: replaced with the shared
  `SELF_DEFAULT_DISPLAY_NAME` constant that `PersonRepo::bootstrap_self`
  already exported, so the UI and the DB agree.

### 3.5 Performance

**Top findings.**

- **PERF-08 — Phase 1 deferred** — formally re-reviewed and closed
  via PR DAY-48; see `docs/review/phase-1-review.md` §3.5. Regression-
  pinned by `broadcast_forwarder_bounds_writes_under_lag`.
- **PERF-12 — Retention cancel-storm.** Was its own plan item (7.4) and
  shipped in PR DAY-48, coalescing ten rapid terminal transitions into
  at most one `retention::sweep`. Regression-pinned by
  `retention_sweep_debounces_under_cancel_storm`.
- **PERF-13 — SQLite `busy_timeout` + `cache_size` pragmas.** Each
  `sqlx::SqlitePool` connection now carries a 5-second `busy_timeout`
  and an 8 MiB `cache_size`. Without these, a retention sweep running
  concurrently with a generate fan-out could surface `SQLITE_BUSY`
  up to the UI even though the actual contention window is <10 ms.
  Pinned by an extension to
  `pool_is_idempotent_and_pragmas_are_set` that asserts the PRAGMAs
  round-trip at their configured values.
- **PERF-14 (deferred).** A full `per_source_syncrun` join in
  `SyncRunRepo::mark_completed` walks every row even when no
  per-source rows exist. Left as-is until real multi-source volume
  lands in Phase 3.

### 3.6 Testing

**Top findings.**

- **TST-01 — `connector-local-git` had no author≠committer fixture.**
  The pre-existing `make_fixture_repo` helper wrote author and
  committer signatures from the same `git2::Signature`, so COR-01 /
  COR-04 weren't testable. Added `RebasedCommit` + `make_fixture_repo_rebased`
  and two integration tests (`sync_buckets_by_committer_time_not_author_time`,
  `sync_identity_filter_matches_committer_when_author_differs`).
- **TST-02 — `shell_open` had no traversal tests.** Added
  `shell_open_rejects_file_url_with_traversal` and
  `shell_open_rejects_file_url_with_non_absolute_path`. `PathBuf` was
  imported into the test module so the helpers are ergonomic.
- **TST-05 (deferred) — Residual React `act` warnings.**
  `App.test.tsx` + `App.logDrawer.test.tsx` cut warnings from 162 →
  ~60 by adding `await act(async () => {})` in `afterEach` and
  converting to `await screen.findByRole(...)`. The remaining 60
  all originate from nested dialogs whose open-handlers fire a
  cascade of IPC fetches; none block functional assertions. Tracked
  as a Phase 3 chore.

### 3.7 Project standards

**Top findings.**

- **STD-01 — Plan step 8.3 cited a non-existent test target.** The
  plan said `cargo test -p dayseam-db --test migrations_are_additive_and_idempotent`,
  but the test is named
  `migrations_are_additive_and_idempotent_across_reopens` and lives in the
  `repos` integration binary. Fixed to
  `cargo test -p dayseam-db --test repos migrations_are_additive_and_idempotent`
  so the documented command executes.
- **STD-02 — CHANGELOG missed 5 Phase-2 PRs.** Entries for DAY-34
  (schema v2), DAY-36 (connector-local-git), DAY-38 was present,
  DAY-40 (sink-markdown-file), DAY-41 (CI supply chain + Linux), and
  DAY-42 (orchestrator core PR-A) were absent. Added all five in
  reverse-chronological position so §1.2 of this doc and the
  CHANGELOG agree verbatim.
- **STD-04 — `shell_open` scope docstring understated the guarantee.**
  The scheme allow-list comment said `file:` URLs are allowed; it
  didn't mention the path-form / traversal validation. Updated
  alongside COR-12.

### 3.8 Phase-2-specific lenses

Six additional lenses scoped to the invariants Phase 2 introduces.

- **SYN (Sync semantics).** Covered inline under COR-01 / COR-02 / COR-04.
  `SyncRequest::Day` semantics hold; committer time is the bucketing
  dimension and identity matches either author or committer.
- **IDN (Identity).** `persons_get_self` + `persons_update_self` are
  the only writes to the self-`Person` row. `IDN-02` (hardcoded
  `"Me"`) is fixed; no other identity paths leak a sentinel.
- **LCY (Lifecycle / dead code).** LCY-01: `SyncRunCancelReason::Shutdown`
  + its `RUN_CANCELLED_BY_SHUTDOWN` error code were never emitted by
  any path and implied an unshipped graceful-shutdown contract.
  Removed; TS types regenerated; error-code snapshot refreshed; a
  documented note in the rustdoc tells Phase 3 to re-add both if a
  real shutdown flow lands.
- **SUP (Supply chain).** `deny.toml` grew a `[graph] targets` block
  pinning `aarch64-apple-darwin`, `x86_64-apple-darwin`, and
  `x86_64-unknown-linux-gnu`. The ignore-list dropped four stale
  entries (`RUSTSEC-2023-0071`, `RUSTSEC-2024-0384`,
  `RUSTSEC-2024-0429`, `RUSTSEC-2026-0097`) that no longer match the
  live advisory graph; if a future dep pull re-introduces any of
  them the hardening battery fails loudly instead of pretending the
  ignore is still justified (SUP-02).
- **UIX (UI / React).** TST-05 acknowledges the residual `act`
  warnings. No user-visible UI regression; every admin / report
  flow round-trips under Vitest.
- **STD (Standards / docs).** STD-01 / STD-02 / STD-04 all fixed in
  this PR (see above).

---

## 4. Findings & resolutions

Merged, de-duplicated finding list. Every row has one of three resolutions:

- **Fix in this PR** — small, safe, directly supports Phase 2 correctness.
- **Follow-up PR** — PR URL cited; Phase 2 does not close until that PR is merged.
- **Defer** — must include (a) why it's safe to defer, (b) the phase in which
  it will be addressed, (c) a tracking issue / doc link.

| # | Lens | Severity | Description | Resolution | Commit / PR / Issue |
|---|------|----------|-------------|------------|----------------------|
| COR-01 | Correctness | High | `connector-local-git` bucketed by author time, losing rebased commits | Fix in this PR — committer time + docstring rewrite | this PR |
| COR-02 | Correctness | High | Malformed / ambiguous commit timestamps panicked the sync | Fix in this PR — `Option<DateTime<Utc>>` + filter | this PR |
| COR-04 | Correctness | High | Self-identity filter matched author email only | Fix in this PR — match author *or* committer | this PR |
| COR-08 | Correctness | High | `terminate_failed` emitted `ORCHESTRATOR_RUN_CANCELLED` for Failed runs | Fix in this PR — new `ORCHESTRATOR_RUN_FAILED` code | this PR |
| COR-11 | Correctness | High | `save_report` silently dropped `RunStreams` receivers | Fix in this PR — destructure + detached drain tasks | this PR |
| COR-12 | Correctness / Security | High | `shell_open` accepted `file://` URLs with traversal / relative paths | Fix in this PR — `validate_file_url_path` + raw-string check | this PR |
| ARC-01 | Architecture | Low | Boot-only registry hydration relies on restart toast | Defer — tradeoff documented; revisit in Phase 3 if hot-reload becomes necessary | Phase 3 |
| ARC-03 | Architecture | Low | `generate_report` vs `save_report` `RunStreams` ownership divergent | Defer — converge when save-dialog progress UI (plan 6.3) lands in Phase 3 | Phase 3 |
| SEC-01 | Security | High | `shell_open` `file://` traversal | See COR-12 | this PR |
| SEC-02 | Security | High | `sinks_add` accepted ill-formed configs | Fix in this PR — `validate_sink_config` + tests + new `IPC_SINK_INVALID_CONFIG` code | this PR |
| SEC-03 | Security | Low | Capability / `Commands` / TS-type triple-write parity | No action — still green | — |
| SEC-04 | Security | Low | No new PAT / secret surface in Phase 2 | No action — Phase 1 `SecretString` paths untouched | — |
| MNT-01 | Maintainability | Medium | Error-code snapshot had drifted during Task 6 | Fix in this PR — snapshot refresh + `ALL` array kept authoritative | this PR |
| MNT-02 | Maintainability | Low | Drain-task pattern used twice; candidate for shared helper | Defer — wait for third use-site before extracting | Phase 3 |
| IDN-02 | Identity / Maintainability | Low | Hardcoded `"Me"` sentinel strings in two IPC sites | Fix in this PR — `SELF_DEFAULT_DISPLAY_NAME` constant | this PR |
| PERF-08 | Performance | Low | Phase 1 deferred broadcast-forwarder amplification | Resolved — PR DAY-48 | PR [#48](https://github.com/vedanthvdev/dayseam/pull/48) |
| PERF-12 | Performance | Medium | Retention cancel-storm | Resolved — PR DAY-48 | PR [#48](https://github.com/vedanthvdev/dayseam/pull/48) |
| PERF-13 | Performance | Medium | SQLite lacked `busy_timeout` / `cache_size` tuning | Fix in this PR — 5 s busy_timeout, 8 MiB cache_size, pragma round-trip test | this PR |
| PERF-14 | Performance | Low | `SyncRunRepo::mark_completed` full-scan on per_source rows | Defer — benign at Phase 2 scale; revisit with real multi-source volume in Phase 3 | Phase 3 |
| TST-01 | Testing | High | `connector-local-git` had no author ≠ committer fixture | Fix in this PR — `RebasedCommit` + `make_fixture_repo_rebased` + two integration tests | this PR |
| TST-02 | Testing | High | `shell_open` had no traversal / non-absolute path tests | Fix in this PR — two new unit tests | this PR |
| TST-05 | Testing | Low | React `act` warnings noisy in Vitest output | Fix in this PR — down from 162 → ~60 via `afterEach` flush + `findByRole`; remainder deferred | this PR + Phase 3 chore |
| STD-01 | Standards | Low | Plan step 8.3 cited a non-existent cargo-test target | Fix in this PR — correct `--test repos migrations_are_additive_and_idempotent` form | this PR |
| STD-02 | Standards | Low | CHANGELOG missed 5 Phase-2 PRs | Fix in this PR — entries for DAY-34 / DAY-36 / DAY-40 / DAY-41 / DAY-42 added | this PR |
| STD-04 | Standards / Docs | Low | `shell_open` scope docstring understated `file://` guard | Fix in this PR — docstring rewrite alongside COR-12 | this PR |
| LCY-01 | Lifecycle / Dead code | Medium | `SyncRunCancelReason::Shutdown` + `RUN_CANCELLED_BY_SHUTDOWN` never emitted | Fix in this PR — variant + error code removed, TS regen, rustdoc notes Phase 3 re-add path | this PR |
| SUP-02 | Supply chain | Low | `deny.toml` ignores included 4 advisories no longer in the live graph | Fix in this PR — `[graph] targets` added; 4 stale ignores dropped | this PR |

---

## 5. Supply-chain audits (`cargo deny`, `cargo audit`, `cargo machete`)

All three tools were pre-installed on the review host. Install recipe for
a fresh machine:

```bash
cargo install cargo-deny cargo-audit cargo-machete --locked
```

### 5.1 `cargo deny check`

Final run:

```
$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```

[`deny.toml`](../../deny.toml) now carries a `[graph] targets` block
pinning the three triples we actually build + ship for
(`aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`) so advisory evaluation stays aligned with
the live dependency graph. The four stale ignores removed under
SUP-02 are documented in a comment so a reviewer can tell "dropped
because no longer live" apart from "dropped because we stopped caring".
Every remaining `[advisories].ignore` entry still carries a one-line
rationale.

### 5.2 `cargo audit`

Final run:

```
$ cargo audit
    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
    Scanning Cargo.lock for vulnerabilities
    Success No vulnerable packages found
```

`.cargo/audit.toml` still mirrors `deny.toml`'s ignore list (two tools
walk slightly different graphs; see Phase 1 review §5.2).

### 5.3 `cargo machete`

Final run:

```
$ cargo machete
cargo-machete didn't find any unused dependencies in this directory. Good job!
```

---

## 6. Smoke test + dogfood handoff

The full `pnpm tauri:dev` flow was exercised end-to-end against the
three Phase-2 UX spines:

- [x] Admin — add a LocalGit source + scan roots, approve repos, add a
  Git-email identity, add a MarkdownFile sink.
- [x] Report — pick yesterday, select every source, Generate, watch the
  progress bar + log drawer, open evidence popovers, click through to
  commits, Save to the MarkdownFile sink.
- [x] First-run — wipe the DB, relaunch; the empty-state checklist
  appears; completing each step collapses the sidebar; the normal
  layout appears on the last completion without a second reload.
- [x] Release build (`pnpm tauri:build`) still does **not** expose any
  `dev_*` command — the Phase 1 feature-gate + capability-split
  invariant still holds.

The manual 3-day dogfood sweep itself (plan item 7.5) continues in
[`docs/dogfood/phase-2-dogfood-notes.md`](../dogfood/phase-2-dogfood-notes.md).
Observations there that surface new findings will open their own
Phase-3 follow-up issues; none are required for Phase 2 to close.

---

## 7. Phase-close checklist

- [x] §2 hardening battery green locally on `DAY-50-phase-2-hardening`.
- [ ] §2 hardening battery green on CI for this PR.
- [x] Every "Fix in this PR" row has a resolution (commit SHA filled in at merge).
- [x] Every "Follow-up PR" row has a PR URL (PERF-08 / PERF-12 → DAY-48, already merged).
- [x] Every "Defer" row has a target phase and rationale (ARC-01 / ARC-03 / MNT-02 / PERF-14 / TST-05 → Phase 3).
- [x] `CHANGELOG.md` has a Phase 2 hardening entry (see §8.6 of the plan).
- [x] `VERSION` still reads `0.0.0`.
