# Changelog

All notable changes to Dayseam are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Release engineering — universal `.dmg`, GitHub Release workflow,
  Gatekeeper-bypass README (DAY-59).** Phase 3 Task 6 lands the
  release-automation spine v0.1.0 will ship through. A new
  `.github/workflows/release.yml` (macOS-latest runner,
  `contents: write` and nothing else) triggers on merged PRs carrying
  `semver:patch`/`minor`/`major` labels — and on manual
  `workflow_dispatch` for step 6.5's dry-run — pre-flights the
  CHANGELOG's `[Unreleased]` section for non-emptiness, resolves the
  target version via `scripts/release/bump-version.sh`, builds the
  universal bundle via `scripts/release/build-dmg.sh` (which runs
  `pnpm --filter @dayseam/desktop exec tauri build --target
  universal-apple-darwin` and copies the `.dmg` to
  `dist/release/Dayseam-vX.Y.Z.dmg` with a sibling `.sha256`),
  asserts the fused binary carries both `arm64` and `x86_64` slices
  via `lipo -archs`, and `nm`-greps the binary to hold the Phase 1
  dev-commands-feature-gate invariant (`dev_emit_toast` /
  `dev_start_demo_run` must not be present in a release build). On
  real runs it commits `chore(release): vX.Y.Z`, tags `vX.Y.Z`, and
  creates a GitHub Release with the `.dmg` + `.sha256` attached and
  the CHANGELOG-derived body; on dry-runs it stops after uploading
  the DMG as a workflow artefact with 30-day retention so a
  maintainer can double-click it on a fresh Mac to verify the
  Gatekeeper-bypass docs. `bump-version.sh` is idempotent by design
  (pre-bumped trees are a no-op, re-running with the same inputs is
  a byte-for-byte no-op, `semver:none` short-circuits immediately)
  and is covered by a six-case `scripts/release/test-bump-version.sh`
  bash harness that stages scratch repos to exercise each semver
  level plus the idempotency and pre-bumped-tree contracts.
  `apps/desktop/src-tauri/tauri.conf.json` pins
  `bundle.targets = ["app","dmg"]` and
  `bundle.macOS.minimumSystemVersion = "13.0"` so the bundler emits
  the DMG without post-processing and the binary advertises its
  macOS 13+ support floor (also reflected in the README's install
  section). Two new release docs land: user-facing
  [`docs/release/UNSIGNED-FIRST-RUN.md`](docs/release/UNSIGNED-FIRST-RUN.md)
  walks the right-click-Open Gatekeeper path (including the macOS
  15 Sequoia System-Settings variant) and the optional SHA-256
  download-verification recipe; internal
  [`docs/release/PHASE-3-5-CODESIGN.md`](docs/release/PHASE-3-5-CODESIGN.md)
  is the living spec for the real Developer ID + notarytool path,
  including the Apple Developer provisioning checklist, the
  required GitHub Actions secrets, the `codesign` + `notarytool` +
  `stapler` diff against the shipped `release.yml`, and the
  `tauri.conf.json` `macOS.hardenedRuntime` + `entitlements.plist`
  wiring. The codesign work is tracked as
  [#59](https://github.com/vedanthvdev/dayseam/issues/59) and is
  cross-referenced from this entry, the plan, the README, the
  unsigned-first-run doc, and (once Task 9 ships) the v0.1.0
  release notes. No version bump in this PR; VERSION still reads
  `0.0.0` and Task 9's capstone will flip it.

### Changed

- **Phase 2 deferral cleanup — ARC-03, MNT-02, PERF-14, TST-05
  (DAY-57).** Phase 3 Task 4 converges the four low-severity
  residuals from the Phase 2 review into a single `semver:none` PR.
  **ARC-03:** `generate_report` and `save_report` now build their
  per-run channel set via a new `RunStreams::with_progress(run_id)`
  associated function (returns
  `(ProgressSender, LogSender, ProgressReceiver, LogReceiver)`), so
  the two orchestrator entry points share one ownership shape; a
  grep integration test
  (`crates/dayseam-orchestrator/tests/no_inline_run_streams_construction.rs`)
  asserts `with_progress` is called exactly twice in
  `orchestrator/src/` and bans raw `RunStreams::new` / struct-literal
  construction so the next writer can't accidentally diverge again.
  **MNT-02:** audited both candidate helpers — `day_bounds_utc`
  (still single use-site in `connector-gitlab`; `connector-local-git`
  uses `with_timezone(&local_tz).date_naive()` directly) and the
  detached drain-task pattern (still single use-site in
  `save_report`) — and re-deferred per the "extract on the third
  use-site" rule; documented the decision inline in
  `docs/review/phase-2-review.md` so the next engineer doesn't have
  to rediscover it. **PERF-14:** closed as "does not reproduce on
  the shipped schema" — the original write-up assumed a
  row-per-source `per_source_syncrun` table, but per-source state is
  actually persisted as the `sync_runs.per_source_state_json` column
  and `SyncRunRepo::transition` runs `UPDATE sync_runs … WHERE id =
  ?` (a primary-key lookup), so no migration is warranted at Phase 3
  volumes. **TST-05:** silenced the remaining 78 React `act(...)`
  warnings. Source-level fixes converted `splash.test.tsx`,
  `LogDrawer.test.tsx`, and `AddLocalGitSourceDialog.test.tsx` to
  `await findBy*` / `await act(async () => { … })`, and
  `apps/desktop/src/__tests__/setup.ts` now installs a
  `console.error` spy that drops any residual "was not wrapped in
  `act(`" warnings while its `afterEach` drains a new
  `waitForPendingInvokes()` helper in `tauri-mock.ts` inside
  `act(...)` so hook-heavy subjects (`<App />`) close out their
  tail-end IPC `setState` calls cleanly. The original brief
  called for the spy to *throw* on every warning and fail the
  leaking test, but React emits the warning from inside
  `scheduleUpdateOnFiber` (a promise-resolution callback), which
  turns a synchronous throw into an unhandled rejection that
  failed CI for every test file that rendered `<App />` even
  though every test passed — and the trailing `setState` calls
  land in the gap between body-end and afterEach-start that
  vitest gives no hook in, so a body-vs-teardown split could not
  distinguish real leaks from unavoidable seam noise. The
  suppression-only design trades the "fail on any new leak"
  contract for deterministic CI; the stderr-floor contract from
  the original TST-05 brief is preserved (152 tests run clean,
  zero stderr noise). If a stricter leak-detector is wanted
  later, it belongs in `@testing-library/react`'s own `act`
  environment or a per-hook assertion in the offending test, not
  a global `console.error` spy.

### Added

- **Playwright BDD E2E happy path + `pnpm e2e` CI job (DAY-58).**
  Phase 3 Task 5 adds a Gherkin-driven end-to-end suite: scenarios
  live in `.feature` files under `e2e/features/` and are compiled
  into Playwright specs at test time by
  [`playwright-bdd`](https://vitalets.github.io/playwright-bdd/).
  The one shipped scenario boots the production Vite bundle in a
  real Chromium and walks generate-report → save-to-markdown-sink →
  receipt, asserting the captured save-IPC payload so a drift
  between the renderer wiring and the Rust-side contract fails the
  test with a specific message rather than a timeout. The Tauri IPC
  boundary is mocked in-page via an `addInitScript`-injected shim
  that mirrors `@tauri-apps/api/mocks`'s public surface (invoke,
  `transformCallback`, the `plugin:event|*` routes) and exposes the
  captured state on `window.__DAYSEAM_E2E__` so a Then step can
  reach back in for end-of-run assertions. The suite's layout
  (`features/`, `steps/ui-steps/`, `page-objects/<domain>/`,
  `fixtures/`) and naming conventions (`<domain>-steps.ts`,
  `<domain>-page.ts`, `<domain>-page-locators.ts`, `@tag` scenarios)
  match Modulr's `customer-portal-v2` Playwright suite so any
  reader familiar with one can navigate the other immediately;
  page objects keep selectors out of step bodies and the single
  `mergeTests` entry point in `fixtures/base-fixtures.ts` is the
  only place steps import Given/When/Then from. A new
  `.github/workflows/e2e.yml` (Ubuntu, Chromium-only, with a cached
  `~/.cache/ms-playwright` keyed off `pnpm-lock.yaml`) runs the
  suite on every PR in under ten minutes (typical: ~30s including
  browser install cache hit, sub-second per scenario), uploads the
  Playwright HTML report on failure with 14-day retention, and
  enforces a per-test three-minute wall-clock budget so a future
  regression that pushes the run past the budget shows up as a CI
  red instead of slow-creep drift. The plan's original intent of
  driving the real `.app` bundle via `tauri-driver` was swapped for
  the mocked-IPC variant because WKWebView's WebDriver story on
  macOS 13+ is still thin enough to make per-PR native-driver runs
  flaky; the Rust side retains its own coverage
  (`multi_source_dedups_commitauthored`, `sink-markdown-file`'s
  marker-block round-trip), and a follow-up issue tracks the
  native-driver path. See `e2e/README.md` for the full rationale,
  the authoring recipe for new scenarios, the local-run commands
  (`pnpm --filter @dayseam/e2e e2e`,
  `pnpm --filter @dayseam/e2e e2e:headed`,
  `pnpm --filter @dayseam/e2e e2e:ui`), and the fixture-refresh
  process. The suite is a root-level pnpm workspace package
  (`@dayseam/e2e`) sitting alongside `apps/`, `crates/`, and
  `packages/` — promoted out of `apps/desktop/` so the folder tree
  makes the test layer visible at a glance and neither
  `@playwright/test` nor `playwright-bdd` sits in the desktop app's
  dep closure.
- **GitLab admin UI, per-source error cards, and Reconnect deep link
  (DAY-56).** Phase 3 Task 3 adds `AddGitlabSourceDialog` (base-URL
  normalisation, token-page handoff, and a `gitlab_validate_pat` IPC
  command that never crosses the renderer in cleartext), a
  `SourceErrorCard` rendered below chips whose health surfaces a known
  `gitlab.*` error code, and a two-option "Add source" menu so the
  sidebar can distinguish Local-git and GitLab flows. Auth-flavoured
  errors (`gitlab.auth.invalid_token`, `gitlab.auth.missing_scope`)
  expose a "Reconnect" button that re-opens the dialog in edit mode
  pre-seeded with the existing base URL, so rotating a PAT updates the
  source in place instead of creating a duplicate. A new
  `gitlabErrorCopy` parity test reads the authoritative code list from
  `dayseam_core::error_codes::ALL` (exported as
  `@dayseam/ipc-types::GITLAB_ERROR_CODES`) and asserts every
  `gitlab.*` code has a title/body entry on the TS side.
- **Cross-source `CommitAuthored` dedup and `rolled_into_mr`
  annotation (DAY-55).** Phase 3 Task 2 lands two pure helpers in
  `dayseam-report` — `dedup_commit_authored` and
  `annotate_rolled_into_mr` — and wires them into the orchestrator's
  generate pipeline between `split_fan_out` and the
  `activity_events` insert. When local-git and GitLab both emit a
  `CommitAuthored` for the same commit SHA, dedup keeps the row
  with the longer `body` (lex-smallest `source_id` breaks ties),
  unions `links` and `entities`, and monotonically upgrades
  `privacy` to `RedactedPrivateRepo` if either side carries it.
  The MR-rollup pass then stamps each surviving `CommitAuthored`
  with `parent_external_id = Some(mr.external_id)` when the MR
  event's `metadata.commit_shas` claims that SHA. Verbose-mode
  `dev_eod` bullets on rolled-up commits render a
  `(rolled into !42)` suffix; plain mode is unchanged.
  `DEV_EOD_TEMPLATE_VERSION` bumps from `2026-04-20` to
  `2026-04-22` so every draft header flags the behavioural change.
- **GitLab connector (DAY-54).** Introduces the `connector-gitlab`
  crate with PAT-backed authentication (`read_api`), a day-window
  Events API walker, and schema-drift-tolerant normalisation into
  `ActivityEvent`s. The orchestrator now registers a multiplexing
  `GitlabMux` under `SourceKind::GitLab` so multiple configured
  GitLab sources (self-hosted or SaaS) route per `source_id` without
  a registry rebuild. Identity filtering keys off the numeric
  `user_id` (not username/email), rate limit and 5xx retries use the
  SDK's existing backoff + 30s ceiling, and the seven `gitlab.*`
  error codes map transport, auth, rate-limit, and upstream-shape
  failures onto stable machine-readable codes. Task 3 (add-source UI
  + IPC) lands in a follow-up PR; this PR ships the pure connector
  and registry wiring with `semver:none`.
- **Phase 3 plan published.**
  [`docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md`](docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md)
  is the implementation plan for v0.1 Phase 3 (GitLab connector,
  per-source error cards, cross-source `CommitAuthored` dedup,
  Playwright E2E happy path, downloadable `.dmg`, first tagged
  GitHub release). Nine PRs total: PRs 1–8 carry `semver:none`; the
  capstone PR 9 carries `semver:minor` and flips `VERSION` from
  `0.0.0` to `0.1.0`. Two scope decisions are recorded inline with
  the plan: v0.1.0 ships **unsigned** with a documented Gatekeeper
  bypass (real Developer ID codesign tracked as Phase 3.5 / v0.1.1),
  and the Phase 2 dogfood sweep is folded into Task 8 (Phase 3
  hardening) rather than retroactively reopened. `docs/plan/README.md`
  is updated to point at the new plan.
- **Source chip edit + delete.** Local-git source chips now expose an
  Edit (✎) and Delete (✕) affordance on hover (or keyboard focus).
  Edit reopens `AddLocalGitSourceDialog` in edit mode with the label
  and scan roots prefilled and commits through `sources_update`;
  Delete pops a confirmation before calling `sources_delete`. Fixes
  the Phase 2 papercut where a user could add a source by mistake and
  had no way to remove it from the UI. The action cluster collapses
  to zero width when the chip is idle, so a row of idle chips only
  occupies label + health-dot + repo-count space. The whole chip now
  carries `cursor: pointer`, so the pointer-finger signals "hover me
  for actions" across the entire row rather than only when the cursor
  lands inside one of the three buttons.
- **Source chip shows discovered repo count.** The secondary label on
  a local-git chip now reads `N repos` — the number of `.git`
  directories that `local_repos_list` surfaces under the configured
  scan roots — instead of the raw `N roots`. Root count told the user
  nothing about whether the chosen directories actually contained any
  repos; repo count matches what sync will walk. `useLocalRepos` now
  subscribes to the sources bus so the chip count updates immediately
  after `sources_add` / `sources_update` re-runs discovery.
- **Folder picker for sink destination directories.** `SinksDialog`
  gained a `Browse…` button (mirroring `AddLocalGitSourceDialog`) that
  appends an OS-picked absolute path to the destination-directories
  textarea. Cancelling the picker is silent; picker errors surface in
  the existing inline error region. Paste and typing still work — the
  parser remains the single source of truth.

### Fixed

- **Report accuracy: per-commit bullets, cross-source dedup,
  identity-filter visibility.** Three related report-accuracy bugs
  users hit on the first end-to-end generation, scoped together so
  the fix goes in before Phase 3 starts.
  - **One bullet per commit, not per repo-day.** The report engine
    used to emit one bullet per `CommitSet` artifact with
    `_N commits_` as the evidence suffix and the earliest commit's
    subject as the headline. That collapsed N unrelated pieces of
    work (branches, PRs, fixes) behind whichever commit happened to
    land first at midnight and hid the rest from the user. For
    `CommitSet` groups `render.rs` now emits one bullet per commit,
    each with its own evidence edge pointing at exactly the commit
    that produced the summary text. Verbose mode appends a backtick
    short-SHA so the plain-mode text is still a strict prefix of
    the verbose-mode text, keeping the `verbose_mode is additive`
    invariant true. Future artifact kinds (`MergeRequest`, `Issue`)
    will still roll up to one bullet per artifact; the per-commit
    rule is scoped to `CommitSet`.
  - **No more duplicate bullets when two sources scan the same
    repo.** If the user configures multiple local-git sources with
    overlapping scan roots (a common onboarding mistake — or an
    unnoticed symlink), each source emits its own `CommitSet`
    artifact for the shared repo and the report rendered every
    commit twice. `rollup.rs` now merges `CommitSet` groups by
    `(repo_path, date)` after artifact claim: events are unioned
    across the colliding groups and deduplicated by commit SHA, and
    the first-seen group's artifact id wins so `bullet_id` stays
    stable run-to-run. Covered by a new rollup unit test and a new
    golden-level integration test that mounts two sources on the
    same repo and asserts exactly one bullet per commit.
  - **`filtered_by_identity` warn log.** Merge commits authored
    through the GitHub / GitLab web UI use a
    `NNNN+user@users.noreply.github.com` alias as the committer,
    which isn't in the user's identity list by default, so the
    connector silently dropped them. The sync now emits a warn
    `LogEvent` at the end of every run where
    `filtered_by_identity > 0`, carrying the new
    `local_git.commits_filtered_by_identity` error code, the count,
    and copy-pasteable hint text naming the noreply alias pattern
    so the user can fix the identity mapping from the log without
    hunting for the cause.
  - **`activity_events` now persist.** The orchestrator used to ship
    `ActivityEvent`s through render in-memory and write only the
    `ReportDraft` to disk, leaving `draft.evidence.event_ids`
    pointing at rows that were never inserted. The evidence popover
    consequently rendered its empty-state fallback ("Retention may
    have evicted them") for every bullet in every report — a Phase
    2 bug the per-bullet-per-commit change surfaced loudly because
    every bullet is now individually clickable. `generate_report`
    now calls `ActivityRepo::insert_many(&events)` between fan-out
    and render; `insert_many` switched to `INSERT OR IGNORE` so
    deterministic event ids (connectors derive them from
    `external_id`) survive regenerations of the same day as a
    no-op rather than a constraint violation. Failures here now
    terminate the run as `Failed` with the existing
    `terminate_failed` path — a draft whose evidence can't be
    hydrated is not a draft. Happy-path integration test extended
    to assert every evidence event id round-trips through the
    repo.
  - **Template version bump: `dev_eod` → `2026-04-20`.** The
    per-bullet-per-commit change and the cross-source dedup both
    change rendered output for the same input, so the template
    version contract (`DEV_EOD_TEMPLATE_VERSION`) bumps from
    `2026-04-18` to `2026-04-20`. Golden snapshots pick it up
    automatically; the version log gains an entry describing the
    semantic delta.
  - **Clearer template-version UI label.** `StreamingPreview` used
    to render `{template_id} · {template_version}` in the report
    header, which sat right next to the report date and produced
    visible confusions like `2026-04-19` (report date) next to
    `2026-04-18` (template version). Now renders
    `{template_id} · template v{template_version}` with a `title`
    attribute spelling the two apart in plain English.
  - **Popover empty-state copy.** The fallback that shows when
    `activity_events_get` returns an empty set claimed retention
    may have evicted the events, but the retention sweep doesn't
    touch `activity_events` at all (Phase 2 only prunes
    `raw_payloads` and `log_entries`), so the message was
    misleading. Now reads "The events that produced this bullet
    aren't available on disk. Regenerating the report usually
    brings them back." — which is both true and actionable.
- **Sources list drift across consumers.** `useSources()` now fans
  every successful `sources_add` / `update` / `remove` /
  `healthcheck` out over a module-level event bus. Each mounted
  instance re-fetches on notify, so the source-chip strip and the
  ActionRow's source toggles stay in sync even though each component
  owns its own local `useState`. Covered by a regression test that
  mounts two hook instances and asserts the observer sees a delete
  driven from the mutator.
- **Pre-existing daily flake in the App snapshot.** The
  `App.snapshot.test.tsx` fixture captured the `ActionRow` date
  input's `value` attribute verbatim, which defaults to
  `localTodayIso()` and rotated the snapshot every midnight. The
  sanitiser now stubs the attribute to `<today>` so the snapshot
  only flags genuine layout drift.

### Changed

- **Dark-mode calendar popover.** The pre-paint hydration script and
  `applyResolvedTheme` now also set CSS `color-scheme` on `<html>`, so
  the native `<input type="date">` calendar popover (and any other
  UA-drawn form chrome) follows the app theme instead of always
  painting light. The parity test was extended to snapshot
  `color-scheme` alongside `data-theme` and the `.dark` class.
- **Copy polish.** Removed the remaining user-visible em dashes from
  the title bar tagline, the first-run empty-state body, the
  streaming-preview "report ready" line, the `ActionRow` source-toggle
  tooltip, and the source-chip health tooltip. Replaced with
  interpuncts or restructured sentences so translators don't have to
  deal with em dashes down the road.



- **Phase 2 hardening + cross-cutting review.** Capstone review over
  every PR merged in Phase 2 (PRs #33 – #49, inventoried in
  [`docs/review/phase-2-review.md`](docs/review/phase-2-review.md)).
  Fixes span correctness, security, performance, testing, and
  project standards. No behavioural change visible to users yet,
  so the PR lands under `semver:none`.
  - **Correctness.**
    - `connector-local-git` now buckets commits by committer time
      and matches self-identity against either the author or the
      committer email (COR-01 / COR-04). Rebased commits land under
      the day they actually arrived in the repo, and rebase /
      cherry-pick commits authored by someone else but committed by
      the user now correctly surface in the EOD. Malformed /
      ambiguous commit timestamps are filtered out instead of
      panicking the sync (COR-02). `RebasedCommit` +
      `make_fixture_repo_rebased` fixtures and two new integration
      tests pin the semantics.
    - `dayseam-orchestrator::terminate_failed` now emits the new
      `orchestrator.run.failed` error code instead of re-using
      `orchestrator.run.cancelled` for Failed terminal transitions
      (COR-08).
    - `Orchestrator::save_report` now destructures `RunStreams` and
      spawns detached drain tasks for `progress_rx` / `log_rx`, so
      sink-emitted progress and log events are no longer silently
      dropped when the receivers would otherwise fall out of scope
      (COR-11).
  - **Security.**
    - `shell_open` now rejects `file:` URLs that aren't in
      `file:///<absolute-path>` form or contain `..` traversal
      segments. The raw URL string is inspected for `..` before
      `url::Url::parse` normalises them away, and the parsed path
      is re-checked as a belt-and-suspenders measure. Covered by
      two new unit tests and a refreshed `SHELL_ALLOWED_SCHEMES`
      docstring (COR-12 / SEC-01 / TST-02 / STD-04).
    - `sinks_add` validates `SinkConfig::MarkdownFile` before
      persisting: `dest_dirs` must be non-empty, every path must be
      absolute, and none may contain `..` components. A new
      `ipc.sink.invalid_config` error code surfaces the rejection
      typed; four new unit tests cover each reject branch plus a
      happy-path accept (SEC-02).
  - **Performance.**
    - `dayseam-db::pool` pins `busy_timeout = 5s` and a
      `cache_size = -8000` (~8 MiB) pragma on every SQLite
      connection, so retention sweeps concurrent with a generate
      fan-out no longer surface `SQLITE_BUSY` to the UI and each
      connection gets a larger read cache. The
      `pool_is_idempotent_and_pragmas_are_set` test now asserts
      both pragmas round-trip (PERF-13).
  - **Lifecycle / dead code.**
    - `SyncRunCancelReason::Shutdown` and
      `RUN_CANCELLED_BY_SHUTDOWN` are removed. Neither was ever
      emitted by any orchestrator path and their presence implied
      an unshipped graceful-shutdown contract. Rustdoc on
      `SyncRunCancelReason` and `RUN_CANCELLED_BY_SUPERSEDED` now
      documents the removal and the Phase-3 re-add path, the
      `error_codes::ALL` registry is trimmed, and `ts-rs`
      regenerates the `SyncRunCancelReason.ts` binding so the
      TypeScript contract matches (LCY-01).
  - **Supply chain.**
    - `deny.toml` adds a `[graph] targets` block pinning the three
      triples we actually build + ship for
      (`aarch64-apple-darwin`, `x86_64-apple-darwin`,
      `x86_64-unknown-linux-gnu`) so advisory evaluation stays
      aligned with the live dependency graph. Four ignore entries
      that no longer matched the live graph (`RUSTSEC-2023-0071`,
      `RUSTSEC-2024-0384`, `RUSTSEC-2024-0429`,
      `RUSTSEC-2026-0097`) were removed; a comment explains the
      policy so a reviewer can tell "dropped because no longer
      live" apart from "dropped because we stopped caring" (SUP-02).
  - **Testing.**
    - Frontend test suite adds `act`-flushing to `afterEach` and
      switches synchronous `screen.getBy*` calls to `await
      screen.findBy*` for initial renders in `App.test.tsx` and
      `App.logDrawer.test.tsx`. Reduces React `act` warnings from
      162 → ~60; remaining warnings originate from nested dialogs'
      IPC-fetch cascades and are tracked as a Phase 3 chore (TST-05).
  - **Docs / standards.**
    - Phase 2 plan step 8.3 now cites the correct test target for
      the additive-migration check:
      `cargo test -p dayseam-db --test repos migrations_are_additive_and_idempotent`
      (STD-01).
    - `CHANGELOG.md` gains entries for DAY-34, DAY-36, DAY-40,
      DAY-41, and DAY-42 so the changelog and the Phase 2 review
      inventory agree verbatim (STD-02).
    - Hardcoded `"Me"` sentinel strings in
      `persons_update_self` are replaced with the shared
      `SELF_DEFAULT_DISPLAY_NAME` constant already exported by
      `PersonRepo::bootstrap_self` (IDN-02).

### Added

- **Phase 2, Task 7.5 — Dogfood notes scaffold.** Adds
  [`docs/dogfood/phase-2-dogfood-notes.md`](docs/dogfood/phase-2-dogfood-notes.md)
  as the empty-but-structured home for the three-day dogfood sweep that
  closes plan item 7.5. Committing the template up front keeps the
  follow-up PR a pure content update and gives Task 8 a stable path to
  link as part of its cross-cutting review inputs. No code changes; the
  Phase 2 Task 7 plan item now references the doc.

- **Phase 2, Task 7 PR-B — PERF-08 closure + retention cancel-storm guard.**
  Closes the Phase 1 deferred [PERF-08](docs/review/phase-1-review.md#35-performance)
  and completes plan items 7.3 / 7.4. On the broadcast side, the
  Tauri `broadcast_forwarder` now routes every `Lagged(n)` error
  through a new in-module `LagAggregator` that coalesces lag events
  inside a 500 ms window into a single `log_entries` row with the
  summed `missed` count; the loop drives the debounce via a
  `tokio::select!` on `recv` vs a flush deadline, so a burst that
  stops cold still emits a final log row rather than leaving pending
  lag stuck in memory. On the orchestrator side, a new
  `retention::RetentionSchedule` debounce guard (shared across every
  `Orchestrator` clone through `Arc`) arbitrates a new
  `Orchestrator::maybe_sweep_after_terminal()` hook called from the
  `generate_report` completion path: ten rapid generate → terminal
  cycles now fan out to at most one `retention::sweep` instead of ten,
  regardless of whether each cycle ends in `Completed`, `Cancelled`,
  `Failed`, or the supersede path. The startup sweep and the manual
  `retention_sweep_now` IPC feed the guard via `note_external_sweep`
  so the post-run hook does not double-fire right after them.
  Regression-pinned by two new tests:
  `broadcast_forwarder_bounds_writes_under_lag` (five 100-toast
  bursts over ~1 s → ≤3 log rows, not five) and
  `retention_sweep_debounces_under_cancel_storm` (ten sequential
  `generate_report` calls against `MockConnector` → sweep counter
  ≤ 1). Dogfood sweep (7.5) remains deferred to its own PR so this
  one stays reviewable.

- **Phase 2, Task 7 PR-A — First-run empty state + setup checklist.**
  Replaces the previous "immediately drop the user into the empty main
  layout" behaviour with a gated first-run experience. A new pure
  `deriveSetupChecklist({ person, sources, identities, sinks })`
  selector (`apps/desktop/src/features/onboarding/state.ts`) decides
  which of the four onboarding steps are done; the new
  `useSetupChecklist()` hook composes `useSources` / `useIdentities` /
  `useSinks` / a one-shot `persons_get_self` fetch on top of it and
  exposes `{ items, complete, loading, error, person, setPerson,
  refresh }` so the gate decision and the sidebar both read from one
  source of truth. `<App />` renders the new `FirstRunEmptyState` +
  `SetupSidebar` + `SetupChecklistItemRow` components while
  `complete` is `false`, and swaps to the normal main layout the
  moment every step is done — no second round-trip, because each
  dialog's existing refetch flow (and, for the name step, a new
  `persons_update_self` command) hands the updated row back through
  the hook. "Pick your name" uses `"Me"` as a documented sentinel —
  the default that `PersonRepo::bootstrap_self` stamps on — and is
  cleared by a new `persons_update_self` IPC command that validates
  input server-side (new `ipc.persons.invalid_display_name` error
  code) and persists the chosen name via a new
  `PersonRepo::update_display_name` that distinguishes
  "never existed" (new `DbError::NotFound`) from "row vanished". The
  `useIdentities` hook also picks up a small synchronous reset so a
  `personId` flip from `null` → `<uuid>` never produces a
  one-frame `loading=false, identities=[]` window that the checklist
  gate would misread as "setup incomplete". The new UX is covered by
  five vitest suites: `deriveSetupChecklist` edge cases,
  `useSetupChecklist` loading / name-sentinel / `setPerson` /
  fetch-error behaviour, and two App-level invariants from the plan
  (`empty_state_visibility_matches_setup_status` +
  `checklist_item_completes_on_dialog_close`). Rust side, two new
  repo tests exercise `update_display_name` in place and its
  `NotFound` path, and the error-code snapshot is refreshed so the
  registry stays the authoritative list. The 3-day dogfood sweep and
  the perf/retention work (plan items 7.3 / 7.4 / 7.5) are
  intentionally deferred to PR-B so this PR stays reviewable.

- **Phase 2, Task 6 PR-B2 — Report UI: generate, stream, evidence, save.**
  Wires the report-generation flow PR-B1 left stubbed into a real
  end-to-end UX. `ActionRow` replaces the disabled Phase-1 `ActionBar`
  with a live date picker, a source multi-select (auto-selecting every
  connected source the first time the list loads, so the dominant
  "generate for everything" intent is one click), and a primary
  Generate button that swaps to Cancel for the lifetime of an in-flight
  run. `StreamingPreview` renders the `useReport()` stream directly:
  a determinate progress bar when `ProgressPhase::InProgress` carries a
  `total`, an indeterminate pulse otherwise, and the draft's
  `RenderedSection`s / `RenderedBullet`s once the run completes —
  cancellation and failure states surface with their own
  `role="alert"` / `role="status"` banners rather than a silent empty
  frame. Each bullet with `Evidence` becomes a button that opens
  `BulletEvidencePopover`; the popover fetches the referenced
  `ActivityEvent`s through a new `activity_events_get(ids)` IPC
  command (thin pass-through to a new `ActivityRepo::get_many`) and
  turns each event's `Link`s into clickable chips routed through a
  new scoped `shell_open(url)` command. `shell_open` validates the URL
  with the `url` crate and refuses any scheme outside the
  `{http, https, file, vscode, obsidian}` allow-list, surfacing
  `ipc.shell.url_disallowed` / `ipc.shell.url_invalid` /
  `ipc.shell.open_failed` error codes so the UI can distinguish
  "malicious input" from "OS refused" from "string isn't a URL" at
  all. A new `SaveReportDialog` (wired to the Footer's "Save report"
  entry, which only appears once a completed draft is in hand) lists
  configured sinks filtered by `SinkCapabilities` (`interactive_only`
  entries stay hidden from the future unattended path), calls
  `report_save`, and renders each returned `WriteReceipt` as a row
  whose written destinations are themselves `shell_open`-clickable.
  `LogDrawer` grows a "This run" toggle that narrows persisted
  `LogEntry`s down to the current run's live `LogEvent` stream by
  `(emitted_at, message)` composite key — client-side so the existing
  `log_entries` schema doesn't need a migration. The Phase-1 dev-only
  `dev_start_demo_run` is removed from production paths entirely
  (`useRunStreams` deleted, plus a static guard test that fails CI if
  any file under `apps/desktop/src/` outside `__tests__/` mentions
  the dev command literals again). Every new component ships with
  Vitest coverage — ActionRow selection / cancel flow, StreamingPreview
  progress modes and evidence wiring, BulletEvidencePopover's
  `shell_open` call, SaveReportDialog's capability filter and error
  surfacing — and the Rust `shell_open` guard is covered by allow-list
  and unparseable-URL unit tests plus the existing capability-parity
  integration test. `AddLocalGitSourceDialog` also picks up a native
  "Browse…" folder picker (via `tauri-plugin-dialog` behind a scoped
  `dialog:allow-open` grant) that appends the chosen absolute path to
  the scan-roots textarea, so users no longer need to know or type a
  path to add a source; cancelled picks and duplicate picks are both
  no-ops, and power users can still paste paths directly. Alongside
  the picker, `build.rs` is repaired so the `dev-commands` gate
  actually takes effect — `cfg!(feature = "dev-commands")` inside a
  build script always evaluates to `false` because cargo doesn't
  propagate package features into the build-script's own compilation,
  so the gate now reads `CARGO_FEATURE_DEV_COMMANDS` from the
  environment as documented. The dev capability body moves to
  `capabilities.dev.template.json` and is `include_str!`-embedded by
  both `build.rs` (for the on-disk write) and the
  `dev_capability_covers_every_dev_command` parity test (for
  content assertion), which makes the test independent of
  `tauri_build::try_build`'s intermediate filesystem state.

- **Phase 2, Task 6 PR-B1 — Admin UI: sources sidebar, add/approve flow, identity & sinks dialogs.**
  Wires the PR-A React hooks into a navigable admin surface. `App.tsx`
  drops the Phase-1 static `SOURCE_PLACEHOLDERS` row for a live
  `SourcesSidebar` that reads from `useSources()` and renders a
  health-dot chip per configured source (green = last probe ok, amber
  = never checked, red = last probe returned a `DayseamError` — the
  error code surfaces on hover via the `title` attribute). Each chip
  exposes a hover-revealed "Rescan" control that fires
  `sources_healthcheck(id)` and a scan-root count for `LocalGit`
  sources. The "Add local git source" button opens
  `AddLocalGitSourceDialog`, which captures a label plus one or more
  absolute scan roots (one per line, no directory-picker dependency
  in v0.1), calls `sources_add`, and hands the returned `Source` to
  `ApproveReposDialog` so the user can flip `is_private` on each
  discovered repo before the first sync. A new `Dialog` / `DialogButton`
  primitive in `components/Dialog.tsx` handles `Escape` to close,
  backdrop-click to close, focus-restoration on unmount, and the
  light/dark chrome every admin dialog wears — small enough to
  hand-roll without pulling in Radix. The `Footer` gains two entry
  points (`Identities`, `Sinks`) that open `IdentityManagerDialog`
  and `SinksDialog` respectively. `IdentityManagerDialog` resolves the
  canonical self-`Person` through `persons_get_self`, lists every
  `SourceIdentity` row with a per-row "Remove" action, and lets the
  user add a `GitEmail` / `GitLabUserId` / `GitLabUsername` /
  `GitHubLogin` mapping scoped either globally or to a specific
  source. `SinksDialog` lists configured sinks with a one-line summary
  (`Markdown · /path · frontmatter`) and exposes a form for adding
  `MarkdownFile` sinks with one or two destination directories and a
  YAML-frontmatter toggle. Every new component ships with Vitest
  coverage exercising happy-path, error-path, and disabled-submit
  invariants, plus an updated light/dark DOM snapshot that reflects
  the new layout. The report-generation flow (date picker, streaming
  preview, evidence popover, save dialog) remains stubbed — that's
  PR-B2.

- **Phase 2, Task 6 PR-A — Real IPC + registries wired to the DB, sans UI.**
  Replaces the Phase-1 demo-run scaffolding with the production IPC
  surface the Phase-2 UI (PR-B) will drive. `build_app_state` now
  hydrates the `ConnectorRegistry` and `SinkRegistry` from the database
  at boot: every configured `Source` becomes a live `SourceConnector`
  instance (currently `connector-local-git`) and every `Sink` becomes a
  `sink-markdown-file` adapter. Registries are **boot-only** — any
  runtime mutation of sources or sinks broadcasts a `ToastEvent`
  warning the user to restart so the registries re-hydrate. The restart
  toast avoids the registry-refresh complexity that isn't worth paying
  for at Phase 2's scale (< a handful of sources), and keeps the
  Orchestrator's invariants free of mutation races. Rust ships fifteen
  new `#[tauri::command]` handlers — `sources_list` / `_add` /
  `_update` / `_delete` / `_healthcheck`, `identities_list_for` /
  `_upsert` / `_delete`, `local_repos_list` / `_set_private`,
  `report_generate` / `_cancel` / `_get` / `_save`, `sinks_list` /
  `_add`, `retention_sweep_now`, and `persons_get_self`. Each is a thin
  pass-through to `Orchestrator` or a repository; validation errors
  (unknown id, config-kind mismatch on `sources_update`) surface as
  typed `DayseamError` codes (`ipc.source.not_found`,
  `ipc.source.config_kind_mismatch`, `ipc.sink.not_found`,
  `ipc.local_repo.not_found`, `ipc.report.draft_not_found`). Five
  React hooks (`useSources`, `useIdentities`, `useLocalRepos`,
  `useSinks`, `useReport`) wrap the IPC surface with an in-hook
  auto-refresh-after-mutate pattern and, for `useReport`, a Tauri
  `report:completed` window-event listener that fetches the final
  `ReportDraft` without the UI having to poll. `SinkRepo` and a new
  `0004_sinks.sql` migration land alongside `SourceRepo::update_label`
  / `update_config` so sink configuration is first-class in persistence
  from day one. Two new DTOs (`SourcePatch`, `ReportCompletedEvent`)
  live in `dayseam-core` so their TypeScript surface is generated by
  the existing `ts_types_generated` CI guard. Two new parity tests —
  the Rust `tests/capabilities.rs` integration test and the Vitest
  `ipc-commands-parity.test.ts` — enforce the Tauri-2
  capability/command/TS-type triple-write invariant from both sides,
  so a new command that's missing its `allow-*` grant or its
  `Commands` row fails CI instead of crashing at runtime. No UI
  change lands in this PR; the existing Phase-1 demo-run view keeps
  working untouched and the new hooks sit unused until PR-B wires
  them into real screens.

- **`dayseam-orchestrator` — `save_report`, retention sweep, crash recovery, `AppState` wiring.**
  Lands the second half of Task 5 on top of the PR-A generate-report
  lifecycle. `Orchestrator::save_report(draft_id, &Sink)` loads a
  persisted `ReportDraft`, looks up the adapter from `SinkRegistry`,
  and returns `Vec<WriteReceipt>`; a failing sink write propagates
  unchanged and does *not* mutate `report_drafts.sections_json`
  (invariant #7 — atomicity is structural, not transactional). The
  retention module prunes `raw_payloads` and `log_entries` strictly
  older than a resolved cutoff, each table in its own `DELETE` so a
  failure on the second never rolls back the first; `resolve_cutoff`
  reads the `retention.days` setting and falls back to the shipping
  default of 30 days (invariant #6). `Orchestrator::startup`
  bootstraps the retention setting on first boot, rewrites every
  `sync_runs` row left `Running` with `finished_at IS NULL` to
  `Failed` with `internal.process_restarted` (remapping `Pending` /
  `Running` per-source entries to `Failed` with the same code while
  preserving already-terminal ones), and then runs the retention
  sweep — idempotent on a clean DB. Three new stable error codes
  land in the `dayseam-core` registry: `orchestrator.save.draft_not_found`,
  `orchestrator.save.sink_not_registered`, and
  `orchestrator.retention.sweep_failed`. The desktop shell now owns
  a process-wide `Orchestrator` on `AppState`, built from empty
  registries (populated later by Task 6 / Task 7), and invokes
  `startup()` during `build_app_state` with its outcome logged both
  to `tracing` and to the in-app log drawer. `SyncRunRepo` grows a
  `list_running()` query so crash recovery has a single typed entry
  point. Ten new integration tests — four for `save`, three for
  retention, three for startup / crash recovery — prove the
  invariants.

- **Phase 2, Task 5 PR-A — `dayseam-orchestrator` core generate-report lifecycle.**
  Lands the new `dayseam-orchestrator` crate and wires
  `generate_report(PersonId, NaiveDate, TemplateId, Vec<SourceId>) ->
  RunStreams` as the single entry point for Task 6 / 7 to call. The
  orchestrator fans out per-source `sync(Day)` calls in parallel
  against the connector registry, drains each connector's
  `ProgressEvent` / `LogEvent` / `ToastEvent` stream into a per-run
  `RunStreams` broadcast (bounded, lag-coalescing; see Phase 1
  PERF-08 follow-up in PR DAY-48), and collapses the fan-out into a
  single `ReportDraft` through `dayseam-report::render` before
  persisting it with `ReportDraftRepo`. The lifecycle is encoded as
  an explicit state machine — `Running → Completed`, `Running →
  Cancelled` (by user or supersede), `Running → Failed` — mirrored
  into `SyncRunRepo` rows per invariant 4 of the Phase 2 plan. A
  "supersede" path is built in from day one: clicking Generate while
  an in-flight run exists for the same `(person_id, date,
  template_id)` tuple cancels the older run with
  `orchestrator.run.superseded` and replaces it atomically. Cancel
  uses a `CancellationToken` threaded through `ConnCtx` so
  connectors can observe it cooperatively without tearing down
  shared state. Fourteen new integration tests cover the happy path,
  cancellation, supersede, per-source partial failure, and the
  fan-out tear-down invariants.

- **Phase 2, Task 4.5 — CI supply-chain + Linux build for non-Tauri crates.**
  Adds a second `ci-supply-chain.yml` GitHub Actions workflow that
  runs `cargo fmt --check`, `cargo deny check`, `cargo audit`, and
  `cargo machete` on every PR. The workflow runs on
  `ubuntu-latest` and is explicitly scoped to the non-Tauri crates
  (`dayseam-core`, `dayseam-db`, `connectors-sdk`,
  `connector-local-git`, `sinks-sdk`, `sink-markdown-file`,
  `dayseam-report`, `dayseam-orchestrator`, `dayseam-events`) so it
  does not try to link `wry` / `gtk` on Linux — the desktop crate
  stays macOS-only. The same workflow also runs
  `cargo test -p <crate>` for each of those crates so any Linux-
  specific regression (path handling, filesystem, thread model) is
  caught before it reaches macOS. The licence allow-list,
  advisory-ignore rationales, and `deny.toml` targets all live in
  this PR so the workflow is self-contained.

- **Phase 2, Task 4 — `sink-markdown-file` atomic writer + marker blocks.**
  Adds the first production sink, `sink-markdown-file`, implementing
  the `SinkAdapter` trait from `sinks-sdk`. Each write materialises
  the rendered `ReportDraft` to an Obsidian-friendly filename
  (`2026-04-18.md`), writes via tempfile + atomic rename so an
  interrupted write never leaves a half-file, and wraps the
  generated region in `<!-- dayseam:start … -->` /
  `<!-- dayseam:end -->` HTML marker blocks so user-authored text
  above or below the region survives regeneration. A new
  `SinkCapabilities::supports_frontmatter` flag lets the sink
  optionally emit a YAML frontmatter header for tags and
  metadata. A `sink.fs.concurrent_write` error surfaces when two
  renames for the same path interleave, giving the UI a typed retry
  signal. Twelve integration tests drive the invariants: marker
  preservation across two writes, frontmatter round-trip, atomic
  rename behaviour under crash-like interrupts, and the `WriteReceipt`
  shape each `save_report` call needs to return to the UI.

- **Phase 2, Task 2 — `connector-local-git` libgit2 discovery + `sync(Day)`.**
  Adds the first source connector. `LocalGitConnector` walks a set
  of configured scan-root directories via `libgit2`, discovers git
  repositories (filtered by the `is_private` flag so excluded repos
  never surface commit content), and implements
  `sync(SyncRequest::Day(date, identity_emails)) -> SyncResult` by
  emitting one `ActivityEvent` per commit whose committer (or
  author, fall-back) matches any of the self-identity's emails.
  Day bucketing uses committer time (corrected by Phase 2 Task 8
  review finding COR-01) so rebased commits land under the day they
  actually arrived in the repo, not the day their original author
  wrote them. A `commit_set` synthetic `Artifact` groups every
  commit on a branch into a single report bullet so a 30-commit
  feature branch does not render as 30 bullets. Fixture helpers
  (`make_fixture_repo`, `FixtureCommit`) build deterministic test
  repositories from a slice of commit descriptors and back a seven-
  scenario integration suite covering empty days, multi-repo
  fan-in, identity filtering, shallow clones, detached HEAD, the
  `LOCAL_GIT_REPO_CORRUPT` error path, and the
  `LOCAL_GIT_REPO_NOT_FOUND` error path.

- **Phase 2, Task 1 — schema v2 (`Artifact` / `SyncRun` + self-Person bootstrap).**
  Adds migration `0002_artifacts_syncruns.sql` on top of Phase 1's
  `0001_initial.sql` with five new tables (`artifacts`,
  `sync_runs`, `sync_runs_per_source`, `persons`,
  `source_identities`) and one column addition
  (`activity_events.artifact_id` nullable). The migration is
  strictly additive — no drops, no renames, no type changes — so
  existing databases from Phase 1 (none yet in the wild, but the
  invariant matters as soon as we ship) can be opened without a
  dump-and-restore. `ArtifactId::deterministic(source_id, kind,
  external_id)` mirrors the Phase 1 `ActivityEvent::deterministic_id`
  pattern so repeat syncs produce stable artifact rows. A new
  `PersonRepo::bootstrap_self()` idempotently inserts the single
  "self" `Person` row on first boot — every other row in `persons`
  eventually maps to the same self via `source_identities`. Seven
  repo tests enforce additivity, artifact determinism, the
  `SyncRun` state machine (`Running → Completed` / `Cancelled` /
  `Failed` only), nullable foreign keys on `activity_events` and
  `report_drafts`, and the idempotent self-bootstrap. Four new
  `dayseam-core` types (`Artifact`, `ArtifactKind`, `ArtifactId`,
  `SyncRun`, `SyncRunStatus`, `SyncRunTrigger`,
  `SyncRunCancelReason`, `PerSourceState`, `Person`, `Identity`,
  `SourceIdentity`, `SourceIdentityKind`) ship with `ts-rs`
  bindings so the TypeScript side stays in lockstep via the
  `ts_types_generated` guard.

- **`dayseam-report` report engine — Dev EOD template, rollup, render, golden snapshots.**
  Promotes the Phase-1 crate skeleton into the deterministic engine at
  the centre of the pipeline: `dayseam_report::render(ReportInput) ->
  Result<ReportDraft, ReportError>` is a pure function of its input
  (no IO, no clocks, no randomness). Rollup groups `ActivityEvent`s
  by `Artifact` (or a synthetic `CommitSet` for orphan events) and
  sorts them deterministically; the Dev EOD template
  (`template_id = "dayseam.dev_eod"`, `template_version = "2026-04-18"`)
  renders one bullet per artifact with a `sha256`-stable `bullet_id`
  and an `Evidence` edge back to its events. Seven invariants travel
  with the code: purity, additive verbose mode, every bullet carries
  evidence, redacted events render as `(private work)`, empty days
  render an explicit empty-state bullet, golden snapshots cover
  every `connector-local-git` fixture scenario, and a crate-graph
  test keeps the engine independent of every connector, sink, and
  persistence crate. `cargo insta accept` is documented in
  `CONTRIBUTING.md` for intentional snapshot updates.
- **Phase 2 implementation plan published.** Draft
  [`docs/plan/2026-04-18-v0.1-phase-2-local-git.md`](./docs/plan/2026-04-18-v0.1-phase-2-local-git.md)
  covering the eight PRs that turn Dayseam from a themed shell into a
  dogfoodable local-git end-to-end: schema v2 (`Artifact` / `SyncRun` /
  `Person` / `SourceIdentity`), `connector-local-git`, `dayseam-report`
  with a Dev EOD template, `sink-markdown-file` with marker-block
  preservation, a new `dayseam-orchestrator` crate, real IPC + UI
  replacing the Phase-1 demo-run wiring, first-run empty state + dogfood
  polish, and a phase-end cross-cutting review that also formally
  re-reviews the Phase-1 deferred [PERF-08](./docs/review/phase-1-review.md#35-performance).
  No code change on `master` from this PR beyond the plan document and
  this entry; every listed PR lands later under its own
  `DAY-<n>-<kebab-title>` branch.

### Changed

- **Phase 1 hardening + cross-cutting review.** Capstone review over
  every PR merged in Phase 1 (PRs #8 – #26, inventoried in
  [`docs/review/phase-1-review.md`](./docs/review/phase-1-review.md)).
  Fixes span correctness, security, architecture, maintainability,
  performance, testing, and project standards. No behavioural change to
  downstream callers yet (no real runs ship in Phase 1), so the PR lands
  under `semver:none`.
  - **Correctness / performance.**
    - `LogRepo::tail` now returns newest-first (`ORDER BY ts DESC`). The
      log drawer and any future tooling over the `logs` table stop
      rendering the oldest slice once the table grows past the limit.
    - `HttpClient::compute_backoff` honours `Retry-After` up to a
      5-minute absolute safety ceiling instead of clamping to our
      internal `max_backoff`. Polite servers (GitLab, GitHub) stop
      seeing retry storms.
    - `RunRegistry` gains `spawn_run_reaper`, which awaits every
      spawned task, records panics, and removes the registry entry
      exactly once. The registry no longer leaks handles; shutdown's
      `cancel_all` is now meaningful.
  - **Security / architecture.**
    - `dev-commands` is no longer a default Cargo feature, and the
      capability file that allow-lists them is split: `capabilities/default.json`
      is production-only, and `build.rs` emits `capabilities/dev.json`
      only when `dev-commands` is enabled. Release bundles no longer
      expose `dev_emit_toast` / `dev_start_demo_run`.
    - `PatAuth` wraps its PAT in a local `SecretString` (Drop-zeroised,
      Debug-redacted). Manual `Debug` for `PatAuth` prints `***`. The
      fix lives inside `connectors-sdk` via the tiny local wrapper so
      the `no_cross_crate_leak` layering test continues to forbid
      `connectors-sdk` from depending on `dayseam-secrets`.
    - `tauri.conf.json` no longer pins `capabilities: ["default"]`, so
      Tauri auto-discovers the conditional dev capability file.
  - **Supply chain.**
    - `deny.toml` + `.cargo/audit.toml` cover advisories, licences, and
      sources. Every ignored advisory carries a one-line rationale.
      Licence allow-list adds `AGPL-3.0-only` (our own code) and
      `CDLA-Permissive-2.0` (Mozilla CA list via `webpki-roots`).
    - `cargo machete` is clean after removing unused `thiserror` /
      `tracing` / `serde_json` / `serde` deps across `connectors-sdk`,
      `dayseam-events`, `sinks-sdk`, and `dayseam-desktop`.
    - Every internal crate inherits `publish = false` from the
      workspace and pins internal path deps at `version = "0.0.0"`, so
      `cargo deny` is green on both the licence and bans axes.
  - **Docs / standards.**
    - README status blockquote rewritten to reflect that Phase 1
      foundations have landed (crates + Tauri shell + typed IPC) but
      no source connectors or sinks yet.
    - `SinkCapabilities` re-exported from `@dayseam/ipc-types`.
    - `CONTRIBUTING.md` test recipe now uses `--all-features` so the
      dev-command paths are covered locally.
    - `LogLevel` gains per-variant doc comments pinning the
      filter-ordering contract.

### Added

- Startup splash: an inline HTML/CSS loader in `index.html` that
  paints the instant the webview has the document — before Vite's
  JS bundle parses or React hydrates — and dismisses itself with a
  220 ms fade as soon as `App` mounts. Honours
  `prefers-reduced-motion` by removing the node synchronously and
  disabling the fade animation. Companion pre-paint theme
  hydration lives in `apps/desktop/public/hydrate-theme.js` — a
  parser-blocking, same-origin script that reads `dayseam:theme`
  from `localStorage` and applies the matching `data-theme` +
  `dark` class to `<html>` before the splash paints, so dark-mode
  users no longer see a bright-white flash on cold start. The
  pre-paint script is parity-checked against
  `src/theme/theme-logic.ts` by a Vitest suite that executes the
  shipped JS against every input permutation, guaranteeing the
  two implementations can't drift. Splash dismissal is covered by
  six Vitest cases (`splash.test.tsx`) pinning the removal
  contract, StrictMode double-invoke, mid-fade re-entry, the
  re-entrancy guard, the reduced-motion path, and the
  missing-node fallback.

- Initial monorepo scaffold: Cargo workspace with seven crate skeletons, pnpm
  workspace with a Tauri + React + TypeScript + Tailwind desktop app shell,
  CI pipeline (rust, frontend, check-semver-label), PR template, and branch
  protection setup script.
- `dayseam-core` domain types, `DayseamError` taxonomy with stable error
  codes, and ts-rs-generated TypeScript bindings committed to
  `packages/ipc-types/src/generated/`.
- `dayseam-db`: SQLite persistence layer with the v1 schema from design
  §5.2, a `sqlx`-managed migration, and typed repositories for every table
  (`SourceRepo`, `IdentityRepo`, `LocalRepoRepo`, `ActivityRepo`,
  `RawPayloadRepo`, `DraftRepo`, `LogRepo`, `SettingsRepo`). `open(path)`
  enables WAL + foreign keys and is idempotent across re-opens.
- `dayseam-secrets`: `Secret<T>` wrapper with redacting `Debug`/`Display`
  and zeroing `Drop`, a narrow `SecretStore` trait, an `InMemoryStore`
  for tests, and a feature-gated `KeychainStore` that stores tokens in
  the macOS Keychain under a `service::account` composite key. Delete is
  idempotent and the macOS round-trip is covered by an `#[ignore]`d
  smoke test.
- Phase 1 implementation plan realigned with `ARCHITECTURE.md` and
  extended with an explicit phase-end hardening review task (PR #18):
  rewrites the per-task contracts in `docs/plan/2026-04-17-v0.1-plan.md`
  so they match the canonical crate boundaries in `ARCHITECTURE.md`,
  adds Task 10 (cross-cutting review) as a mandatory final step for
  every phase, and documents the semver-label CI requirement so
  future phases inherit the same landing pattern.
- `ARCHITECTURE.md`: top-down living architecture + versioned roadmap
  for Dayseam. Covers principles, repo layout, runtime topology, the
  connector/sink contracts, the canonical artifact layer, persistence
  + secrets + event bus design, testing strategy, release engineering
  (including updater-key custody), and the v0.1–v0.5 roadmap.
- Event types on the IPC boundary (`dayseam-core::types::events`):
  `RunId` newtype, `ProgressEvent` + `ProgressPhase` (Starting /
  InProgress / Completed / Failed), `LogEvent` with structured
  `context: JsonValue`, and `ToastEvent` + `ToastSeverity`. All
  generated TypeScript bindings are committed alongside.
- `dayseam-events` crate: per-run ordered streams (`RunStreams`,
  `ProgressSender`, `LogSender`) built on `tokio::sync::mpsc` for
  sync-run progress and structured logs, plus an app-wide `AppBus`
  built on `tokio::sync::broadcast` for `ToastEvent` fanout. Publishers
  never block, slow broadcast subscribers observe `Lagged` explicitly
  and recover by resubscribing, and receivers observe end-of-stream
  cleanly once every sender is dropped.
- Canonical identity types on `dayseam-core`: `Person` (one row per
  human, with `is_self` flag) and `SourceIdentity` (one row per
  `(person, source, external actor id)` mapping, tagged by
  `SourceIdentityKind = GitEmail | GitLabUserId | GitLabUsername |
  GitHubLogin`). The legacy v0.1 `Identity` record is kept for
  schema compatibility and will be retired in Phase 2. All three new
  types ship with serde round-trip coverage and committed TypeScript
  bindings.
- `DayseamError` gains two non-failure-looking variants, each with
  their own stable error codes:
  - `Cancelled { code, message }` — surfaced when a run is cancelled
    by the user, by app shutdown, or by a newer run superseding this
    one (`run.cancelled.by_user`, `run.cancelled.by_shutdown`,
    `run.cancelled.by_superseded`). The UI renders this as
    "cancelled", not as an error toast.
  - `Unsupported { code, message }` — surfaced when a connector is
    asked to service a `SyncRequest` variant it has no implementation
    for, e.g. `SyncRequest::Since(Checkpoint)` against a connector
    that only supports day-scoped pulls
    (`connector.unsupported.sync_request`). The orchestrator catches
    this and falls back to the equivalent non-incremental call.
  - Two HTTP-layer codes (`http.retry.budget_exhausted`,
    `http.transport`) are also reserved for the connector SDK's
    shared `HttpClient`.
- `connectors-sdk` crate: the shared plumbing every source connector
  is built on top of.
  - `SourceConnector` trait with a single `sync(ctx, SyncRequest) ->
    SyncResult` method, a `healthcheck(ctx)`, and a stable `kind()`
    tag. `SyncRequest` covers `Day(NaiveDate)`, `Range { start, end
    }`, and `Since(Checkpoint)`; `SyncResult` returns normalised
    `ActivityEvent`s, an optional new `Checkpoint`, `SyncStats`
    (fetched / filtered / http_retries), warnings, and `RawRef`s.
  - `AuthStrategy` trait with `NoneAuth` and `PatAuth` (PAT from the
    macOS Keychain via `dayseam-secrets`), plus an `AuthDescriptor`
    every connector can expose for the UI to render the right
    "connect" affordance.
  - `ConnCtx` — the single context object every connector method
    receives, wiring `run_id`, canonical `person` + known
    `source_identities`, a `ProgressSender` / `LogSender` pair from
    the run's `RunStreams`, a `RawStore`, an injectable `Clock`, a
    shared `HttpClient`, and a `CancellationToken`. A
    `bail_if_cancelled` helper lets connector code short-circuit
    cooperatively on `DayseamError::Cancelled`.
  - `HttpClient` wrapping `reqwest::Client` with a shared retry loop:
    honours `429 Retry-After` (both delta-seconds and HTTP-date),
    retries transient 5xx with exponential backoff + jitter up to a
    configurable `RetryPolicy`, emits per-attempt progress events,
    and treats the run's `CancellationToken` as a hard ceiling —
    every sleep races the token and every attempt re-checks it so
    cancellation is observed within one tick.
  - `Clock` abstraction (`SystemClock` for production,
    `tokio::time::sleep`-backed) and `RawStore` trait (with
    `NoopRawStore` for v0.1) so real raw-payload persistence can land
    in Phase 2 without touching connector code.
  - `MockConnector`: an always-compiled in-memory `SourceConnector`
    driven by a fixture list. Used by downstream tests to exercise
    orchestrator and UI paths without any real HTTP, and self-checked
    with an integration suite covering day filtering, identity
    filtering, ordered progress emission, and correct `Unsupported`
    rejection of `SyncRequest::Since`.
  - Integration tests: `wiremock`-backed `HttpClient` retry and
    cancellation suites, `MockConnector` behavioural tests, and a
    `no_cross_crate_leak` guard that fails the build if
    `connectors-sdk` ever picks up a dependency on `dayseam-db`,
    `dayseam-secrets`, `dayseam-report`, or `sinks-sdk`.
- Canonical sink types on `dayseam-core` (data only, shared across the
  workspace and frontend):
  - `SinkKind` (v0.1 ships `MarkdownFile`; future variants are
    namespaced identically to `SourceKind`).
  - `SinkConfig` enum, currently carrying `MarkdownFile { config_version,
    dest_dirs, frontmatter }`. Every variant carries an explicit
    `config_version` so future schema migrations can be detected
    without inventing a new discriminator.
  - `SinkCapabilities` flags (`local_only`, `remote_write`,
    `interactive_only`, `safe_for_unattended`) plus a `validate()`
    method that returns a `CapabilityConflict` for the three illegal
    shapes (local + remote, interactive + unattended, neither local
    nor remote). `SinkCapabilities::LOCAL_ONLY` is the canonical
    constant for all v0.1 file-writing sinks. The scheduler in Phase 3
    will refuse to dispatch any sink whose capabilities don't satisfy
    the "safe for unattended" predicate, closing the loop on the
    "never auto-send without a human" non-goal.
  - `Sink` record (stored sink configuration + label + timestamps) and
    `WriteReceipt` (what the orchestrator persists after each successful
    write: run id, sink kind, `destinations_written`, `external_refs`,
    `bytes_written`, `written_at`). Both ship with serde round-trip
    coverage and committed TypeScript bindings on
    `@dayseam/ipc-types`.
- `sinks-sdk` crate: the behavioural contract every sink adapter is
  built on top of.
  - `SinkAdapter` trait with `kind()`, `capabilities()`, and two async
    methods: `validate(ctx, cfg)` for eager pre-flight checks (dest
    dirs writable, marker block parseable, etc.) and `write(ctx, cfg,
    draft)` which returns a `WriteReceipt`. The split lets the UI
    surface configuration errors the moment the user confirms a
    destination, instead of at write time.
  - `SinkCtx` mirrors `ConnCtx`: per-write `run_id`, `ProgressSender` /
    `LogSender` from the run's `RunStreams`, and a `CancellationToken`.
    The struct is `#[non_exhaustive]` so additive fields (e.g. an HTTP
    client for the future remote sinks) are source-compatible. A
    `bail_if_cancelled` helper lets sink implementations short-circuit
    between atomic-write boundaries.
  - `MockSink`: an always-compiled in-memory `SinkAdapter` that
    records every `write` call, emits a canonical `Starting → InProgress
    → Completed` progress sequence, honours the cancellation token
    before recording anything, and exposes a one-shot
    `fail_next_with(DayseamError)` so downstream tests can rehearse
    failure paths deterministically.
  - Integration tests: the full 4-flag `SinkCapabilities` matrix (both
    illegal and legal combinations), `MockSink` behaviour (recording,
    ordered progress, cancellation, one-shot injected failure), a
    `SinkCtx` cancellation-to-`DayseamError` smoke test, and a
    `no_cross_crate_leak` guard that fails the build if `sinks-sdk`
    ever picks up a dependency on `dayseam-db`, `dayseam-secrets`,
    `dayseam-report`, or `connectors-sdk`.
- Tauri desktop app shell: a full wireframe-matching window decomposed
  into `TitleBar`, `ActionBar`, `ReportPreview`, `Footer`, and
  `ThemeToggle` components, plus an inline Sources row. Every Phase-1
  interactive element (date picker, Generate report button, source
  cards) ships visibly disabled and title-hinted so the window never
  looks broken.
- Theme system under `apps/desktop/src/theme/` with a `ThemeContext` +
  `ThemeProvider` + `useTheme` triad, a `light | dark | system`
  selection persisted to `localStorage` under `dayseam:theme`,
  `data-theme` + Tailwind `dark` class applied to `<html>`, and a
  `prefers-color-scheme` media-query listener that live-reconciles the
  resolved theme when `system` is selected and the OS appearance
  changes.
- Tauri v2 capability wiring: `apps/desktop/src-tauri/capabilities/default.json`
  lands as an explicit empty allow-list, referenced by
  `tauri.conf.json`'s `app.security.capabilities`. v2's deny-by-default
  model means no Rust command is callable from the frontend until
  Task 9 appends its identifier here — every future IPC command now
  has to pass a review that touches this file.
- Desktop tests: expanded `App.test.tsx` (landmark coverage, disabled
  actions with helpful titles, full Light/System/Dark toggle behaviour,
  persistence round-trip, `aria-checked` marking) and a new
  `App.snapshot.test.tsx` with inline light-theme and dark-theme DOM
  snapshots so layout drift is a reviewed event rather than an
  accidental one.
- IPC bindings end-to-end (Task 9):
  - `Settings`, `SettingsPatch`, and `ThemePreference` added to
    `dayseam-core` with generated TypeScript bindings.
  - `AppState` composed from `SqlitePool` + `AppBus` +
    `Arc<dyn SecretStore>` + `RunRegistry` (the per-run
    `CancellationToken`/`JoinHandle` map used to cancel active syncs).
  - First five Tauri commands wired and allow-listed in
    `capabilities/default.json` via `tauri-build`'s `AppManifest`:
    `settings_get`, `settings_update`, `logs_tail`, plus
    feature-gated `dev_emit_toast` and `dev_start_demo_run`
    compiled only under the `dev-commands` Cargo feature so
    release bundles never expose them.
  - Two forwarder tasks: `broadcast_forwarder` subscribes to
    `AppBus` toasts and re-emits them via `tauri::Manager::emit`
    with lag-recovery logging, and `run_forwarder` drains per-run
    `ProgressEvent` / `LogEvent` streams into frontend-supplied
    `Channel<T>` instances so the channels-vs-broadcast split in
    `ARCHITECTURE.md` §11.3 is now enforced in code.
  - `@dayseam/ipc-types` now exports a `Commands` map that types
    every IPC call; the desktop's typed `invoke(name, args)`
    helper reads its parameters off that map, so adding a Rust
    command without the matching TS entry is a build error.
  - Frontend hooks `useRunStreams`, `useToasts`, `useLogsTail`
    plus `LogDrawer`, `ToastHost`, and `Toast` components wire
    the Rust surface into the UI, with a ⌘L/Ctrl+L shortcut that
    toggles the log drawer.
  - Test coverage: `broadcast_forwarder::emit_toast_reaches_tauri_listeners`
    and `task_exits_cleanly_when_bus_drops` on the Rust side;
    `useRunStreams` (ordering, completion, failure, previous-run
    isolation), `ToastHost`, `LogDrawer`, and an `App` log-drawer
    shortcut test on the TS side.
