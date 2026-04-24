//! Registry of stable machine-readable error codes.
//!
//! Every new code added here is a minor-version bump at worst; renaming or
//! removing a code is a **breaking change** because the frontend and any
//! external tooling (log parsers, support playbooks) key off these
//! literal strings. The `error_codes_registry_snapshot` test in
//! `lib.rs` guards against accidental renames.

// -------- GitLab connector --------------------------------------------------

pub const GITLAB_AUTH_INVALID_TOKEN: &str = "gitlab.auth.invalid_token";
pub const GITLAB_AUTH_MISSING_SCOPE: &str = "gitlab.auth.missing_scope";
pub const GITLAB_URL_DNS: &str = "gitlab.url.dns";
pub const GITLAB_URL_TLS: &str = "gitlab.url.tls";
pub const GITLAB_RATE_LIMITED: &str = "gitlab.rate_limited";
pub const GITLAB_UPSTREAM_5XX: &str = "gitlab.upstream_5xx";
pub const GITLAB_UPSTREAM_SHAPE_CHANGED: &str = "gitlab.upstream_shape_changed";

/// CONS-v0.2-02: a GitLab endpoint returned 404. The most common
/// cause in the dogfood path is a `base_url` that points at a host
/// that doesn't serve GitLab, a project path that no longer exists,
/// or a PAT whose scope can see the group but not a given project
/// (GitLab surfaces a scope miss as 404 rather than 403 for
/// projects inside private groups). Emitted as
/// [`DayseamError::Network`] so the UI surfaces a "check the URL /
/// reconnect" card rather than silently remapping to
/// `gitlab.upstream_5xx` (which would hint at a transient outage
/// and make the user wait for nothing). Mirrors
/// [`ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND`] at the taxonomy level.
pub const GITLAB_RESOURCE_NOT_FOUND: &str = "gitlab.resource_not_found";
/// DAY-89 CONS-v0.2-06. 410 Gone from any GitLab endpoint — the
/// project, MR, or issue has been deleted; retries will never
/// succeed. Surfaced as `DayseamError::Network`; symmetric with
/// [`JIRA_RESOURCE_GONE`] + [`CONFLUENCE_RESOURCE_GONE`] so the
/// cross-connector property test in
/// `crates/connectors/tests/server_error_symmetry.rs` passes.
pub const GITLAB_RESOURCE_GONE: &str = "gitlab.resource_gone";

// -------- Atlassian (Jira + Confluence) connectors -------------------------
//
// Added in DAY-73. Jira and Confluence share one Atlassian Cloud
// credential and one hostname, so anything that can fail on both
// products (auth, cloudId discovery, identity row shape, ADF render)
// lives under the `atlassian.` prefix; anything that only fails inside
// one product's walker lives under `jira.` or `confluence.`. Keeping
// the split at the code level means the UI error-card copy (DAY-82)
// can render shared auth errors once and product-specific ones per
// source.

/// 401 from any Atlassian endpoint — the email + API token combination
/// was refused. Surfaced as `DayseamError::Auth` so the UI can render a
/// Reconnect prompt the same way it does for `gitlab.auth.invalid_token`.
pub const ATLASSIAN_AUTH_INVALID_CREDENTIALS: &str = "atlassian.auth.invalid_credentials";

/// 403 from an Atlassian endpoint whose body indicates a product-scope
/// miss — the token is valid for the workspace but not for the product
/// we're asking about. Surfaced as `DayseamError::Auth` with a
/// product-specific `action_hint`.
pub const ATLASSIAN_AUTH_MISSING_SCOPE: &str = "atlassian.auth.missing_scope";

/// `GET /_edge/tenant_info` (or the equivalent `getAccessibleAtlassian
/// Resources` endpoint under OAuth in a future phase) returned 404 or
/// an empty body for the configured workspace URL. The Basic-auth path
/// doesn't strictly need `cloudId`, but a 404 here is a strong signal
/// the user typed `foo.atlassian.net` when they meant `bar.atlassian.net`.
pub const ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND: &str = "atlassian.cloud.resource_not_found";

/// The `accountId` returned by `GET /rest/api/3/myself` or found in an
/// API response failed the sanity check (non-empty ASCII string,
/// ≤ 128 chars). Emitted as a warn-and-drop in the identity seed path
/// and as an error in the walker's self-filter — mirrors the
/// `gitlab.identity.malformed_user_id` warn log from DAY-72 CORR-08.
pub const ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID: &str = "atlassian.identity.malformed_account_id";

/// ADF walker hit a node type it doesn't know (e.g. a future Atlassian
/// content block like a new `panel` variant). Degrades gracefully to
/// `[unsupported content]` in the rendered body rather than panicking;
/// emitted once per unknown node so the operator can see the shape
/// change in `reports-debug` and add handling in a follow-up PR.
pub const ATLASSIAN_ADF_UNRENDERABLE_NODE: &str = "atlassian.adf.unrenderable_node";

/// A JQL search response (`/rest/api/3/search/jql` or
/// `/rest/api/3/issue/{key}`) contained a `changelog` item with an
/// unknown `field` / `fromString` / `toString` shape, or an issue with
/// a missing required field (e.g. `status.statusCategory.key`). Emitted
/// as `DayseamError::UpstreamChanged` so the orchestrator can degrade
/// the single issue without killing the whole day's walk.
pub const JIRA_WALK_UPSTREAM_SHAPE_CHANGED: &str = "jira.walk.upstream_shape_changed";

/// 429 from any Jira walker endpoint. The SDK's rate-limit retry
/// (carried over from Phase 1 `HttpClient`) handles the backoff;
/// this code fires only when the retry budget is exhausted.
pub const JIRA_WALK_RATE_LIMITED: &str = "jira.walk.rate_limited";

/// 5xx from any Jira endpoint after the SDK's retry budget is
/// exhausted. Surfaced as `DayseamError::Network` (transient), not
/// `UpstreamChanged` — a 500 is the upstream service misbehaving, not
/// Dayseam's walker misreading the response shape. Symmetric with
/// [`GITLAB_UPSTREAM_5XX`] (DAY-89 CONS-v0.2-06).
pub const JIRA_UPSTREAM_5XX: &str = "jira.upstream_5xx";

/// 410 Gone from any Jira endpoint — the issue, project, or
/// attachment has been deleted and the URL will never resolve again.
/// Distinct from 404 (which can be a transient permissions race) so
/// the orchestrator can stop retrying immediately. Surfaced as
/// `DayseamError::Network`. Symmetric with [`GITLAB_RESOURCE_GONE`]
/// (DAY-89 CONS-v0.2-06).
pub const JIRA_RESOURCE_GONE: &str = "jira.resource_gone";

/// A CQL search response (`/wiki/rest/api/content/search`) or a v2
/// content fetch (`/wiki/api/v2/pages/{id}`) returned an unknown
/// content `type` or `extensions` shape. Same degradation semantics
/// as `JIRA_WALK_UPSTREAM_SHAPE_CHANGED`.
pub const CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED: &str = "confluence.walk.upstream_shape_changed";

/// 429 from any Confluence walker endpoint, after the SDK's rate-limit
/// retry budget is exhausted.
pub const CONFLUENCE_WALK_RATE_LIMITED: &str = "confluence.walk.rate_limited";

/// 5xx from any Confluence endpoint after the SDK's retry budget is
/// exhausted. Surfaced as `DayseamError::Network`; see
/// [`JIRA_UPSTREAM_5XX`] for the full rationale. Symmetric across the
/// three connector families (DAY-89 CONS-v0.2-06).
pub const CONFLUENCE_UPSTREAM_5XX: &str = "confluence.upstream_5xx";

/// 410 Gone from any Confluence endpoint — the space, page, or
/// attachment has been deleted. See [`JIRA_RESOURCE_GONE`] for the
/// full rationale.
pub const CONFLUENCE_RESOURCE_GONE: &str = "confluence.resource_gone";

// -------- GitHub connector -------------------------------------------------
//
// Added in DAY-93 (v0.4). Mirrors the GitLab family one-to-one —
// GitHub's REST surface maps to the same failure modes (bad PAT,
// missing token scope, 404 for deleted / private repos, 410 for
// truly deleted records, 429 rate-limited, 5xx transient, unknown
// response shape) and the UI error-card copy reuses the GitLab
// playbooks. The parallel codes are deliberate — the cross-
// connector symmetry property test in
// `crates/connectors/tests/server_error_symmetry.rs` (DAY-89)
// extends to GitHub in DAY-96 with no new test shape, only a new
// row in the symmetry table.

/// 401 from any GitHub endpoint — the PAT was refused (revoked,
/// rotated, or mistyped). Surfaced as `DayseamError::Auth` so the
/// UI renders a Reconnect prompt; mirrors
/// [`GITLAB_AUTH_INVALID_TOKEN`] and
/// [`ATLASSIAN_AUTH_INVALID_CREDENTIALS`].
pub const GITHUB_AUTH_INVALID_CREDENTIALS: &str = "github.auth.invalid_credentials";

/// 403 from a GitHub endpoint whose body indicates the PAT is
/// valid but missing a required scope (`repo`, `read:user`, or
/// `read:org` depending on the request). Distinct from a rate-
/// limit 403 (`X-RateLimit-Remaining: 0`), which routes to
/// [`GITHUB_RATE_LIMITED`]. Surfaced as `DayseamError::Auth`.
pub const GITHUB_AUTH_MISSING_SCOPE: &str = "github.auth.missing_scope";

/// 404 from any GitHub endpoint — the repo / PR / issue has been
/// deleted, was never public to this PAT, or the owner renamed
/// the login and the cached URL no longer resolves. Surfaced as
/// `DayseamError::Network` so the UI offers "check URL / reconnect";
/// mirrors [`GITLAB_RESOURCE_NOT_FOUND`].
pub const GITHUB_RESOURCE_NOT_FOUND: &str = "github.resource_not_found";

/// 429 from any GitHub endpoint, **or** a 403 with
/// `X-RateLimit-Remaining: 0` / the secondary-rate-limit body
/// shape. The SDK's retry honours `Retry-After` and the
/// `X-RateLimit-Reset` header; this code fires only when the
/// retry budget is exhausted. Mirrors [`GITLAB_RATE_LIMITED`] /
/// [`JIRA_WALK_RATE_LIMITED`].
pub const GITHUB_RATE_LIMITED: &str = "github.rate_limited";

/// 5xx from any GitHub endpoint after the SDK's retry budget is
/// exhausted. Surfaced as `DayseamError::Network` (transient),
/// not `UpstreamChanged`; symmetric with the GitLab / Jira /
/// Confluence triplet (DAY-89 CONS-v0.2-06), now a quadruplet.
pub const GITHUB_UPSTREAM_5XX: &str = "github.upstream_5xx";

/// A GitHub response carried an unknown `type` / `action` /
/// `event` shape the walker can't route. Emitted as
/// `DayseamError::UpstreamChanged` so the orchestrator degrades
/// the single event without killing the run; mirrors
/// [`GITLAB_UPSTREAM_SHAPE_CHANGED`].
pub const GITHUB_UPSTREAM_SHAPE_CHANGED: &str = "github.upstream_shape_changed";

/// 410 Gone from any GitHub endpoint — the PR / issue / repo has
/// been hard-deleted; retries will never succeed. Surfaced as
/// `DayseamError::Network`; mirrors [`GITLAB_RESOURCE_GONE`] /
/// [`JIRA_RESOURCE_GONE`] / [`CONFLUENCE_RESOURCE_GONE`] so the
/// DAY-89 server-error-symmetry property test extends without
/// shape change.
pub const GITHUB_RESOURCE_GONE: &str = "github.resource_gone";

/// DAY-122 / C-2. The walker's `MAX_PAGES` cycle-guard tripped —
/// the paginator kept advertising a `rel="next"` Link past the
/// hard cap (30 pages × 100 rows = 3 000 rows, i.e. well past any
/// realistic single-day output for one user). Surfaced as
/// `DayseamError::Internal` so a non-terminating paginator is
/// reported upstream instead of silently truncating the day's
/// data. The pre-C-2 code simply `break`ed out of the loop, which
/// produced a partial day window with no warning in the UI and no
/// breadcrumb in the logs — exactly the silent-failure shape
/// DAY-115 filed a Medium-severity finding against.
pub const GITHUB_PAGINATION_CYCLE_GUARD_TRIPPED: &str =
    "connector.github.pagination.cycle_guard_tripped";

// -------- Local-git connector ----------------------------------------------

pub const LOCAL_GIT_REPO_LOCKED: &str = "local_git.repo_locked";
pub const LOCAL_GIT_REPO_UNREADABLE: &str = "local_git.repo_unreadable";
/// The repository exists but its object database is corrupt and
/// `git2::Repository::open` / `walk` returned an error that does not
/// fit the "locked" or "unreadable" buckets. Surfaced as a
/// `LogEvent::Error` so the run continues across other repos; the
/// orchestrator still produces a report, just without this repo's
/// commits.
pub const LOCAL_GIT_REPO_CORRUPT: &str = "local_git.repo_corrupt";
/// A configured scan root does not exist on disk. Fatal for the
/// connector's `sync` call (the user asked us to scan a path that
/// isn't there); surfaced as `DayseamError::Io` with a
/// `path = Some(root)` so the UI can render the exact missing path.
pub const LOCAL_GIT_REPO_NOT_FOUND: &str = "local_git.repo_not_found";
/// The repo has no author/committer signature configured and every
/// commit we tried to read came back without a usable email. Emitted
/// as a warning log; we still emit the `CommitAuthored` event because
/// "someone committed here today" is still signal, just without
/// identity attribution.
pub const LOCAL_GIT_NO_SIGNATURE: &str = "local_git.no_signature";
/// Discovery hit the configured `max_roots` cap before finishing
/// walking the scan tree. Surfaced as a warning log with the cap's
/// value and the first N roots so the user can either raise the cap
/// or narrow their scan roots.
pub const LOCAL_GIT_TOO_MANY_ROOTS: &str = "local_git.too_many_roots";
/// One or more commits on the selected day were skipped because the
/// author **and** committer emails are both absent from the user's
/// [`crate::types::source::SourceIdentity`] list for this source.
/// Surfaced as a warning log at the end of a sync so the user can
/// see (a) that there was activity, (b) roughly how many commits
/// got filtered, and (c) a hint pointing at the most common cause —
/// merge commits authored through GitHub / GitLab's web UI, which
/// use the platform's `NNNN+user@users.noreply.github.com` alias
/// instead of the user's real email. DAY-52.
pub const LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY: &str = "local_git.commits_filtered_by_identity";

// -------- Sinks -------------------------------------------------------------

pub const SINK_FS_NOT_WRITABLE: &str = "sink.fs.not_writable";
pub const SINK_FS_DESTINATION_MISSING: &str = "sink.fs.destination_missing";
pub const SINK_MALFORMED_MARKER: &str = "sink.malformed_marker";
/// A concurrent `MarkdownFileSink::write` is already in flight for the
/// same target path. The second caller observes the lock sentinel
/// (`<file>.dayseam.lock`) and refuses to write rather than risk
/// interleaving atomic renames. Surfaced as `DayseamError::Io` with the
/// target path so the UI can offer a "retry in a moment" action.
pub const SINK_FS_CONCURRENT_WRITE: &str = "sink.fs.concurrent_write";

// -------- Connector SDK ----------------------------------------------------

/// Run cancelled by the user (e.g. they clicked Cancel in the UI).
pub const RUN_CANCELLED_BY_USER: &str = "run.cancelled.by_user";
/// Run cancelled because a newer run for the same source/date superseded
/// it.
///
/// A `run.cancelled.by_shutdown` code existed in Phase 1 alongside
/// a `SyncRunCancelReason::Shutdown` variant, but Phase 2 Task 8
/// removed both (LCY-01): no orchestrator path ever emits them, so
/// keeping them in the registry implied an unshipped graceful-
/// shutdown contract. A future Phase 3 shutdown implementation can
/// reintroduce the code (and the matching variant on
/// [`crate::types::run::SyncRunCancelReason`]) at that time.
pub const RUN_CANCELLED_BY_SUPERSEDED: &str = "run.cancelled.by_superseded";
/// Connector does not support the requested `SyncRequest` variant — most
/// commonly `Since(Checkpoint)` against a connector with no incremental
/// fetch. The orchestrator catches this and falls back.
pub const CONNECTOR_UNSUPPORTED_SYNC_REQUEST: &str = "connector.unsupported.sync_request";
/// Generic retry budget exhausted after multiple 429 / 5xx attempts.
pub const HTTP_RETRY_BUDGET_EXHAUSTED: &str = "http.retry.budget_exhausted";
/// Transport-level HTTP failure (DNS, TLS, connection reset) for an
/// endpoint that isn't bound to a specific connector.
pub const HTTP_TRANSPORT: &str = "http.transport";

// -------- Orchestrator ------------------------------------------------------

/// A run's terminal `Cancelled` state is reached because a newer run for the
/// same `(person_id, date, template_id)` tuple superseded it. The stream's
/// final `ProgressPhase::Failed` carries this code so the UI can render a
/// distinct "superseded" chip instead of a generic cancel.
pub const ORCHESTRATOR_RUN_SUPERSEDED: &str = "orchestrator.run.superseded";
/// The orchestrator tripped cancellation on a run (user clicked Cancel,
/// app is shutting down, …). Distinct from the connector-level
/// `run.cancelled.*` codes so log-parsers can tell the difference
/// between "a connector observed cancel" and "the orchestrator
/// intentionally cancelled the whole run".
pub const ORCHESTRATOR_RUN_CANCELLED: &str = "orchestrator.run.cancelled";
/// The orchestrator terminated a run in a `Failed` state because one
/// or more fan-out steps returned an error the orchestrator could
/// not recover from (e.g. a connector returned `Internal`). The
/// stream's final `ProgressPhase::Failed` carries this code so the
/// UI renders a "failed" row, not a "cancelled" one. Emitted by
/// `terminate_failed`; distinct from `ORCHESTRATOR_RUN_CANCELLED`
/// which is specifically about cancel/supersede termination.
pub const ORCHESTRATOR_RUN_FAILED: &str = "orchestrator.run.failed";
/// The orchestrator's startup sweep found a `sync_runs` row still in
/// `Running` with `finished_at IS NULL` — evidence of an unclean
/// shutdown. The row is rewritten to `Failed` with this code so the
/// next UI render can surface the recovery explicitly.
pub const INTERNAL_PROCESS_RESTARTED: &str = "internal.process_restarted";
/// A [`crate::DayseamError::Internal`] surfaced from the retention
/// sweep path (pruning `raw_payloads` / `log_entries`). Distinct from
/// [`INTERNAL_PROCESS_RESTARTED`] so a log parser can tell the two
/// orchestrator maintenance paths apart.
pub const ORCHESTRATOR_RETENTION_SWEEP_FAILED: &str = "orchestrator.retention.sweep_failed";
/// `save_report(draft_id, sink_id)` was called with a `draft_id` that
/// does not resolve to a `report_drafts` row. The Task 6 save dialog
/// surfaces this as an inline row in the dialog (SEC-03 / STD-01
/// double-visibility rule).
pub const ORCHESTRATOR_SAVE_DRAFT_NOT_FOUND: &str = "orchestrator.save.draft_not_found";
/// `save_report` was called with a sink whose kind is not registered
/// in the orchestrator's [`crate::SinkKind`]-keyed registry. Usually
/// a feature-flag mismatch between the Tauri layer and the
/// orchestrator build.
pub const ORCHESTRATOR_SINK_NOT_REGISTERED: &str = "orchestrator.save.sink_not_registered";

// -------- IPC layer --------------------------------------------------------

/// An IPC command was given an `id` (source, sink, local repo, draft)
/// that has no row in the database. Returned as
/// `DayseamError::InvalidConfig` with the resource name in the message
/// so the UI can render an actionable "not found" toast and refresh
/// its list.
pub const IPC_SOURCE_NOT_FOUND: &str = "ipc.source.not_found";
pub const IPC_SINK_NOT_FOUND: &str = "ipc.sink.not_found";
pub const IPC_LOCAL_REPO_NOT_FOUND: &str = "ipc.local_repo.not_found";
pub const IPC_REPORT_DRAFT_NOT_FOUND: &str = "ipc.report_draft.not_found";
/// `sources_update` was called with a `config` whose `kind` does not
/// match the persisted source's `kind`. Surfaced so the UI never
/// silently widens a `LocalGit` source into a `GitLab` source via a
/// patch.
pub const IPC_SOURCE_CONFIG_KIND_MISMATCH: &str = "ipc.source.config_kind_mismatch";

/// `sources_add` or `sources_update` was called with a `LocalGit`
/// config whose `scan_roots` overlap (are equal to, or an ancestor
/// or descendant of) the scan roots of another already-persisted
/// `LocalGit` source. The guard is prefix-based on canonicalised
/// paths — so `~/code` overlapping `~/code/foo` is rejected, but two
/// siblings under the same parent (`~/code/alpha` vs `~/code/beta`)
/// are not.
///
/// Introduced in DAY-106 (F-8 / #113) to prevent the pre-0.5.1
/// failure mode where the `local_repos` table — primary-keyed on
/// `path` alone — let two overlapping LocalGit sources
/// ping-pong row ownership on every rescan, producing flickering
/// sidebar counts and a `reconcile_for_source` that could no longer
/// prune its own stale rows once another source had claimed them.
/// The structurally-correct fix is a `(source_id, path)` composite
/// key (still tracked as a deferred follow-up on #113), but
/// disallowing overlap at source-add time is the cheaper, reversible
/// shape that matches every current Dayseam user's mental model —
/// sources are scopes, and scopes don't overlap.
pub const IPC_SOURCE_SCAN_ROOT_OVERLAP: &str = "ipc.source.scan_root_overlap";

/// `sources_add` / `sources_update` was called for a GitLab source
/// without a PAT, or `sources_healthcheck` / `report_generate` loaded a
/// GitLab source row whose `secret_ref` points at an empty keychain
/// slot. The UI surfaces this as a "Reconnect" prompt rather than as a
/// generic network error because the fix is always the same: paste a
/// fresh PAT. Introduced in DAY-70 after we discovered that the entire
/// GitLab happy path was silently running unauthenticated on self-
/// hosted hosts, returning HTTP 200 with an empty events array.
pub const IPC_GITLAB_PAT_MISSING: &str = "ipc.gitlab.pat_missing";

/// Writing the GitLab PAT to the OS keychain failed. The PAT did not
/// persist, so the subsequent `report_generate` would silently fall
/// back to unauthenticated requests. We abort `sources_add` /
/// `sources_update` so the caller can retry; no half-written source
/// survives.
pub const IPC_GITLAB_KEYCHAIN_WRITE_FAILED: &str = "ipc.gitlab.keychain_write_failed";

/// Reading the GitLab PAT out of the OS keychain failed at
/// `report_generate` / `sources_healthcheck` time. Rendered as an
/// auth-style error so the UI offers Reconnect; the user re-pasting
/// the PAT overwrites whatever stale/corrupt Keychain row is to blame.
pub const IPC_GITLAB_KEYCHAIN_READ_FAILED: &str = "ipc.gitlab.keychain_read_failed";

/// `atlassian_sources_add` was called with both `enable_jira` and
/// `enable_confluence` set to `false`. The Atlassian dialog also
/// blocks the Submit button in this state (UI-level invariant 1 in
/// DAY-82), but the IPC layer enforces the same rule so a bespoke
/// caller cannot round-trip an empty intent into the database.
pub const IPC_ATLASSIAN_NO_PRODUCT_SELECTED: &str = "ipc.atlassian.no_product_selected";

/// `atlassian_sources_add` / `atlassian_validate_credentials` was
/// called with an email or API token that is empty or whitespace-only.
/// Mirrors `IPC_GITLAB_PAT_MISSING`.
pub const IPC_ATLASSIAN_CREDENTIALS_MISSING: &str = "ipc.atlassian.credentials_missing";

/// The `workspace_url` argument to an Atlassian IPC command failed
/// to parse as an absolute `https://` URL. The dialog normalises the
/// user's input client-side before calling IPC (DAY-82 invariant 2),
/// so this code fires only for hand-crafted callers or a stale
/// in-memory value; either way the request is rejected before any
/// network call.
pub const IPC_ATLASSIAN_INVALID_WORKSPACE_URL: &str = "ipc.atlassian.invalid_workspace_url";

/// Writing the Atlassian API token to the OS keychain failed. Same
/// rollback + retry semantics as [`IPC_GITLAB_KEYCHAIN_WRITE_FAILED`]:
/// `atlassian_sources_add` does not persist a partial source row
/// when this fires.
pub const IPC_ATLASSIAN_KEYCHAIN_WRITE_FAILED: &str = "ipc.atlassian.keychain_write_failed";

/// `atlassian_sources_add` was called with `reuse_secret_ref =
/// Some(...)` but the supplied slot is empty in the OS keychain.
/// This usually means a prior source that owned the slot was
/// deleted (DAY-81's refcount dropped the row) and the dialog held
/// stale state; the frontend should fall back to the full "enter a
/// new token" path.
pub const IPC_ATLASSIAN_REUSE_SECRET_MISSING: &str = "ipc.atlassian.reuse_secret_missing";

/// `github_validate_credentials` / `github_sources_add` /
/// `github_sources_reconnect` was called with a PAT that is empty
/// or whitespace-only. Mirrors `IPC_GITLAB_PAT_MISSING`. The
/// frontend dialog's Validate button also blocks this state (DAY-99
/// invariant 1); the IPC guard exists so a hand-crafted caller
/// cannot round-trip an empty-PAT source into the keychain.
pub const IPC_GITHUB_PAT_MISSING: &str = "ipc.github.pat_missing";

/// The `api_base_url` argument to a GitHub IPC command failed to
/// parse as an absolute `https://` URL. The dialog normalises the
/// user's input client-side before firing IPC (DAY-99 invariant 2),
/// so this code fires only for hand-crafted callers or a stale
/// in-memory value; either way the request is rejected before any
/// network call.
pub const IPC_GITHUB_INVALID_API_BASE_URL: &str = "ipc.github.invalid_api_base_url";

/// Writing the GitHub PAT to the OS keychain failed. Same
/// rollback-and-retry semantics as
/// [`IPC_GITLAB_KEYCHAIN_WRITE_FAILED`]: `github_sources_add` and
/// `github_sources_reconnect` do not persist a partial source row
/// when this fires.
pub const IPC_GITHUB_KEYCHAIN_WRITE_FAILED: &str = "ipc.github.keychain_write_failed";

/// `sinks_add` was called with a `config` whose body fails the IPC
/// layer's structural check (e.g. a `MarkdownFile` sink with an
/// empty `dest_dirs` list, a non-absolute path, or a path with `..`
/// traversal segments). The guard exists so a buggy or hostile
/// frontend cannot wedge a sink that would later refuse every
/// `report_save`.
pub const IPC_SINK_INVALID_CONFIG: &str = "ipc.sink.invalid_config";

/// `persons_update_self` was called with an empty or whitespace-only
/// `display_name`. The command rejects the update before it touches
/// the DB so the onboarding dialog can re-prompt without a round-trip
/// to SQLite.
pub const IPC_INVALID_DISPLAY_NAME: &str = "ipc.persons.invalid_display_name";

/// `shell_open` was asked to open a URL whose scheme is not in the
/// explicit allow-list (`file`, `http`, `https`, `vscode`, `obsidian`).
/// The guard exists so a malicious or buggy evidence row cannot coax
/// Dayseam into handing a `javascript:` URL to the OS.
pub const IPC_SHELL_URL_DISALLOWED: &str = "ipc.shell.url_disallowed";
/// `shell_open` could not parse the provided string as a URL at all.
/// Distinct from `URL_DISALLOWED` so the UI can tell "looks broken"
/// apart from "looks malicious".
pub const IPC_SHELL_URL_INVALID: &str = "ipc.shell.url_invalid";
/// `shell_open` handed the URL to the OS and the OS refused (missing
/// handler, sandbox denial, etc.). Surfaced as `Internal` so it
/// bubbles into a toast without retry.
pub const IPC_SHELL_OPEN_FAILED: &str = "ipc.shell.open_failed";

// -------- Database ---------------------------------------------------------

/// `sqlx::migrate!` failed to apply a pending migration. Always fatal
/// at startup; the app bails out rather than run against a half-
/// migrated schema.
pub const DB_SCHEMA_MIGRATION_FAILED: &str = "db.schema.migration_failed";

/// All known codes in declaration order. The snapshot test iterates over
/// this slice so a missing entry means either the slice wasn't updated or
/// a constant was renamed — in either case review needs to happen.
pub const ALL: &[&str] = &[
    GITLAB_AUTH_INVALID_TOKEN,
    GITLAB_AUTH_MISSING_SCOPE,
    GITLAB_URL_DNS,
    GITLAB_URL_TLS,
    GITLAB_RATE_LIMITED,
    GITLAB_UPSTREAM_5XX,
    GITLAB_UPSTREAM_SHAPE_CHANGED,
    GITLAB_RESOURCE_NOT_FOUND,
    GITLAB_RESOURCE_GONE,
    ATLASSIAN_AUTH_INVALID_CREDENTIALS,
    ATLASSIAN_AUTH_MISSING_SCOPE,
    ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND,
    ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID,
    ATLASSIAN_ADF_UNRENDERABLE_NODE,
    JIRA_WALK_UPSTREAM_SHAPE_CHANGED,
    JIRA_WALK_RATE_LIMITED,
    JIRA_UPSTREAM_5XX,
    JIRA_RESOURCE_GONE,
    CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED,
    CONFLUENCE_WALK_RATE_LIMITED,
    CONFLUENCE_UPSTREAM_5XX,
    CONFLUENCE_RESOURCE_GONE,
    GITHUB_AUTH_INVALID_CREDENTIALS,
    GITHUB_AUTH_MISSING_SCOPE,
    GITHUB_RESOURCE_NOT_FOUND,
    GITHUB_RATE_LIMITED,
    GITHUB_UPSTREAM_5XX,
    GITHUB_UPSTREAM_SHAPE_CHANGED,
    GITHUB_RESOURCE_GONE,
    GITHUB_PAGINATION_CYCLE_GUARD_TRIPPED,
    LOCAL_GIT_REPO_LOCKED,
    LOCAL_GIT_REPO_UNREADABLE,
    LOCAL_GIT_REPO_CORRUPT,
    LOCAL_GIT_REPO_NOT_FOUND,
    LOCAL_GIT_NO_SIGNATURE,
    LOCAL_GIT_TOO_MANY_ROOTS,
    LOCAL_GIT_COMMITS_FILTERED_BY_IDENTITY,
    SINK_FS_NOT_WRITABLE,
    SINK_FS_DESTINATION_MISSING,
    SINK_MALFORMED_MARKER,
    SINK_FS_CONCURRENT_WRITE,
    RUN_CANCELLED_BY_USER,
    RUN_CANCELLED_BY_SUPERSEDED,
    CONNECTOR_UNSUPPORTED_SYNC_REQUEST,
    HTTP_RETRY_BUDGET_EXHAUSTED,
    HTTP_TRANSPORT,
    ORCHESTRATOR_RUN_SUPERSEDED,
    ORCHESTRATOR_RUN_CANCELLED,
    ORCHESTRATOR_RUN_FAILED,
    INTERNAL_PROCESS_RESTARTED,
    ORCHESTRATOR_RETENTION_SWEEP_FAILED,
    ORCHESTRATOR_SAVE_DRAFT_NOT_FOUND,
    ORCHESTRATOR_SINK_NOT_REGISTERED,
    IPC_SOURCE_NOT_FOUND,
    IPC_SINK_NOT_FOUND,
    IPC_LOCAL_REPO_NOT_FOUND,
    IPC_REPORT_DRAFT_NOT_FOUND,
    IPC_SOURCE_CONFIG_KIND_MISMATCH,
    IPC_SINK_INVALID_CONFIG,
    IPC_INVALID_DISPLAY_NAME,
    IPC_SHELL_URL_DISALLOWED,
    IPC_SHELL_URL_INVALID,
    IPC_SHELL_OPEN_FAILED,
    DB_SCHEMA_MIGRATION_FAILED,
];

#[cfg(test)]
mod tests {
    use super::ALL;
    use std::collections::HashSet;

    #[test]
    fn registry_has_no_duplicates() {
        let set: HashSet<_> = ALL.iter().collect();
        assert_eq!(
            set.len(),
            ALL.len(),
            "duplicate error code in error_codes::ALL"
        );
    }

    #[test]
    fn registry_snapshot() {
        insta::assert_yaml_snapshot!(ALL);
    }
}
