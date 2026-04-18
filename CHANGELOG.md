# Changelog

All notable changes to Dayseam are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
