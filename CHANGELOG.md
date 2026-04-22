# Changelog

All notable changes to Dayseam are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **DAY-98: `dayseam-report` — `ArtifactPayload::MergeRequest`
  promotion, `ReportSection::Unlinked` rename, grouper one-pass
  rework (PERF-v0.3-01).** GitLab MR and GitHub PR lifecycle events
  (`MrOpened` / `…Merged` / `…Closed` / `…Approved` /
  `MrReviewComment` + `GitHubPullRequestOpened` / `…Merged` /
  `…Closed` / `…Reviewed` / `…Commented`) now roll up into
  first-class `ArtifactPayload::MergeRequest` artefacts keyed by
  `(provider, project_key, number, day)` and render as one bullet
  each under a new `## Merge requests` section — ordered between
  `## Commits` and `## Jira issues` so the reading flow is "what I
  shipped → what I reviewed → what I triaged → what I wrote →
  stray". The new `OrphanKey::MergeRequest` bucket in
  `rollup::orphan_key` keeps every lifecycle event for a single MR
  in one bucket, stripping `Opened MR:` / `Merged PR:` / etc.
  prefixes to surface the canonical title. Commits that rolled
  into an MR continue to render once under `## Commits` with the
  verbose `(rolled into !N)` suffix — the MR's own bullet is a
  peer, not a duplicate. `ReportSection::Other` is renamed to
  `ReportSection::Unlinked` (id `unlinked`, title
  `Unlinked activity`, CORR-v0.3-01) to read as a user-facing hint
  instead of a grab-bag label; unattached Confluence comments
  still route here. PERF-v0.3-01 replaces the
  `BTreeMap<ReportSection, _>` bucket in `render::build_sections`
  with a fixed-size array indexed by `ReportSection::index()` — one
  walk, O(1) inserts, same render order. `MergeRequestProvider`
  gains `PartialOrd` / `Ord` so it can key the orphan bucket.
  Tests: six new invariants in `tests/mr_promotion.rs`
  (`gitlab_mrs_render_under_merge_requests_section`,
  `github_prs_render_under_merge_requests_section`,
  `commits_rolled_into_mr_render_once`,
  `unlinked_section_renders_confluence_orphan_comments`,
  `grouper_makes_single_pass_over_rollup`) plus a render-order lock
  in `sections::ord_matches_render_order` pinning
  `Commits → MergeRequests → JiraIssues → ConfluencePages → Unlinked`.

- **DAY-97: `dayseam-report` — GitHub PR ↔ Jira enrichment + cross-source
  PR↔MR linking + verbose `(triggered by …)` rendering.**
  `annotate_transition_with_mr` is now provider-agnostic: alongside
  GitLab `MrOpened` / `MrMerged`, it credits `GitHubPullRequestOpened` /
  `…Merged` / `…Closed` as triggering events for a Jira transition.
  A new `MR_TRIGGER_WINDOW = 24h` constant enforces the temporal guard
  the DAY-88 docstring promised but never actually applied — a
  candidate MR/PR must fall in `[transition - 24h, transition]` to
  be credited. New pass `extract_github_pr_urls` (regex-free, same
  rationale as `scan_ticket_keys`) scans GitLab MR titles + bodies
  for `https://github.com/<owner>/<repo>/pull/<N>` URLs and attaches
  an `EntityKind::GitHubPullRequest` entity with
  `external_id = "<repo>#<N>"` + `label = "#<N>"` on the MR event so
  the evidence popover can surface the cross-link. Wired into
  `pipeline` between `extract_ticket_keys` and
  `annotate_transition_with_mr` so the usual ordering invariants
  hold. `render_atlassian_bullet` now takes a `verbose_mode` flag and
  renders a `(triggered by <label>)` suffix when the transition
  carries a `parent_external_id` — GitLab MRs pass through as
  `!321`, GitHub PRs strip the repo prefix to `#42` to match the
  notation GitHub itself uses. `group_key_from_event` adds an
  explicit GitHub arm so PR / issue events group by the
  `github_repo` entity's `owner/repo` slug instead of landing in the
  `/` orphan bucket. Tests: unit coverage for the URL scanner
  (single / multiple / trailing path fragments / non-GitHub hosts /
  non-MR events), the 24h temporal guard (before / after / exactly
  at the window edge), the suffix-shape helpers
  (`!321` pass-through, `repo#42` → `#42`, empty → `None`, unknown
  → verbatim), the GitHub-repo grouping arm, and three integration
  tests pinning the end-to-end shape: verbose mode renders
  `(triggered by !321)` for GitLab and `(triggered by #42)` for
  GitHub, plain mode hides the suffix, and a mixed GitLab + GitHub
  + Jira day still produces one bullet per event with the earliest
  credit winning.
- **DAY-96: `connector-github::walk` — events walker + normaliser +
  rapid-review collapse.** `GithubConnector::sync(SyncRequest::Day)`
  now walks a local-timezone day of GitHub activity end-to-end. Four
  new modules land: `events.rs` (strict envelope DTOs for
  `/users/:login/events` + `/search/issues`, with
  `GithubEventPayload::from_raw` decoding each action lazily so
  unknown event types are silently routed to an `Unknown { reason }`
  variant rather than failing the walk); `normalise.rs` (one arm per
  supported event type — `PullRequestEvent` → opened / closed /
  merged, `PullRequestReviewEvent` → reviewed with `review_state` in
  metadata, `PullRequestReviewCommentEvent` / `IssueCommentEvent` on
  PRs → commented, `IssuesEvent` → opened / closed / assigned-self,
  `IssueCommentEvent` on issues → commented); `rollup.rs` (rapid-review
  collapse: N `GitHubPullRequestReviewed` events on the same PR by
  the same author within `RAPID_REVIEW_WINDOW_SECONDS = 60` fold into
  one event whose `metadata.review_count == N` and whose
  `metadata.review_state` is the last review's state — symmetric to
  Jira's rapid-transition collapse); and `walk.rs` (combines the
  events stream and `/search/issues?q=involves:<login>+updated:<start>..<end>`
  with cross-stream dedup on `(ActivityKind, external_id)`; events
  stream wins on conflict since its payload carries the actor + the
  actual action). Jira ticket keys in PR / issue titles (e.g.
  `CAR-5117: Fix …`) are extracted by a hand-rolled parser
  (deliberately avoids pulling in the `regex` crate — the graph stays
  lean and no other connector uses it) and added as
  `EntityKind::JiraIssue` entities, setting up DAY-97's cross-source
  enrichment. In-page dedup within the events stream keys off the
  raw GitHub `event.id` so multiple distinct reviews on the same PR
  survive to reach the rollup. 401 → `github.auth.invalid_credentials`,
  410 → `github.resource_gone` (GitHub returns 410 for fully deleted
  user accounts). Identity resolution requires both
  `SourceIdentityKind::GitHubUserId` (for filtering `event.actor.id`)
  and `SourceIdentityKind::GitHubLogin` (for composing
  `/users/:login/events`); missing either early-bails with an empty
  outcome and a `Warn` log, never a silent zero-event day. Tests:
  62 in-crate unit tests (per-event-type normalisation, ticket-key
  extraction edge cases, rollup boundary conditions, self-identity
  resolution, day-window math, search-hit synthesis) and 8 wiremock
  integration tests in `crates/connectors/connector-github/tests/walk.rs`
  pinning the full authn → HTTP → paginate → normalise → rollup
  round-trip: 401 / 410 surface as the documented `github.*` codes;
  events from other actors are filtered; events outside the local
  day's UTC bounds are filtered; a PR that appears in both the
  events stream and `/search/issues` surfaces exactly once and the
  dedup counter increments; three rapid reviews collapse into one;
  the walker early-bails with an empty outcome when no GitHub
  identity is configured (the wiremock `.expect(0)` verifies no
  request was issued); and `/search/issues` receives the
  `involves:<login> updated:<start>..<end>` clause so scope is bounded
  to the user's activity within the window.
- **DAY-95: `connector-github` scaffold + `SourceConnector` +
  `validate_auth` + `LinkHeaderPaginator` + `errors::map_status`.**
  New crate `connector-github` lands with the minimal surface needed
  for DAY-96's walker to be a thin loop: `GithubConnector` implements
  `SourceConnector` (`kind() == SourceKind::GitHub`; `healthcheck`
  probes `GET /user` with `Authorization: Bearer …`, `Accept:
  application/vnd.github+json`, `X-GitHub-Api-Version: 2022-11-28`;
  `sync` returns `DayseamError::Unsupported` +
  `CONNECTOR_UNSUPPORTED_SYNC_REQUEST` for `Day` / `Range` / `Since`
  until the walker ships). `auth::validate_auth` classifies 401 →
  `github.auth.invalid_credentials`, 403 →
  `github.auth.missing_scope`, 404 → `github.resource_not_found`
  (with the documented "forgot `/api/v3` on your GHE URL" hint in
  the error message), and transport errors → reused
  `gitlab.url.{dns,tls}` codes (UI copy is host-agnostic; minting
  `github.url.*` twins would bloat the registry without changing
  behaviour). `pagination::next_link` + `parse_next_from_link_header`
  parse GitHub's RFC 8288 `Link` header, tolerating bare-token
  `rel=next`, multi-token `rel="next prev"`, reordered entries,
  malformed URLs (silent-failure-avoidance: stops pagination rather
  than crashing the walk), and absent / whitespace-only headers.
  `errors::map_status` extends the DAY-89 5xx / 410 symmetry to a
  quadruplet: `crates/dayseam-core/tests/error_codes.rs` now asserts
  `{gitlab,jira,confluence,github} × {upstream_5xx, resource_gone}`
  are all registered in `error_codes::ALL`; the orchestrator-level
  `server_error_symmetry` test gains a `github` arm so a future
  mapping drift fails the same way for every family. `GithubMux`
  plugs into `DefaultRegistryConfig::github_sources`; desktop
  startup hydrates `GithubConfig` from persisted
  `SourceConfig::GitHub { api_base_url }` rows. `build_source_auth`
  loses its DAY-93 `ipc.github.not_implemented` placeholder and now
  returns `PatAuth::github` — a boundary enabler for end-to-end
  testing, not a user-visible feature (Add-Source dialog still
  lands in DAY-99). Tests:
  `crates/connectors/connector-github/tests/{scaffold,auth,pagination}.rs`
  pin the registration, authentication, header-shape, and pagination
  seams against a wiremock server; 33 in-crate unit tests cover
  config parsing, error taxonomy, user-info decoding, identity
  seeding, and Link-header edge cases.
- **DAY-94: `PatAuth::github` constructor + connectors-SDK DTO design
  note.** New `PatAuth::github(token, keychain_service, keychain_account)`
  constructor (delegates to `PatAuth::bearer` — GitHub's classic and
  fine-grained PATs both accept the `Authorization: Bearer <token>`
  shape) with inline unit tests pinning the header, descriptor
  round-trip, Debug-no-leak, and shared-handle invariants.
  Integration test `crates/connectors-sdk/tests/github_pat_auth.rs`
  mirrors the Atlassian suite and re-proves CORR-01 for GitHub: 401
  / 403 responses flow through `HttpClient::send` as raw
  `reqwest::Response` objects so `connector-github` (DAY-95) owns
  the `github.auth.*` classification. New `connectors_sdk::dtos`
  doc-only module documents the persisted-state-vs-HTTP-DTO
  convention (persisted: `SerdeDefaultAudit` required; HTTP:
  `#[serde(default)]` freely) resolving CONS-v0.3-01 from the v0.3
  capstone. A new trybuild pass fixture
  `accepts_github_variant.rs` locks in that `SourceConfig::GitHub`'s
  required-only `api_base_url` field passes the audit derive
  alongside the existing audited-variant shape.
- **DAY-93: `dayseam-core` GitHub types landed.** `SourceKind::GitHub`
  + `SourceConfig::GitHub { api_base_url }`,
  `SourceIdentityKind::GitHubUserId`, nine `ActivityKind::GitHub*`
  variants, four new `EntityKind`s (`GitHubRepo`, `GitHubPullRequest`,
  `GitHubIssue`, `Workspace`), `ArtifactKind::GitHubPullRequest` /
  `GitHubIssue`, the first-class `ArtifactPayload::MergeRequest` variant
  with its `MergeRequestProvider` discriminant (`GitLab` | `GitHub`),
  and seven `GITHUB_*` error codes. Every downstream crate that matches
  on these enums (`dayseam-db`, `dayseam-report`, `dayseam-orchestrator`,
  `apps/desktop/src-tauri`, `connector-gitlab`, `connector-local-git`)
  picks up the new variants as dormant placeholders — the GitHub
  connector itself lands in DAY-95 and the `MergeRequest` renderer
  wiring lands in DAY-98, so today the variants compile cleanly but
  produce no user-visible output. No DB migration: `sources.kind` is a
  plain `TEXT` column, so the new string value is added directly (the
  DAY-92 plan's draft of migration `0006_github_sources.sql` was
  dropped — see the plan doc's Task 2 status note).
- **DAY-92: v0.4 plan drafted** (`docs/plan/2026-04-22-v0.4-github-connector.md`).
  Headline track is the fifth connector (`connector-github`) plus the v0.3
  seam promotions that naturally land alongside it (first-class
  `ArtifactPayload::MergeRequest`, `EntityKind::Workspace`,
  `ReportSection::Unlinked` rename). Also absorbs the four v0.3 deferred
  findings (TST/PERF/CONS/CORR-v0.3-0X). 10 PRs (DAY-92..DAY-101); only
  DAY-101 carries `semver:minor`, matching the v0.3 release-workflow
  discipline.

## [0.3.0] - 2026-04-22

### Changed

- **v0.3 capstone — report polish + deferred-findings hardening (DAY-85..DAY-91).**
  v0.3 is a polish + hardening phase, not a feature phase: no new
  connectors, no new event kinds, no new IPC surface beyond the one
  Reconnect-prefill helper. The headline user-visible changes are
  **per-kind report sections** (DAY-86 — events now group under
  `Commits` / `Jira issues` / `Confluence pages` headings instead
  of a single `COMMITS` catchall that previously labelled Jira
  transitions as "Commits") and the **Atlassian Reconnect chip**
  (DAY-87 — the previously dead chip now opens the Add-Source
  dialog pre-filled with workspace URL + email, mirroring the
  GitLab Reconnect flow that shipped in v0.1). Behind the UX, the
  v0.2 capstone's 22 deferred Medium findings (umbrella issues
  [#83](https://github.com/vedanthvdev/dayseam/issues/83)–[#87](https://github.com/vedanthvdev/dayseam/issues/87))
  are resolved: silent-failure sweep across the Confluence
  normaliser + Jira walker + orchestrator temporal ordering
  (DAY-88); `EntityRef.kind` promoted to a proper `EntityKind`
  enum with lossless custom serde, 5xx / 410 classification
  symmetry across every connector, and migration `0005_secret_ref_index.sql`
  indexing the shared-secret column (DAY-89); count-aware E2E
  assertions, a real-Rust orchestrator-level Atlassian integration
  test backed by wiremock, and the new `SerdeDefaultAudit`
  proc-macro derive that makes any future `#[serde(default)]`
  field on a persisted type fail to compile unless it names a
  paired repair or carries an explicit waiver (DAY-90). Each of
  DAY-85..DAY-90 carried `semver:none` so intermediate merges did
  not auto-release; this capstone is the only v0.3 PR with
  `semver:minor` and is the one that triggers the `v0.3.0` tag —
  the release-workflow policy correction the v0.2 review called
  for. Full lens-by-lens writeup in
  [`docs/review/v0.3-review.md`](docs/review/v0.3-review.md); the
  3-day dogfood sweep is scaffolded in
  [`docs/dogfood/v0.3-dogfood-notes.md`](docs/dogfood/v0.3-dogfood-notes.md)
  and runs against the published `v0.3.0` DMG. The capstone review
  surfaced four deferred items (2 × Medium + 2 × Low) across the
  test-quality, efficiency, cross-source, and correctness lenses;
  all four are small enough to thread directly into the v0.4 plan
  doc rather than carry through v0.4 as standalone umbrella
  issues. **DAY-86 per-kind report sections.** `dayseam-report`
  gains a new `sections` module whose `ReportSection` enum is the
  single source of truth for "which heading does this payload
  render under" via an exhaustive `match` on `ArtifactPayload`.
  The match is deliberately exhaustive so any future
  `ArtifactPayload` variant fails to compile until its author
  picks a section, which is the compile-time nudge that prevents
  a silent fall-through to `Commits` from ever recurring (that
  fall-through is exactly what produced the v0.2 "Jira transition
  labelled as a Commit" bug). Section ordering is the derived
  `Ord` on the enum's declaration order — `Commits`, `JiraIssues`,
  `ConfluencePages`, `Other` — not alphabetical, because the
  intended reading order is "what I shipped → what I triaged →
  what I wrote". Empty sections are omitted; the empty-*day*
  fallback still renders a single `## Commits` "No tracked
  activity" bullet so the desktop streaming preview's existing
  contract doesn't break. Golden snapshots under
  `crates/dayseam-report/src/templates/` pin the rendered output
  for a mixed Jira + Confluence + GitLab + local-git day — the
  "mixed-day section heading test" issue [#85](https://github.com/vedanthvdev/dayseam/issues/85)
  called out. **DAY-87 Atlassian Reconnect dialog parity.** The
  Reconnect chip on an Atlassian source with a rejected token now
  opens `AddAtlassianSourceDialog` pre-filled with the source's
  workspace URL + email, routed through a new IPC command
  `atlassian_sources_prefill_for_reconnect` (added to the Tauri
  capability whitelist and typed in `@dayseam/ipc-types`). The
  pre-fill read path reuses the row the Reconnect chip already
  has in the sidebar store — no extra `sources_list` round-trip —
  and collapses to the Journey A (shared-PAT) invariant when the
  source's `secret_ref` points at a keychain slot another
  Atlassian source also references. Validate-edit semantics: if
  the user edits the email between pressing Validate and Submit,
  the dialog forces a fresh Validate click before accepting
  Submit — the second Validate must not reuse the first's cached
  result. A new RTL test (`AddAtlassianSourceDialog.validate-edit.test.tsx`)
  and an E2E BDD scenario (`atlassian-reconnect-validate-edit.feature`)
  guard this contract. **DAY-88 silent-failure sweep.** Nine
  findings (`CORR-v0.2-02..10`) inlined from the v0.2 deferred
  umbrella. Confluence normaliser hardening: `version.number`
  defaults are gone (missing version now returns a shape error),
  ADF parser errors surface via `Result` rather than being
  swallowed into an empty preview, unparseable `createdDate`
  values fail loudly instead of defaulting to the walk's target
  date. Jira empty-transition render path no longer emits a blank
  bullet when the issue has no name text. Orchestrator temporal
  ordering for MR↔transition enrichment now requires the MR to
  *precede* the transition by at most 24h before surfacing the
  `(triggered by !<iid>)` annotation — previously it could
  annotate a transition that happened hours before the MR even
  existed. Rollback warning surface: the keychain error path in
  `atlassian_sources_add_impl` now surfaces every upstream error
  via `tracing::warn!` rather than silencing any of them. The
  `SerdeDefaultRepair` trait that DAY-90's macro builds on is
  introduced here — every `#[serde(default)]` field on a
  persisted type now has a documented named repair or a reasoned
  waiver, starting with `confluence_email` (the repair helper the
  v0.2.1 hotfix shipped). Shared-secret refcount race is resolved
  by holding the transactions lock across the delete-count +
  delete-keychain compound action; dropped error-body previews
  are now included in the `tracing::warn!` message chain. **DAY-89
  cross-source consistency.** `EntityRef.kind` promoted from
  free-form `String` to `EntityKind` enum with custom
  `Serialize` / `Deserialize` that preserves the exact snake_case
  string wire shape every v0.2.1 connector emitted — meaning
  v0.2.1 `activity_events` rows round-trip byte-stable on the
  v0.3.0 upgrade with no migration. Unknown kinds deserialise as
  `EntityKind::Other(String)` carrying the original string, so a
  future connector's new kind doesn't break a rollback. The
  `ActivityKind` enum gains a doc-comment recording its naming
  convention (noun-past-participle pairs per source, e.g.
  `JiraIssueTransitioned`, `ConfluencePageCreated`) so the next
  connector's author picks a name consistent with the existing
  seven. 5xx / 410 classification symmetry: every connector's
  `map_status` function now routes `5xx` to `DayseamError::Network`
  (was `UpstreamChanged` for Atlassian, `Network` for GitLab —
  the asymmetry the v0.2 review flagged), and `410 Gone` routes
  to a new `{service}.resource_gone` error code (`jira.resource_gone`,
  `confluence.resource_gone`, `gitlab.resource_gone` — three
  symmetric codes registered in the `dayseam-core::error_codes::ALL`
  snapshot so accidental drops fail the test). Migration
  `0005_secret_ref_index.sql` creates a partial index
  `idx_sources_secret_ref ON sources(secret_ref) WHERE secret_ref IS NOT NULL`
  — zero-downtime for upgraders (the `IF NOT EXISTS` clause
  handles the repeated-boot case), useful the moment the shared-
  secret repair pipeline DAY-90 builds on has to look up repair
  candidates by `secret_ref`. The `workspace` entity variant the
  v0.2 review called for is **deferred to v0.4** — the plan's
  explicit scope decision, recorded in the v0.3 plan's "Risks /
  out-of-scope" list — because no call-site currently needs it
  and adding it would widen the `EntityKind` enum without a
  corresponding value delivery. **DAY-90 test-quality floor.**
  Count-aware E2E assertions: `StreamingPreview.tsx` now stamps
  `data-section={section.id}` on each rendered section and
  `data-bullet={bullet.id}` on each bullet, giving Playwright
  stable DOM hooks. The page object gains
  `expectSectionBulletCount(sectionId, n)` and
  `expectSectionContainsBullet(sectionId, text)`, and all four
  existing happy-path scenarios migrate onto them — the presence-
  not-count assertions the v0.2 review flagged as inadequate are
  gone. Real-Rust orchestrator-level Atlassian integration test:
  new `crates/dayseam-orchestrator/tests/atlassian_integration.rs`
  exercises the full `GenerateRequest → orchestrator → JiraMux
  + ConfluenceMux → normaliser → persist → ReportDraft` stack
  against `wiremock` backends in three scenarios (Jira-only,
  Confluence-only, both-at-once with independent mock servers) —
  the orchestrator-level test [#85](https://github.com/vedanthvdev/dayseam/issues/85)
  called out as the absent-from-v0.2 coverage. Registry
  round-trip invariant: `registry_kind_round_trips_for_every_registered_connector`
  in `registries.rs` exhaustively iterates `SourceKind` (so
  adding a new kind fails to compile until it's either registered
  or explicitly excluded) and asserts every registered
  connector's `kind()` matches its registration key. Validate-
  edit dialog RTL test (see DAY-87 entry). New `dayseam-macros`
  proc-macro crate with the `SerdeDefaultAudit` derive: every
  `#[serde(default)]` field on a type carrying the derive must
  carry a paired `#[serde_default_audit(repair = "NAME")]` (naming
  a registered `SerdeDefaultRepair`) or an explicit
  `#[serde_default_audit(no_repair = "reason")]` waiver — failing
  either shape produces a compile error whose message names the
  offending field and cites the DOG-v0.2-04 background. A
  `trybuild` suite (4 compile-pass + 3 compile-fail fixtures)
  pins the derive's behaviour; the derive is applied to
  `SourceConfig` (the v0.2.1 `#[serde(default)]` Confluence email
  field now carries the `confluence_email` repair annotation) and
  `EntityRef` (no `#[serde(default)]` fields today — the derive
  is a zero-cost future-proofing nudge).

## [0.2.1] - 2026-04-21

### Fixed

- **v0.2 capstone review — hardening hotfix (DAY-84).** Twelfth and
  final task of the v0.2 Atlassian arc. The v0.2.0 release was cut
  by the release workflow immediately after [DAY-83](https://github.com/vedanthvdev/dayseam/pull/82)
  merged with a `semver:minor` label — before the capstone review
  battery had a chance to run. This hotfix lands the seven P0/HIGH
  findings the review surfaced. The remaining 22 Medium + Low findings
  are filed as five themed GitHub umbrella issues ([#83](https://github.com/vedanthvdev/dayseam/issues/83),
  [#84](https://github.com/vedanthvdev/dayseam/issues/84),
  [#85](https://github.com/vedanthvdev/dayseam/issues/85),
  [#86](https://github.com/vedanthvdev/dayseam/issues/86),
  [#87](https://github.com/vedanthvdev/dayseam/issues/87))
  and deferred to v0.3. Full reviewer writeups in
  [`docs/review/v0.2-review.md`](docs/review/v0.2-review.md); the
  three-day dogfood sweep is scaffolded in
  [`docs/dogfood/v0.2-dogfood-notes.md`](docs/dogfood/v0.2-dogfood-notes.md)
  and runs against the published v0.2.1 DMG. **DOG-v0.2-01 —
  `build_source_auth` now constructs `BasicAuth` for
  Jira/Confluence.** The `Unsupported` stub that had silently
  survived from DAY-74 is gone. Adding a Jira source, restarting,
  and pressing Generate report now returns a real `ReportDraft`
  instead of aborting the entire run with `CONNECTOR_UNSUPPORTED_SYNC_REQUEST`
  (which, because `build_source_auth` runs in the pre-loop with
  `?`, previously also erased any GitLab bullets selected for the
  same run). **DOG-v0.2-02 — Jira/Confluence muxes are hydrated
  at startup + upserted on add.** `resolve_registry_config` in
  `startup.rs` now matches `(SourceKind::Jira, SourceConfig::Jira { … })`
  + the Confluence twin and pushes `JiraSourceCfg` /
  `ConfluenceSourceCfg` into the mux constructor. `atlassian_sources_add_impl`
  now calls `mux.upsert(…)` after the DB/keychain transaction
  commits. The "restart required" toast that previously masked
  this gap is no longer necessary. **DOG-v0.2-03 — workspace URL
  normalisation enforces `.atlassian.net` (security).** Both the
  client-side `normaliseWorkspaceUrl` and the server-side
  `parse_workspace_url` now reject any host that doesn't end in
  `.atlassian.net` (case-insensitive, post-IDN). Previously, a user
  who typoed a hostname — or pasted a phishing link — would ship
  their PAT to `https://evil.com/rest/api/3/myself` on the Validate
  button press, carrying `Authorization: Basic <base64(email:token)>`.
  New error reason: *"Only Atlassian Cloud hosts (e.g. `modulrfinance.atlassian.net`)
  are supported."* **CORR-v0.2-01 — `confluence_page` entity on
  `ConfluenceComment` events.** `normalise_comment` now pushes a
  `confluence_page` entity onto every comment event via a new
  `comment_parent_page_ref` helper that pulls the parent page id +
  title from `content.ancestors[]` / `content.container`. Without
  it, five comments on five different pages in one space collapsed
  to one synthetic `ConfluencePage` artifact with `page_id = "UNKNOWN"`
  and colliding `artifact_id`s — the evidence popover resolved to
  the same artifact for every comment. New rollup test
  `confluence_comment_on_different_pages_bucket_into_separate_synthetic_artifacts`.
  **CONS-v0.2-01 — Atlassian self-identity parity with GitLab.**
  New `ensure_atlassian_self_identity` helper (idempotent on the
  unique index, re-asserts from existing DB rows without a network
  hop) wired into `sources_update` for `SourceKind::Jira |
  SourceKind::Confluence`. New `backfill_atlassian_self_identities`
  in `startup.rs` mirrors the DAY-71 GitLab backfill, invoked right
  after it. Without these, a PAT rotation or workspace-URL edit
  through `sources_update` would re-write the keychain but not
  touch `source_identities` — and a missing `AtlassianAccountId`
  row silently drops every event in the render-stage self-filter,
  the exact DAY-71 silent-empty shape re-introduced for Atlassian.
  **TST-v0.2-01 — `walk_day` auth-mapping tests for Jira +
  Confluence.** New `walk_day_maps_401_to_atlassian_auth_invalid_credentials`
  + `walk_day_maps_403_to_atlassian_auth_missing_scope` in both
  `connector-jira/tests/walk.rs` and `connector-confluence/tests/walk.rs`.
  Before, the 401/403 surface was tested only through `validate_auth`
  and `discover_cloud` — a refactor that converted `walk_day`'s
  non-success branch to "log + `Ok(SyncOutcome::empty())`" would
  have passed every existing test and re-introduced DAY-71
  silent-empty. **CONS-v0.2-02 — 404/429 arms in
  `gitlab::map_status`.** New `GITLAB_RESOURCE_NOT_FOUND = "gitlab.resource_not_found"`
  error code + `GitlabUpstreamError::ResourceNotFound { message }`
  variant → `DayseamError::Network`. `map_status` now routes
  `NOT_FOUND` to `ResourceNotFound` (was misreported as
  `gitlab.upstream_5xx`) and `TOO_MANY_REQUESTS` to `RateLimited`
  with a conservative `retry_after_secs: 0` (the SDK layer reads
  the real `Retry-After` header). The `validate_pat_is_active`
  comment was updated to explain that the now-redundant 429 arm is
  kept as a defensive layer against a future refactor that accidentally
  drops the 429 routing from `map_status`. New tests
  `map_status_routes_404_to_resource_not_found` +
  `map_status_routes_429_to_rate_limited_with_zero_retry_after_as_conservative_default`
  + the `error_taxonomy_matches_design` insta snapshot regenerated.
  Full taxonomy cleanup — dot-separator depth, `invalid_token` vs
  `invalid_credentials`, 5xx routing to `Network` vs `UpstreamChanged`
  — is deferred to [#87](https://github.com/vedanthvdev/dayseam/issues/87)
  (CONS-v0.2-03/04/05) and wants a single connector-conventions ADR
  in v0.3 before the GitHub connector starts. **DOG-v0.2-04 —
  v0.2.0 → v0.2.1 Confluence email upgrade backfill.** Caught during
  dogfood of this very PR: v0.2.0 persisted `SourceConfig::Confluence`
  with only `workspace_url`; this hotfix added a required `email`
  field with `#[serde(default)]` so old rows still deserialise, but
  `build_source_auth` then rejected them with
  `atlassian.auth.invalid_credentials` — before any network call,
  even though the token was fine. Users who connected Jira and
  Confluence together on v0.2.0 saw Jira bullets render and
  Confluence bullets silently missing with a confusing "token rejected"
  message. `backfill_atlassian_confluence_email` now runs on boot:
  for every Confluence row with empty email, it finds the sibling
  Atlassian source that shares the same `secret_ref` (Journey A's
  shared-PAT invariant) and copies the sibling's email across via
  `SourceRepo::update_config`. Confluence-only installs (no sibling)
  are logged + left alone so the Reconnect flow catches them — we
  deliberately do not fall back to matching on workspace URL alone,
  which would risk copying an email across two independently-added
  tenants on the same host. New tests
  `confluence_email_backfill_copies_from_jira_sibling_sharing_secret_ref`,
  `…_leaves_row_alone_when_no_sibling`,
  `…_is_noop_when_email_already_present`,
  `…_skips_sibling_with_different_secret_ref`.

## [0.2.0] - 2026-04-21

### Added

- **v0.2 orchestrator registry wiring + Playwright happy-path E2E for
  Atlassian (DAY-83).** Eleventh task of the v0.2 Atlassian arc.
  Closes the loop between the DAY-76/77 Jira connector, the
  DAY-79/80 Confluence connector, the DAY-82 add-source dialog, and
  the user-visible "a Jira/Confluence bullet appears in my daily
  report" contract, with three BDD scenarios exercising the full
  renderer stack on every PR. **Registry hydration smoke.**
  `default_registries_populate_shipping_kinds` now asserts the
  connector registry's `.kinds()` is **exactly** `{LocalGit, GitLab,
  Jira, Confluence}` and the sink registry's is exactly
  `{MarkdownFile}`. Using a `HashSet` equality (rather than
  individual `.get(kind).is_some()` probes) catches both directions
  of regression — a kind that silently drops out (orchestrator
  fan-out skips it) and a spurious extra kind that slips in without
  a matching `DefaultRegistryConfig` field (mux running on a default
  config that ignores the user's sources). **Three new Playwright
  scenarios under `@connector:atlassian`.** The suite drives the
  real `AddAtlassianSourceDialog` end-to-end — sidebar menu → product
  checkboxes → workspace URL normalisation → email + API token →
  `atlassian_validate_credentials` → submit via
  `atlassian_sources_add` → generate report → assert per-product
  bullet appears. (1) `@atlassian-jira-only` wires one Jira source,
  confirms the workspace slug `dayseam-e2e` normalises to the
  canonical `https://dayseam-e2e.atlassian.net` origin, then checks
  the Jira bullet lands in the Completed section. (2)
  `@atlassian-confluence-only` does the same for Confluence.  (3)
  `@atlassian-both` ticks both products (Journey A — shared PAT),
  submits once, and asserts both bullets appear in the same
  Completed section — the "grouped correctly" invariant from the
  plan. Every scenario ends with `no console or page errors were
  captured during the run` so an uncaught renderer exception in any
  Atlassian code path fails the build loudly. **Mock surface
  extension.** `tauri-mock-init.ts` gains handlers for
  `atlassian_validate_credentials` (returns the catalogue-seeded
  account triple) and `atlassian_sources_add` (captures the full
  IPC payload on `state.captured.atlassianAddCalls` for future
  contract assertions, appends the fresh Jira / Confluence rows to
  a closure-local sources array the sidebar reads via
  `sources_list`, and mints a shared `secret_ref` that mirrors the
  Rust-side `dayseam.atlassian::slot:<uuid>` contract). The draft
  returned by `report_get` is now built on demand from the current
  sources list, so a scenario that adds Jira sees the Jira bullet,
  one that adds Confluence sees the Confluence bullet, and one that
  adds both sees both — the same per-source-conditioning the real
  Rust renderer applies. **New infrastructure pieces.**
  `e2e/page-objects/atlassian/atlassian-dialog-page.ts` is the
  single surface the steps talk to (one intent-named method per
  user action — `openFromSidebar`, `selectOnlyJira`,
  `selectOnlyConfluence`, `selectBothProducts`,
  `fillCredentialsFromFixture`, `validateCredentials`, `submit`,
  `expectNormalisedWorkspaceUrl`); `atlassian-dialog-locators.ts`
  pins every `data-testid` the dialog exposes so a React-side
  rename is a single edit here. `e2e/steps/ui-steps/atlassian/
  atlassian-steps.ts` hosts the new Gherkin bindings, registering
  the new `@atlassian` domain tag and `@connector:atlassian`
  family per the README's tag taxonomy. **Catalogue additions.**
  `e2e/fixtures/runtime/catalogue.ts` gains the Atlassian fixture
  (workspace slug + canonical URL, email, API token placeholder,
  account triple, shared `SecretRef` slot) and two per-product
  bullet strings the mock appends when the matching source is
  present; the feature file asserts against those exact strings
  so a drift between "what the mock serves" and "what the scenario
  expects" is a fixture-pinned change, not a silently-passing
  test. **Stability.** All three scenarios pass on three
  consecutive `pnpm e2e` runs (the README's "not stable at three =
  not done" invariant), with headless total time ≈3.5s for the
  Atlassian feature.

- **v0.2 `apps/desktop` — Atlassian add-source UI + IPC (DAY-82).**
  Tenth task of the v0.2 Atlassian arc. Ships the user-facing surface
  for connecting Jira and Confluence so every earlier task in the arc
  (DAY-73 walker, DAY-76/77 Jira + Confluence connectors, DAY-78
  orchestrator wiring, DAY-79 onboarding checklist, DAY-80 identity
  manager, DAY-81 shared-secret refcount) becomes reachable from the
  desktop shell instead of only from integration tests. **New IPC
  commands.** `atlassian_validate_credentials(workspaceUrl, email,
  apiToken) -> AtlassianValidationResult` probes
  `GET /rest/api/3/myself` with Basic auth over `connectors-sdk`'s
  `HttpClient`, returning the `{account_id, display_name, email}`
  triple the dialog needs both for its "Connected as …" confirmation
  ribbon and for seeding `SourceIdentity::AtlassianAccountId` on
  persist. `atlassian_sources_add(workspaceUrl, email, apiToken,
  accountId, enableJira, enableConfluence, reuseSecretRef?) ->
  Source[]` is a single round-trip that covers the four journeys
  the product supports — Journey A (shared PAT, both products),
  Journey B (single product), Journey C mode 1 (reuse the existing
  product's `secret_ref` for the other product, no new keychain
  row), Journey C mode 2 (separate PAT for the other product). The
  command is transactional end-to-end: if any step fails after the
  keychain write, `rollback_sources_add` drops partial `sources`
  rows and — only when the caller asked us to mint a new slot —
  deletes the keychain entry, leaving the DB and keychain in the
  same state as before the command. Both commands live in
  `apps/desktop/src-tauri/src/ipc/atlassian.rs` and are registered
  in `main.rs` + `build.rs` + `capabilities/default.json` +
  `PROD_COMMANDS` so the Tauri capability / Rust handler / TS type
  quadruple-write invariant from `ARCHITECTURE.md` §6 stays intact.
  **Keychain account scheme.** New Atlassian slots are keyed
  `dayseam.atlassian::slot:<uuid>` — UUID-based rather than
  id-per-product — so the shared-PAT flow can point two `sources`
  rows at the same `keychain_account` and DAY-81's refcounted
  delete path treats the pair as shared from the first insert. The
  Journey-C mode-1 reuse path takes an `Option<SecretRef>` directly,
  never re-prompts for the PAT, and writes zero new keychain rows.
  **New DTO.** `AtlassianValidationResult` is a `dayseam-core` type
  exported via `ts-rs` (the upstream
  `connector-atlassian-common::cloud::AtlassianAccountInfo` stays
  free of IPC concerns); mirrors the cloud crate's account triple
  plus optional email. **Dialog.**
  `AddAtlassianSourceDialog.tsx` renders one checkbox per product,
  a workspace-URL field that previews the normalised canonical form
  live (bare slugs expand to `https://<slug>.atlassian.net`, full
  URLs strip trailing slashes, `http://` is refused outright rather
  than silently upgraded), an "Open token page" shell-out, a paste
  field for the API token, and a Validate button that wires into
  `atlassian_validate_credentials` before the submit button is
  enabled. When an Atlassian source already exists the dialog
  detects the asymmetry, pre-collapses to the missing product, and
  surfaces a "Reuse / paste different token" radio pair —
  Journey-C is a one-click flow when the existing source already
  has a cached account id, or a short paste-and-validate otherwise.
  **URL normalisation.**
  `apps/desktop/src/features/sources/atlassian-workspace-url.ts`
  encapsulates the one canonical form every downstream code path
  (IPC, DB, identity seeding, keychain account) needs; the unit
  test table exercises every documented input shape. **Error copy
  parity.** `SourceErrorCard.tsx` learns the nine Atlassian error
  codes from DAY-76/77 (`atlassian.auth.invalid_credentials`,
  `atlassian.auth.missing_scope`, `atlassian.network.*`,
  `jira.*`, `confluence.*`) and renders product-specific messages
  with a "Reconnect" action for the auth-flavoured codes —
  `atlassianErrorCopy.ts` carries one entry per code and the
  `atlassianErrorCopy` parity test in
  `apps/desktop/src/features/sources/__tests__/atlassianErrorCopy.test.ts`
  fails if the Rust catalogue and the TS copy drift. **Sidebar
  wiring.** The "Add source" menu grows a third item ("Add Atlassian
  source") and the sidebar passes the current `sources` list into
  the dialog so the Journey-C detection runs against live state
  instead of an extra IPC round-trip. Reconnect for Atlassian is a
  follow-up — Phase 3 GitLab shipped with a reconnect flow and
  Atlassian's will land in a subsequent ticket once the update IPC
  exists. **Tests.** 7 new Vitest cases in
  `AddAtlassianSourceDialog.test.tsx` cover the
  at-least-one-product gate, URL normalisation preview,
  `http://` rejection, Journey A (shared PAT, both products),
  Journey B (single product), Journey C mode 2 (separate PAT
  with pre-collapsed product selection), and Journey C mode 1
  (reuse with the defensive "missing account id" error path).
  11 new `atlassian-workspace-url.test.ts` cases table-check every
  documented shape. 9 new backend IPC integration tests in
  `ipc/atlassian.rs` exercise the same four journeys plus the
  rejection paths (both-products-disabled, empty-keychain-slot
  in reuse mode, missing `api_token` when not reusing, empty
  email, malformed URL, empty `account_id`). The capabilities
  parity test learns the two new commands; the
  `ipc-commands-parity` Vitest catches any TS-type drift. Ships
  semver:none — the v0.2 milestone is defined in `AGENTS.md` as
  "Atlassian works end-to-end from the desktop UI", and this task
  is the final rung.

- **v0.2 `dayseam-db` — reference-counted shared secrets (DAY-81).**
  Ninth task of the v0.2 Atlassian arc. The v0.1 `sources_delete` IPC
  path dropped the keychain secret unconditionally, which is correct
  for a single-source-per-secret install (every Phase 3 install) and
  becomes silently wrong the moment two `sources` rows share a
  `secret_ref` — the DAY-82 Atlassian shared-PAT flow where one API
  token authenticates both a Jira and a Confluence source. Removing
  one of the two under the old contract would strand the other with
  a DB row that pointed at a now-absent keychain slot, which would
  surface to the user as a silent-empty report on the next sync
  (`build_source_auth` → Reconnect card copy is the only breadcrumb,
  and it's confusing when the *other* source still works).
  **Repo refactor.** `SourceRepo::delete` is now a two-statement
  transaction that (1) reads the deleted row's `secret_ref`,
  (2) `DELETE`s the row, and (3) asks `SELECT 1 FROM sources WHERE
  secret_ref = ? LIMIT 1` whether any other row still points at the
  same keychain slot. Returns `Some(SecretRef)` only when the deleted
  row was the *last* holder of the secret — i.e. the single case where
  the caller can safely drop the keychain entry. Returns `None` when
  the row had no `secret_ref`, when the row did not exist, *or* when
  another source still shares the ref (the shared-PAT case this fix
  guards against). Doing the reference check inside the same
  transaction as the `DELETE` is load-bearing: a pair of racing
  deletes could otherwise each see the other's row before it was
  removed and both think themselves the last reference, firing two
  keychain drops for a slot that no longer has any DB referrers at
  all (harmless) or, worse, missing each other entirely (a lingering
  slot). **Orchestrator wiring.** `sources_delete` in
  `apps/desktop/src-tauri/src/ipc/commands.rs` now consumes the
  `Option<SecretRef>` from the repo and only calls
  `best_effort_delete_secret` when it is `Some`. The pre-DAY-81
  code read the `secret_ref` out via `repo.get(&id)` *before* the
  `DELETE`, which under the new shape would still be correct for
  non-shared secrets but unsound the instant a second source pointed
  at the same slot — the read-then-delete pattern is replaced wholesale
  to route the decision through the DB, which is the only layer that
  can answer the question atomically. **Orphan-secret audit.** A new
  `audit_orphan_secrets` pass runs once at startup right after the
  orchestrator maintenance sweep. For every distinct `secret_ref`
  persisted on the `sources` table it probes the keychain; missing
  slots are logged as `tracing::warn!` lines (no auto-fix in either
  direction — the keyring is the source of truth for "is this secret
  still real?", and the DB row is the source of truth for "is this
  source configured?"). Returns the orphan count for the test to
  assert on; boot never fails because the audit errored, because a
  locked / permission-denied keychain on a dev laptop would otherwise
  brick the app on first launch. The counter-part to
  `SourceRepo::delete`: the transactional refcount is the
  *new-install* half of the "no dangling keychain rows" invariant,
  the boot-time audit is the *existing-install* half (if a user on a
  pre-DAY-81 build stranded a secret, the audit surfaces it on their
  next boot instead of leaving them with a silent-empty report).
  **DB helper.** `SourceRepo::distinct_secret_refs` fans out a
  `SELECT DISTINCT secret_ref FROM sources WHERE secret_ref IS NOT
  NULL` and parses each blob back into a `SecretRef`. Kept on the
  repo because the JSON-blob compare the `delete` txn relies on must
  match the listing path byte-for-byte; the two helpers share the
  same serde contract and a future migration that rewrites the
  column format has to revisit exactly this pair. **Tests.** 4 new
  dayseam-db integration tests
  (`deleting_source_preserves_shared_secret_until_last_reference`,
  `deleting_source_with_no_secret_ref_returns_none`,
  `deleting_nonexistent_source_is_a_no_op`, and
  `distinct_secret_refs_lists_each_shared_slot_once`) + 2 new
  dayseam-desktop startup tests
  (`orphan_secret_detector_logs_but_does_not_delete` and
  `orphan_secret_detector_is_quiet_when_every_ref_resolves`) cover
  the full refcount + audit matrix. The existing
  `sources_round_trip_and_delete_cascades` test was tightened to
  assert the new return shape: sole owner of a `secret_ref` → caller
  receives it back; the FK cascade invariant is unchanged. No IPC
  type surface changed; PR carries `semver:none` — existing consumer
  behaviour is preserved for non-shared installs and strictly
  corrected for shared ones.

- **v0.2 `connector-confluence::walk` — CQL-driven day walker (DAY-80).**
  Eighth task of the v0.2 Atlassian arc. DAY-79 stood up the Confluence
  scaffold with `sync` stubbed as `DayseamError::Unsupported` across
  every `SyncRequest`; this task wires `SyncRequest::Day` onto a real
  walker that GETs `/wiki/rest/api/search` with
  `cql=contributor = currentUser() AND lastModified >= "<start>" AND
  lastModified < "<end>" ORDER BY lastModified DESC` (`Range` / `Since`
  stay `Unsupported` until v0.3's incremental scheduler lands, matching
  the Jira connector). **Walker.** `walk_day` derives a UTC window from
  a `NaiveDate + FixedOffset`, pages the endpoint via
  `connector_atlassian_common::V2CursorPaginator` (the shared
  `_links.next` → `cursor=` extractor), enforces a `MAX_PAGES = 50`
  safety cap, asks for `expand=content.space,content.history,
  content.version,content.body.atlas_doc_format,content.extensions,
  content.ancestors,content.container` on every call (spike §8.5: one
  body format, one normalisation path through
  `connector_atlassian_common::adf_to_plain`), and rebrands the SDK's
  retry-exhausted 429 onto `confluence.walk.rate_limited` while mapping
  other non-2xx via `connector_atlassian_common::map_status`. A missing
  or un-parseable `results` array hard-fails with
  `confluence.walk.upstream_shape_changed` (DAY-71 invariant). An
  identity miss (no `SourceIdentityKind::AtlassianAccountId` registered
  for the source) short-circuits with an empty `WalkOutcome` instead
  of burning a rate-limit call. **Normaliser.** A new
  `normalise.rs` transforms each CQL `results[i]` row into at most one
  `ActivityEvent`: pages with `version.number == 1 && createdBy == self
  && createdDate ∈ window` emit `ActivityKind::ConfluencePageCreated`;
  pages with `version.number > 1 && version.by == self &&
  version.when ∈ window` emit `ActivityKind::ConfluencePageEdited`;
  comments authored by self inside the window emit
  `ActivityKind::ConfluenceComment` with ADF body rendered through
  `adf_to_plain` and `metadata.location ∈ { "inline", "footer" }`
  pulled from `content.extensions.location`. Non-self rows drop
  silently. Links are assembled from `result.url +
  _links.base`, falling back to `content._links.webui`. Entity refs
  seed `confluence_space` + `confluence_page` / `confluence_comment`
  so DAY-78's group-by-space rollup lights up without further work.
  **Rollup.** `rollup.rs` owns the 5-minute rapid-save collapse
  (`RAPID_SAVE_WINDOW_SECONDS = 300`): consecutive
  `ConfluencePageEdited` records for the same `(content_id, author)`
  within the window fold into a single event whose
  `metadata.save_count` records the run length and whose title reads
  "Edited page \"…\" (rolled up from N saves)". The CQL search
  itself returns one row per content-id today (its `contributor =
  currentUser()` query folds versions), so for live data the collapse
  is a no-op; the machinery is still exercised end-to-end by an
  integration test that pre-fabricates a five-version fixture. When
  the walker paginates, every follow-up request also carries
  `expand=...content.body.atlas_doc_format...` — a wiremock assertion
  walks every received request and pins this, because a silent flip
  to `storage` format would leak raw HTML through `adf_to_plain`.
  **Tests.** The plan's Task 8 matrix lands as nine wiremock-driven
  integration tests in `connector-confluence/tests/walk.rs` (created
  vs edited, ADF comment with `@mention` rendering displayName,
  self-filter drops colleague's comments + page-versions, rapid-save
  collapse of five autosaves into one event, pagination via
  `_links.next`, 429 rebrand, shape guard on missing `results`,
  identity-miss early-bail, and the ADF expand assertion) plus 27
  inline unit tests across `walk.rs` / `normalise.rs` / `rollup.rs`.
  The scaffold test that previously pinned "Day is Unsupported"
  flips to assert the new identity-miss short-circuit. `semver:none`
  — additive: no existing consumer path changes shape.

- **v0.2 `connector-confluence` — crate scaffold (DAY-79).**
  Seventh task of the v0.2 Atlassian arc. Parallels the DAY-76 Jira
  scaffold: a new `crates/connectors/connector-confluence` crate that
  registers `SourceKind::Confluence` with the orchestrator, exposes
  `ConfluenceConnector` + `ConfluenceMux` (the per-kind multiplexer
  the DAY-82 Add-Source flow upserts into), ships
  `validate_auth` + `list_identities` on top of
  [`connector_atlassian_common`], and stubs `sync` as
  `DayseamError::Unsupported` across every `SyncRequest` variant —
  the CQL walker lands in DAY-80. `ConfluenceConfig` carries only
  the `workspace_url` (no `email`): a Confluence source pairs with
  a sibling `SourceConfig::Jira` row that already knows the email,
  so the IPC layer rebuilds a `BasicAuth` from the shared secret on
  demand and a single keychain entry can serve both products.
  **Core-types.** `dayseam-core` gains a
  `SourceConfig::Confluence { workspace_url }` variant (additive,
  `semver:none` — previously unreachable because no connector could
  emit one). The matching `ts-rs` bindings regenerate automatically.
  **Orchestrator.** `DefaultRegistryConfig` gains a
  `confluence_sources: Vec<ConfluenceSourceCfg>` field; the default
  registry registers `SourceKind::Confluence` with an empty mux on
  every install (mirroring the Jira "register-empty, upsert-later"
  contract) so the DAY-82 Add-Source dialog can slot a fresh
  Confluence source in without rebuilding the registry. The desktop
  startup backfill passes an empty list until DAY-82 lands the
  dialog. **Tests.** The plan's four scaffold invariants land as
  `connector-confluence/tests/scaffold.rs` (registered kind,
  non-Day unsupported, Arc<dyn SourceConnector> object-safety,
  `ConfluenceMux::upsert`/`remove` round-trip) and
  `connector-confluence/tests/auth.rs` (200/401/403/404 classification
  against the shared `GET /rest/api/3/myself` endpoint, plus the
  shared-identity invariant: the Jira and Confluence `list_identities`
  helpers emit rows with byte-identical `(kind, external_actor_id)`
  from the same `AtlassianAccountInfo`, which is what makes "one
  credential serves both products" real at the walker-filter layer).
  The orchestrator integration helper drops its
  `unreachable!("Confluence lands in DAY-79")` stub in favour of a
  real `SourceConfig::Confluence` row. `semver:none` — the kind +
  scaffold are additive, the walker behaviour flip is DAY-80.

- **v0.2 `dayseam-report` — `group_key_from_event` + cross-source
  enrichment (DAY-78).** Sixth task of the v0.2 Atlassian arc. The
  report engine used to bucket every event by a single `repo_path`
  primitive, so Jira / Confluence events silently collapsed into a
  `/` orphan group — one section header for an entire day's
  Atlassian activity. This task generalises the grouping key and
  adds the cross-source enrichment pipeline that links MRs to Jira
  issues with zero Jira API calls. **`GroupKey` + `GroupKind`** — a
  new `crates/dayseam-report/src/group_key.rs` introduces
  `GroupKind ∈ { Repo, Project, Space }` and `GroupKey { kind, value,
  label }` with a `display()` helper that prefers `label` and falls
  back to `value`; `group_key_from_event` dispatches on
  `ActivityKind` (commits + GitLab MRs → `repo` entity, `Jira*` →
  `jira_project`, `Confluence*` → `confluence_space`) and degrades
  to a synthetic `/` when the canonical entity is absent so render
  never panics on a malformed upstream. **Orphan rollup** —
  `rollup.rs` now synthesises `ArtifactKind::JiraIssue` /
  `ArtifactKind::ConfluencePage` artefacts keyed on the per-event
  `issue_key` / `page_id` (not the project / space) so the evidence
  popover maps each bullet back to the exact issue or page that
  produced it; `merge_duplicate_commit_sets` keeps its DAY-52
  cross-source behaviour for `CommitSet`s and passes the Atlassian
  variants through untouched because issue keys and page ids are
  already globally unique within a workspace. **Render** — Jira /
  Confluence bullets render with a kind-aware prefix
  (`**Cardtronics** (CAR) — CAR-5117: …` for Jira,
  `**Engineering** (ENG) — Edited: …` for Confluence) while the
  existing `**<repo_label>** — <title>` shape for commits stays
  byte-identical (all v0.1 goldens remain green). **Enrichment** —
  a new `crates/dayseam-report/src/enrich.rs` adds two pure passes:
  `extract_ticket_keys` scans each event's title + body for
  `[A-Z]{2,10}-\d+` tokens via a hand-rolled ASCII scanner (no
  `regex` dependency — the report crate is a hot path, the UI
  re-renders on every filter toggle) and attaches `jira_issue`
  `EntityRef` targets, bailing out when >3 candidates surface on a
  single event so a commit message chaining five ticket keys
  attaches nothing rather than the wrong thing;
  `annotate_transition_with_mr` builds a `HashMap<issue_key,
  mr_external_id>` from the MR events in the same day and stamps
  `parent_external_id` on the matching `JiraIssueTransitioned` so a
  verbose-mode bullet can show `(triggered by !321)` next to a
  status change. First-MR-wins on ties, idempotent on re-run.
  **Pipeline** — `crates/dayseam-report/src/pipeline.rs` exposes
  `pipeline(events, mrs)` as the single sequence callers run:
  `dedup_commit_authored → extract_ticket_keys →
  annotate_transition_with_mr → annotate_rolled_into_mr`. The
  orchestrator's `generate.rs` now calls this one function in place
  of the earlier two-pass shape, so every surface (CLI, orchestrator,
  tests) runs the same chain in the same order.
  `crates/dayseam-report/tests/enrich.rs` (new) + two new golden
  fixtures (`dev_eod_jira_two_projects`, `dev_eod_confluence_two_spaces`)
  prove all nine plan invariants end-to-end: existing goldens
  unchanged, Jira events group by project, Confluence events group
  by space, ticket-key extraction attaches / is idempotent / bails
  on noise, transition annotation links MRs / is idempotent, and
  the pipeline composition is stable across re-runs. Zero changes to
  the orchestrator's on-disk shape, the DB schema, or the connector
  traits — this is a pure refactor + additive enrichment. (SemVer:
  `none`.)

- **v0.2 `connector-jira` JQL walker (DAY-77).** Fifth task of the
  combined Jira + Confluence phase: lands the per-day JQL walker
  DAY-76 reserved a seat for, turning the scaffold's `sync` stub into
  a real `SyncRequest::Day` implementation that issues one
  `POST /rest/api/3/search/jql` per day window and normalises the
  returned issues + expanded changelogs into the
  `ActivityKind::Jira*` family. Three new modules: (1) `walk.rs`
  computes day-window bounds via a crate-local `day_bounds_utc`
  (soon to consolidate into `dayseam-core::time`), builds one JQL —
  `(assignee = currentUser() OR comment ~ currentUser() OR reporter =
  currentUser()) AND updated >= "…" AND updated < "…"` — with an
  explicit `fields=summary,status,issuetype,project,priority,labels,updated,created,reporter,comment`
  list and `expand=changelog`, paginates via
  `connector-atlassian-common::JqlTokenPaginator` up to a
  `MAX_PAGES=50` safety cap, and resolves the self `accountId` from
  `ctx.source_identities` (returning an empty outcome + warn log when
  no `SourceIdentityKind::AtlassianAccountId` row is registered —
  DAY-71's known-cause empty invariant, never a silent data loss);
  (2) `normalise.rs` maps one JQL issue + its changelog into zero-or-
  more events — status transitions and assignee changes from
  `changelog.histories[].items[]` (filtered to `author.accountId ==
  self` for transitions, and `items[].to == self` for self-
  assignments), comments from `fields.comment.comments[]` (filtered
  to `author.accountId == self`, body rendered via
  `connector-atlassian-common::adf_to_plain` so `@mentions` surface
  as `@displayName` and never as the raw `accountId`), and issues
  self-reported inside the window from the envelope's `fields.created`
  + `fields.reporter`; unknown `changelog.items[].field` entries
  (custom fields like `cf[10019]`, the `RemoteWorkItemLink` history
  entries Phase-3's spike flagged) drop silently with a `Debug` log
  and bump a `dropped_unknown_changelog` counter so the upstream
  surface stays observable without failing the walk; (3) `rollup.rs`
  implements the rapid-transition collapse the `CAR-5117` spike
  motivated — consecutive self-authored status transitions within
  `RAPID_TRANSITION_WINDOW_SECONDS=60` fold into one
  `JiraIssueTransitioned` event whose `metadata.transition_count`
  records the collapse and whose `from_status` / `to_status` span the
  first and last change (a 6-transition cascade becomes one bullet,
  not six). Error surface stays product-scoped: the SDK's exhausted
  `429` retry budget remaps to `DayseamError::RateLimited { code:
  jira.walk.rate_limited }` (never leaking the SDK's internal
  `http.retry_budget_exhausted`), a JQL response missing the `issues`
  array fails as `jira.walk.upstream_shape_changed` rather than
  silently producing zero events. `JiraConnector` + `JiraMux` now
  carry `local_tz: FixedOffset` (threaded through from
  `DefaultRegistryConfig` in `dayseam-orchestrator::default_registries`)
  so the day-window bounds match the user's local day; `SyncRequest::
  Range` and `SyncRequest::Since` continue to return `Unsupported`
  until v0.3's incremental scheduler lands. 50 total tests across
  the crate (up from 19 in DAY-76): 28 inline unit tests covering
  the rollup window boundary, normaliser field-by-field invariants
  (transition self-filter, assignee-to-self, issue-created window
  bounds, unknown-field drop, shape guards for missing
  `key`/`project`), and walker helpers (JQL construction, request
  body, `day_bounds_utc`, identity resolution); 5 auth tests carried
  over from DAY-76; 9 scaffold tests (one rewritten: `SyncRequest::
  Day` is no longer `Unsupported`, it now degrades to an empty
  `SyncResult` when no identity is configured); and 8 new
  wiremock-driven end-to-end integration tests in `tests/walk.rs`
  pinning the full authn → HTTP → paginate → normalise → rollup
  round-trip (happy path, `CAR-5117` six-transition collapse,
  `KTON-4550` ADF `@mention` rendering privacy, colleague-authored
  comment drop, two-page `nextPageToken` pagination,
  `429 → jira.walk.rate_limited`, missing-`issues` shape guard,
  no-identity early bail without firing a JQL). Ships as
  `semver:none`: the public surface from DAY-76 is preserved (same
  crate exports, same `SourceConnector` impl), and the walker lands
  as a pure replacement of the `sync` body plus additive
  `walk::walk_day` / `rollup::{collapse_rapid_transitions, …}`
  exports — no caller downstream of `connector-jira` can observe a
  breaking change until DAY-82 wires the UI to it. See
  [`docs/plan/2026-04-20-v0.2-atlassian.md`](docs/plan/2026-04-20-v0.2-atlassian.md)
  §Task 5 for the full invariant list.

- **v0.2 `connector-jira` crate scaffold (DAY-76).** Fourth task of
  the combined Jira + Confluence phase: lands the Jira Cloud
  `SourceConnector` shell that DAY-77's per-day JQL walker will plug
  into, without yet shipping the walker itself. Introduces the
  `SourceConfig::Jira { workspace_url, email }` variant in
  `dayseam-core` (with the matching `ts-rs` binding for the desktop
  IPC layer), plus a `connector-jira` crate containing: (1)
  `JiraConfig::from_raw` which parses the raw `workspace_url` into a
  `url::Url` and normalises the trailing slash so every downstream
  `Url::join("rest/api/3/…")` stays inside the intended Cloud site;
  (2) `validate_auth` + `list_identities` thin wrappers over
  `connector-atlassian-common::{discover_cloud, seed_atlassian_identity}`
  that the DAY-82 Add-Source IPC will call from the popover — keeping
  credential-probing and identity-seeding product-scoped even though
  the primitives are shared; (3) `JiraConnector` implementing
  `SourceConnector` with a `GET /rest/api/3/myself` healthcheck that
  authenticates through `ctx.auth` (so any `AuthStrategy` trait
  object, not just `BasicAuth`, can drive the probe) and a `sync`
  method that deliberately returns
  `DayseamError::Unsupported { code: CONNECTOR_UNSUPPORTED_SYNC_REQUEST, … }`
  for every `SyncRequest` variant — the orchestrator can register and
  health-check Jira sources today, while DAY-77 is free to land the
  JQL walker as a pure replacement of the `sync` body without
  touching the crate's public surface; (4) a `JiraMux` multi-source
  dispatcher mirroring `GitlabMux` so the orchestrator registry holds
  one entry per `SourceKind::Jira` and the DAY-82 popover can
  `upsert`/`remove` live without rebuilding the registry. Registered
  in `dayseam-orchestrator::default_registries` behind a
  `DefaultRegistryConfig::jira_sources: Vec<JiraSourceCfg>` field the
  desktop `startup.rs` now wires (empty for now; DAY-82 populates it
  from the source table). 19 unit + wiremock tests (config
  round-trip + trailing-slash idempotency, `validate_auth` 200 /
  401 / 403 / 404 status mapping through `atlassian.*` codes,
  `list_identities` emits exactly one `SourceIdentity` row,
  `JiraMux` object safety + `upsert`/`remove`, `sync` returns
  `Unsupported` for `Day` / `Range` / `Since`). Ships as
  `semver:none`: no public surface lands that a caller could rely
  on for real work yet — the connector exists to receive DAY-77's
  walker. See
  [`docs/plan/2026-04-20-v0.2-atlassian.md`](docs/plan/2026-04-20-v0.2-atlassian.md)
  §Task 4 for the full invariant list.

- **v0.2 `connector-atlassian-common` crate (DAY-75).** Third task of
  the combined Jira + Confluence phase: extracts the five primitives
  the per-product walkers (DAY-77 / DAY-80) will each call into a
  shared, once-and-only-once layer, so a 401 from Jira and a 401 from
  Confluence surface the same way, an `@mention` renders the same
  way in both products' bullets, and a malformed `accountId` is
  caught by the same shape check regardless of which product
  returned it. The five primitives: (1) an ADF → plain-text walker
  (`adf_to_plain`) covering `text`, `mention`, `paragraph`,
  `hardBreak`, `bulletList`, `orderedList`, `heading`, `blockquote`,
  `codeBlock`, `rule`, `inlineCard`, `emoji` — with the spike §12
  privacy rule (mentions emit only `attrs.text`, never `attrs.id` /
  `attrs.email`) baked into the walker and guarded by a dedicated
  test; (2) a `discover_cloud` probe that hits
  `GET /rest/api/3/myself` to validate credentials and surface the
  `accountId + displayName + emailAddress?` triple identity seeding
  needs; (3) `seed_atlassian_identity`, a pure helper that builds the
  `SourceIdentity { kind: AtlassianAccountId, … }` value DAY-82's
  IPC layer will persist — keeping DB writes in the IPC layer (the
  `ensure_gitlab_self_identity` precedent) and this crate
  database-free; (4) two cursor-pagination state machines
  (`JqlTokenPaginator` for Jira v3 `{isLast, nextPageToken}` and
  `V2CursorPaginator` for Confluence v2 `_links.next`); (5) an
  `AtlassianError` taxonomy + `map_status(Product, StatusCode, …)`
  classifier that routes 401/403/404/429/5xx responses to the nine
  `atlassian.*`/`jira.walk.*`/`confluence.walk.*` registry codes
  DAY-73 reserved — this is the CORR-01 classifier DAY-74 deferred
  here. 50 unit tests + 7 wiremock integration tests (5 for
  `discover_cloud` happy-path + 401/403/404/malformed-accountId, 2
  for paginator end-to-end termination). Ships as `semver:minor`:
  the shared crate becomes a public surface a third-party consumer
  could build against, and the ADF walker + cloud-discovery helpers
  are additions no existing caller can have taken a dep on (the two
  product crates land in DAY-76 / DAY-79). See
  [`docs/plan/2026-04-20-v0.2-atlassian.md`](docs/plan/2026-04-20-v0.2-atlassian.md)
  §Task 3 for the full invariant list.

- **v0.2 Atlassian `BasicAuth` strategy (DAY-74).** Second task of the
  combined Jira + Confluence phase: lands the HTTP-Basic auth shape
  the walkers in DAY-77 / DAY-80 will authenticate through, plus the
  matching `AuthDescriptor::Basic` durable-descriptor variant so
  persisted `secret_ref`s round-trip into equivalent strategies on
  app restart. `BasicAuth::atlassian(email, api_token,
  keychain_service, keychain_account)` pre-encodes `email:api_token`
  at construction time and wraps the full `Basic <base64…>` header
  value in a `SecretString` — plain token bytes never outlive the
  constructor stack frame, and `Debug` redacts both the encoded
  header and the plain token via a manual impl. Designed to be
  **agnostic to the shared-PAT / separate-PAT decision**: two
  sources that pass the same `(service, account)` pair collapse
  into a shared-PAT keychain row (the common case — one Atlassian
  token unlocks both Jira and Confluence on the same tenant), and
  two sources that pass different pairs stay independent (the
  separate-service-account or separate-tenant case). The DAY-81
  refcount guard is still the correct delete-path for the shared
  case and degenerates to `refcount == 1` per row in the separate
  case. 12 unit tests (header shape, UTF-8, debug-no-leak,
  descriptor round-trip, shared/separate descriptor equality) + 5
  wiremock integration tests (live header attachment, 401/403
  flowing through as raw responses per the Phase-3 CORR-01
  invariant, shared-handle / separate-handle on-the-wire parity).
  Ships as `semver:none`: no connector consumes `BasicAuth` yet —
  DAY-76 (Jira) and DAY-79 (Confluence) add the `SourceConfig`
  variants that the desktop IPC will hand the strategy to. See
  [`docs/plan/2026-04-20-v0.2-atlassian.md`](docs/plan/2026-04-20-v0.2-atlassian.md)
  §11 row DAY-74 and the amended §Task 2 discussion of the
  shared-vs-separate PAT design.

- **v0.2 Atlassian core types (DAY-73).** First task of the
  combined Jira + Confluence phase: lands the vocabulary the
  walkers in DAY-77 / DAY-80 will speak. Adds `SourceKind::Jira`
  and `SourceKind::Confluence`,
  `SourceIdentityKind::AtlassianAccountId`, seven new
  `ActivityKind` variants (`JiraIssueTransitioned`,
  `JiraIssueCommented`, `JiraIssueAssigned`, `JiraIssueCreated`,
  `ConfluencePageCreated`, `ConfluencePageEdited`,
  `ConfluenceComment`), and two new `ArtifactKind` /
  `ArtifactPayload` variants (`JiraIssue` keyed by
  `(project_key, date)` and `ConfluencePage` keyed by
  `(space_key, date)`). Nine new stable error codes under
  `atlassian.*` / `jira.*` / `confluence.*` are registered in
  `error_codes::ALL` so connector code in later tasks never has
  to invent ad-hoc codes. Ships as `semver:none` because every
  `#[ts(export)]` enum grows additively, the on-disk schema is
  unchanged (kind columns are plain `TEXT`, the required
  `(source_id, external_id)` index already exists in migration
  `0003`), and no connector in this PR emits the new variants
  yet. See
  [`docs/plan/2026-04-20-v0.2-atlassian.md`](docs/plan/2026-04-20-v0.2-atlassian.md)
  §11 row DAY-73.

- **Phase 3 review addendum (DAY-72).** Deeper post-`v0.1.0` hardening
  sweep that runs five new lenses (silent-failure, efficiency,
  dogfood-path, cross-source-consistency, test-quality) on top of the
  formal Phase 3 review battery. Motivated by DAY-71's two dogfood
  bugs ("empty GitLab report" and `**/**` prefix) — both silent
  failures the template-only Phase 3 review had no way to catch. See
  [`docs/review/phase-3-review-addendum.md`](docs/review/phase-3-review-addendum.md)
  for the full 20-finding table and inline-fix narratives. Eight
  High / Medium fixes ship under "Fixed" below (`CORR-addendum-01/02/07/08`,
  `CONS-addendum-04/06`, `PERF-addendum-04/06`); the rest are deferred
  with linked tracking issues.

- **Phase 3 hardening pass (DAY-68, Task 8 capstone).** Closes Phase 3
  against the published `v0.1.0` DMG. See
  [`docs/review/phase-3-review.md`](docs/review/phase-3-review.md) for
  the full 15-finding table and
  [`docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md` "Phase 3
  close-out"](docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md#phase-3-close-out-recorded-2026-04-20)
  for the task-by-task status. Two High-severity correctness bugs
  (CORR-01, CORR-02, documented under "Fixed" below) ship inside the
  same PR because they degrade the very first thing a new v0.1.0 user
  does ("add a GitLab source, click an evidence link"). Folds in the
  three-day Phase 2 dogfood sweep (per the plan's Task 8 intro);
  entries recorded in
  [`docs/dogfood/phase-2-dogfood-notes.md`](docs/dogfood/phase-2-dogfood-notes.md)
  §2. ARC-01 from Phase 2 re-deferred to v0.2 with fresh evidence.

### Fixed

- **`connector-gitlab::project::fetch_project_path` propagates 401 /
  403 from `/api/v4/projects/:id` instead of swallowing them as
  `Ok(None)` (DAY-72 / CORR-addendum-01).** Auth failures fetching the
  `path_with_namespace` for a project previously degraded to a
  synthetic `project-<id>` token and a successful-looking walk, hiding
  exactly the error the `SourceErrorCard` + "Reconnect" flow exists to
  recover. The fetch now returns `DayseamError::Auth` on 401 / 403 via
  `crate::errors::map_status` so the UI gets the specific code
  (`gitlab.auth.invalid_token` / `gitlab.auth.insufficient_scope`).
  Other non-success statuses (404, 5xx) still return `Ok(None)` — the
  synthetic fallback remains correct for "project vanished" / upstream
  blip. Tests: `fetch_project_path_propagates_401_as_auth_error`,
  `fetch_project_path_propagates_403_as_auth_error`. Ships
  `semver:none`.
- **`local_repos.upsert` no longer clobbers user-set `is_private` on
  rescan (DAY-72 / CORR-addendum-02).** Discovery scans populate
  `is_private = false` by default; if a user flagged a repo as private
  via Settings → Sources → Privacy, the next `upsert` would silently
  un-privatise it and subsequent reports would leak commits the user
  had explicitly redacted. The `ON CONFLICT DO UPDATE SET` clause now
  refreshes every other column but leaves `is_private` whatever the
  user made it. Test:
  `local_repos_upsert_preserves_user_set_is_private_on_rescan`. Ships
  `semver:none`.
- **Cross-source dedup unions `actor.email` and `actor.external_id`
  from the loser event instead of discarding them (DAY-72 /
  CORR-addendum-07).** When a `CommitAuthored` appears in both
  `local-git` (carrying `actor.email` from the commit trailer) and
  `gitlab` (carrying `actor.external_id` from the Events API), the
  dedup picks a canonical survivor by source priority. Before this
  fix, whichever field the loser uniquely carried was thrown away.
  `merge_actors` now promotes the loser's `email` / `external_id`
  into the survivor when the survivor's was `None`. `display_name`
  is intentionally left untouched on the winner to avoid flapping
  between sources with different display-name conventions. Test:
  `dedup_unions_actor_identity_fields_across_sources`. Ships
  `semver:none`.
- **`identity_user_ids` warns on malformed `GitLabUserId` rows
  instead of silently dropping them (DAY-72 / CORR-addendum-08).** A
  non-numeric `external_actor_id` used to be filtered out with
  `.parse::<i64>().ok()`, producing an empty result identical in
  shape to "no filter configured" — which the caller interprets as
  "pass every event through", silently leaking every other user's
  events on the instance into the report. The `filter_map` now
  emits a `Warn` log with code `gitlab.identity.malformed_user_id`
  and the offending string so the degrade shows up in
  `reports-debug`. Ships `semver:none`.
- **local-git `repo` `EntityRef` populates `label` for parity with
  `connector-gitlab` (DAY-72 / CONS-addendum-04).** GitLab events
  shipped with `label: Some(basename(path_with_namespace))`;
  local-git events shipped with `label: None`, forcing every
  downstream reader to re-derive the basename from the absolute
  filesystem `external_id`. `build_commit_event` now computes the
  label once and applies it to both the sibling `Link.label` and
  the `repo` `EntityRef.label`. Ships `semver:none`.
- **`render::commit_headline` drops the bolded prefix for synthetic
  `project-<digits>` tokens (DAY-72 / CONS-addendum-06).** The GitLab
  normaliser's docstring promised the render layer would strip the
  prefix for the synthetic `project-42` shape (emitted when
  `/projects/:id` was unreachable); the render layer did not, so
  bullets rendered as `**project-42** — …`. New
  `is_synthetic_project_token` helper + two short-circuit branches
  in `commit_headline` make the contract hold. Test:
  `commit_headline_drops_prefix_for_synthetic_project_token`. Ships
  `semver:none`.
- **`activity_events.list_by_source_date` uses a sargable half-open
  range, restoring the composite index (DAY-72 / PERF-addendum-04).**
  The previous filter
  `WHERE source_id = ? AND substr(occurred_at, 1, 10) = ?` wrapped
  `occurred_at` in a function call, defeating SQLite's ability to
  seek on the `(source_id, occurred_at)` index. The new filter
  `WHERE source_id = ? AND occurred_at >= ? AND occurred_at < ?`
  seeks directly on the index, and the `ORDER BY occurred_at ASC`
  is now satisfied by the index traversal instead of a separate
  sort step. Ships `semver:none`.
- **`annotate_rolled_into_mr` uses a `HashMap` index instead of a
  nested scan (DAY-72 / PERF-addendum-06).** Previous shape was
  O(commits × merge_requests × shas_per_mr); for a heavy day (C=200,
  M=30, S=50) that burned ~300k string comparisons every report
  generation. The helper now builds a `HashMap<&str, &str>`
  (sha → mr_external_id) once — `entry().or_insert` preserves the
  first-MR-wins tiebreak — and does a single O(1) lookup per event.
  New complexity: O(C + ΣS). All six existing `rollup_mr` tests
  pass unchanged. Ships `semver:none`.
- **`HttpClient::send` returns non-retriable 4xx responses raw so the
  GitLab walker can classify them (DAY-68 / Phase 3 Task 8 CORR-01).**
  Before: any non-success HTTP status that was not retriable (401, 403,
  404, …) was collapsed by `crates/connectors-sdk/src/http.rs` into
  `DayseamError::Network { code: "http.transport" }` before the caller
  saw it. This robbed `connector-gitlab::errors::map_status` of the
  status code it needs to route 401 → `gitlab.auth.invalid_token` and
  403 → `gitlab.auth.missing_scope`, which in turn meant the
  `SourceErrorCard` UI fell back to the generic "Reconnect" copy
  instead of the scope-specific copy it already had code for. The SDK
  now returns the raw `reqwest::Response` on these statuses (retry
  logic for 429 + 5xx is unchanged) and the walker does the
  classification, matching the phase-3 design contract. Two new tests
  pin the shape: `http_retry::status_401_and_403_return_response_so_caller_can_classify`
  and `connector-gitlab::sync::walk_day_surfaces_403_as_missing_scope_from_walker_path`.
  Ships `semver:none`.
- **GitLab evidence links resolve to real API endpoints (DAY-68 /
  Phase 3 Task 8 CORR-02).** `compose_links` in
  `connector-gitlab/src/normalise.rs` used to build MR / issue / commit
  URLs as `{base}/-/api/v4/projects/{id}/merge_requests/{iid}` — `/-/`
  is GitLab's UI routing prefix, `api/v4/` is the REST API prefix, and
  no real GitLab endpoint answers a request that mixes them. Every
  evidence link clicked in the v0.1.0 report preview 404-ed. The `/-/`
  segment is now dropped so the link points at a valid REST endpoint
  (`{base}/api/v4/projects/{id}/merge_requests/{iid}`). This serves
  JSON rather than the human-readable page — the richer UI-shaped
  link via `web_url` is tracked as a v0.1.1 follow-up in the plan's
  "What's next" section — but "JSON that loads" beats "404 that
  doesn't" for the first public release. Test:
  `normalise::tests::compose_links_emit_clean_api_paths_without_ui_prefix`.
  Ships `semver:none`.
- **Release assertions glob the actual `.app` main binary instead
  of assuming it matches `productName` (DAY-67).** Post-DAY-66 the
  universal `Dayseam.app` landed at the right path, but the lipo
  and dev-IPC-symbol checks still failed with `Binary not found
  at .../Dayseam.app/Contents/MacOS/Dayseam`. The executable
  *inside* `Dayseam.app/Contents/MacOS/` is named after the cargo
  crate binary name (`dayseam-desktop`), not after `productName`
  (`Dayseam`) — Tauri renames the `.app` directory via
  `productName` but leaves the inner executable at the cargo name
  unless `mainBinaryName` is explicitly set. Both assertion steps
  now `find` the single executable under `Contents/MacOS/` at
  runtime, with an explicit "exactly one" guard so a future
  bundler change that emits helper binaries (e.g. sidecars) trips
  the check instead of picking the wrong one silently. The loop
  uses `while IFS= read -r ... < <(find ...)` because GHA macOS
  runners ship `/bin/bash` 3.2 without `mapfile`. Ships
  `semver:none`; followed by a `workflow_dispatch` run on master
  to publish v0.1.0.
- **Release workflow resolves Cargo's workspace `target/` instead
  of a per-crate path (DAY-66).** Post-DAY-65 the DMG build
  finally produced artefacts — the tauri bundler reported
  `Bundling Dayseam.app (.../target/universal-apple-darwin/.../
  Dayseam.app)` and `Bundling Dayseam_0.1.0_universal.dmg` — but
  `build-dmg.sh` then reported `Tauri bundler did not produce a
  .dmg under apps/desktop/src-tauri/target/...` and exited 1. The
  root cause: in a Cargo workspace, cargo writes all outputs to
  `<workspace_root>/target/`, *not* `<crate_dir>/target/`. Both
  `build-dmg.sh` and the two post-build assertion steps
  (universal-lipo and dev-IPC-symbol checks) hardcoded
  `apps/desktop/src-tauri/target/...`, which was empty. They now
  resolve `target_directory` via `cargo metadata --format-version
  1 --no-deps`, which is the canonical Cargo-native way to find
  the target dir and also respects a `CARGO_TARGET_DIR` override
  — useful in sandboxed CI that redirects builds to an ephemeral
  path. Both assertion steps also gain `shell: bash` so a failing
  `cargo metadata | jq` surfaces at the pipeline, not downstream.
  Ships `semver:none`; followed by a `workflow_dispatch` run on
  master to publish v0.1.0.
- **Release workflow builds the universal DMG again (DAY-65).**
  The first live dispatch of the post-DAY-64 pipeline failed at the
  binary-assertion step with `Binary not found at .../Dayseam.app/
  Contents/MacOS/Dayseam`. Two bugs were in play. First, the build
  step set `CI: "1"` in its `env:` block; the Tauri v2 CLI binds
  the `CI` environment variable to its `--ci` boolean flag and
  rejects anything other than `true`/`false`, so `tauri build`
  exited immediately with `invalid value '1' for '--ci'` before
  emitting any bundle. Second, the step invoked the builder as
  `dmg="$(build-dmg.sh ... | tail -n 1)"` under GitHub Actions's
  default `/bin/bash -e {0}` shell, which runs with `set -e` but
  *not* `pipefail` — so a non-zero exit from `build-dmg.sh` was
  masked by a zero exit from `tail`, the step was reported green,
  and the workflow only surfaced the failure three steps later
  when the missing binary tripped the lipo assertion. The `CI`
  override is removed (GitHub Actions already injects `CI=true`,
  which Tauri and pnpm both accept) and the step now declares
  `shell: bash` so `pipefail` is active and a future build-dmg
  failure surfaces at the build step itself instead of a
  downstream assertion. Ships `semver:none`; followed by a
  `workflow_dispatch` run on master to publish v0.1.0.
- **Release notes extraction prefers `[$TARGET]` over `[Unreleased]`
  (DAY-64).** DAY-63 shipped `scripts/release/extract-release-notes.sh`
  with `[Unreleased]` as the first choice and `[$TARGET]` as the
  fallback — the right order for a normal `semver:patch` bump where
  only `[Unreleased]` exists, but the *wrong* order the moment the
  CHANGELOG contains both a populated `[Unreleased]` (DAY-63's own
  entry, accumulating for v0.1.1) and an explicit `[0.1.0]` block
  from the Task 9 capstone. Dispatching v0.1.0 under that shape
  would publish the DAY-63 infrastructure notes as the v0.1.0
  release body. This change flips the priority so the explicit
  `[$TARGET]` section wins when present, with `[Unreleased]` as
  the fallback for normal patch/minor bumps. Test coverage in
  `scripts/release/test-extract-release-notes.sh` was rewritten to
  prove both shapes and to guard the subheader-only vacuity check
  under the new ordering. Ships `semver:none` because only the
  release tooling behaviour changes.
- **Release workflow unblocks v0.1.0 and all future releases (DAY-63).**
  The v0.1.0 capstone surfaced two latent bugs in `release.yml` the
  moment it ran for real: (1) the CHANGELOG preflight gate only
  looked at `[Unreleased]`, which the Task 9 pattern closes into
  `[$VERSION]` inside the PR itself — producing a spurious
  "CHANGELOG.md [Unreleased] section has no entries; refusing to
  release" failure on the first post-merge run; and (2)
  `bump-version.sh` fell back to the VERSION file at HEAD when no
  `v*` tag existed yet, so on a pre-bumped capstone tree it would
  compute `minor(0.1.0) = 0.2.0` instead of the intended 0.1.0. Both
  bugs are fixed by extracting two thin helpers
  (`scripts/release/resolve-prev-version.sh` and
  `scripts/release/extract-release-notes.sh`) the workflow now
  delegates to, each with a bash unit-test suite wired into a new
  `shell-scripts` CI job. The new preflight prefers the explicit
  `[$TARGET]` section (the Task 9 capstone shape) and falls back
  to `[Unreleased]` for normal semver bumps — see DAY-64 above for
  why this priority is the correct one; the new PREV resolver
  prefers the most recent `v*`
  tag, falls back to VERSION at `HEAD^`, and defaults to `0.0.0`
  for bootstrap. The `has_content` filter was also rewritten to
  avoid a `set -o pipefail` + `grep -q` SIGPIPE race that silently
  misread release bodies larger than a few KB as empty (the real
  v0.1.0 body is 73 KB and tripped this immediately). This PR ships
  under `semver:none` and is followed by a `workflow_dispatch` run
  on master to publish v0.1.0 through the now-fixed pipeline.

## [0.1.0] - 2026-04-20

### Release highlights

Dayseam v0.1.0 is the first user-installable release. It turns a local
macOS checkout of your work day into a reviewed, save-to-markdown
Dev EOD report, with evidence you can click through back to the
original commits and merge requests.

What's in the box:

- **Two source connectors — local git and GitLab** (self-hosted or
  SaaS). Local-git walks every repo under your configured scan roots
  via `libgit2`, filtered by `is_private` so excluded repos never
  surface commit content. GitLab uses a PAT with `read_api`, walks
  the Events API for the day window, and is identity-filtered by
  the numeric `user_id` (not username/email) so username renames
  don't silently break your report.
- **Cross-source de-duplication** — when local-git and GitLab both
  emit a `CommitAuthored` for the same commit SHA, the report keeps
  one bullet with unioned evidence and annotates rolled-up commits
  with `(rolled into !42)` in verbose mode.
- **Per-source error cards with typed error codes and Reconnect.**
  Expired PATs, missing scopes, and schema-drift failures each
  surface with their own `gitlab.*` error code, a plain-language
  explanation, and (for auth errors) a one-click Reconnect button
  that re-opens the add-source dialog pre-seeded for in-place
  rotation.
- **Evidence-clickable report bullets.** Every bullet has a popover
  that fetches the referenced `ActivityEvent`s and renders their
  links as clickable chips gated by a scheme allow-list
  (`http`, `https`, `file`, `vscode`, `obsidian`).
- **Markdown file sink (Obsidian-friendly).** Atomic tempfile +
  rename writes, `<!-- dayseam:start … -->` marker blocks so
  user-authored text around the generated region survives
  regeneration, optional YAML frontmatter, and a
  `WriteReceipt` per destination so the UI can deep-link the
  written files.
- **First-run onboarding** with a four-step setup checklist gate
  (name, source, identity, sink), source chips with edit/delete
  affordances, a native folder picker for sink destinations, and
  dark-mode calendar popovers.
- **Universal macOS `.dmg`** at
  [Releases / v0.1.0](https://github.com/vedanthvdev/dayseam/releases/tag/v0.1.0).
  Single download works on Apple Silicon and Intel Macs (macOS 13+).
  The build is **unsigned** — first-run Gatekeeper bypass is a
  right-click → Open, documented step-by-step in
  [docs/release/UNSIGNED-FIRST-RUN.md](docs/release/UNSIGNED-FIRST-RUN.md).
  Codesigning + notarization are tracked as
  [#59](https://github.com/vedanthvdev/dayseam/issues/59) for v0.1.1
  and will make that whole first-run page obsolete.

Every detail of what landed in Phase 3 is inventoried in the
per-PR entries below; this `[0.1.0]` block also carries the full
Phase 1 and Phase 2 change history because v0.1.0 is the first
tagged release — nothing has shipped to a user before now.

### Added

- **v0.1.0 capstone — `VERSION` 0.0.0 → 0.1.0 (DAY-62).** Phase 3
  Task 9 flips every published version marker in the tree to
  `0.1.0`: the root `VERSION` file, the workspace
  `[workspace.package].version` in `Cargo.toml` (which every member
  crate picks up through `version.workspace = true`), and the
  `"version"` field in
  `apps/desktop/src-tauri/tauri.conf.json`. `Cargo.lock` is
  regenerated so every Dayseam crate resolves to `0.1.0`. This PR
  is the only Phase 3 PR carrying the `semver:minor` label — every
  earlier PR landed under `semver:none` — so merging it runs the
  release workflow (DAY-59) against a real `semver:minor`
  trigger, producing the tagged `v0.1.0` GitHub Release, the
  universal `Dayseam-v0.1.0.dmg`, and its `.sha256` sibling.
  Alongside the version flip, this PR retires a latent bug found
  by the capstone's `cargo check`: every internal path dep in
  every crate's `Cargo.toml` pinned `version = "0.0.0"` as a
  crates.io publishability hint, but `[workspace.package].publish`
  is `false` workspace-wide so those version specifiers were
  noise that would have silently broken every future semver bump
  (a `0.1.1` patch would have left all 34 path-dep sites stuck at
  `0.0.0`, failing resolver validation). The specifiers are
  removed — path-only deps resolve fine through the workspace —
  and the `bump-version.sh` contract is now genuinely complete
  with the three files its six-case test suite already covers.
  README's install section is also tag-pinned to the v0.1.0
  release page so the "Download the DMG" link never 404s between
  tagged releases.

### Changed

- **`graphify` adopt-or-defer decision: deferred to v0.2 (DAY-60).**
  Phase 3 Task 7 resolves the
  [`docs/plan/README.md`](docs/plan/README.md) question about whether
  to adopt [`safishamsi/graphify`](https://github.com/safishamsi/graphify)
  as a committed knowledge-graph index for Dayseam. Scored against
  the plan's three-axis rubric on current `master` (12 crates, ~294
  source files, ~37 800 LOC, ~6 000 lines of hand-curated
  design+plan+review+architecture docs, two committers) the
  evaluation came back 0/3 positive: (a) nothing a combined
  `cargo doc` + `rust-analyzer` + `rg` + the explicit cross-references
  in the design doc cannot already surface; (b) `graphify`'s
  staleness signal is AST-only on the code half but requires an
  LLM-backed `graphify --update` on the docs half, and our canonical
  architecture lives in markdown, so every doc-touching merge would
  stale the graph until the next paid regen; (c) committing
  LLM-summarised artefacts adjacent to PAT fixtures, capability
  allowlists, and `dayseam-secrets` call sites is a net-negative
  review and security surface. The decision, its scoring, and the
  five re-evaluation triggers (workspace size doubling, concurrent
  contributor count > ~3, drift between code and architecture docs,
  first-class LLM-agent orchestration in Dayseam itself, or a
  zero-token alternative shipping) are recorded in the new
  [`docs/decisions/2026-04-20-graphify-deferred.md`](docs/decisions/2026-04-20-graphify-deferred.md);
  the v0.2 re-evaluation is tracked in
  [#61](https://github.com/vedanthvdev/dayseam/issues/61). No
  `graphify-out/` artefacts, `scripts/graphify/`, or freshness CI
  guard land in this PR; contributors remain free to run `graphify`
  against their own checkout, and this decision only blocks
  *committing* generated artefacts into `master`.

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
