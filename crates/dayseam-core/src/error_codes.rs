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
///
/// Kept as the catch-all fallback. The four sub-codes below
/// (`HTTP_TRANSPORT_DNS` / `_TLS` / `_CONNECT` / `_TIMEOUT`) are
/// preferred when `HttpClient::send` can classify the underlying
/// `reqwest::Error` with confidence; `HTTP_TRANSPORT` survives for
/// unknown or builder-level failures where no narrower label applies.
/// Log parsers can continue to grep `http.transport` as a prefix and
/// still match every transport-family code.
pub const HTTP_TRANSPORT: &str = "http.transport";
/// Name-resolution failed for the target host. Most often surfaces when
/// a private GitLab / self-hosted GitHub hostname is only resolvable
/// from a corporate VPN that isn't currently connected — the symptom
/// user-side is a sync that worked yesterday and now fails with no
/// apparent change. The user-facing message includes the host so the
/// UI can render "couldn't resolve `git.example.com`" instead of the
/// generic "something went wrong" card.
pub const HTTP_TRANSPORT_DNS: &str = "http.transport.dns";
/// TLS / SSL handshake failed for the target host. Typically a
/// corporate MITM proxy re-signing traffic with a CA Dayseam doesn't
/// trust, or a genuinely expired / mismatched upstream certificate.
/// Distinct from `HTTP_TRANSPORT_DNS` so the UI copy can point at
/// "check your security software / certificate chain" rather than
/// "check your URL / network".
pub const HTTP_TRANSPORT_TLS: &str = "http.transport.tls";
/// TCP connect to the resolved address failed (`ECONNREFUSED`,
/// `EHOSTUNREACH`, route-to-host failure). Distinct from DNS because
/// the name *did* resolve — the machine, router, or firewall between
/// Dayseam and the target is what's denying the connection.
pub const HTTP_TRANSPORT_CONNECT: &str = "http.transport.connect";
/// Request timed out after `reqwest`'s configured connect or overall
/// timeout. Distinct from `HTTP_TRANSPORT_CONNECT` because a timeout
/// typically means the connection got far enough to stall rather than
/// being refused outright — a different root cause (upstream load,
/// captive portal, half-open NAT) from a clean connection refusal.
pub const HTTP_TRANSPORT_TIMEOUT: &str = "http.transport.timeout";

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

/// DAY-203. `outlook_validate_credentials` / `outlook_sources_add`
/// was called with a session id the in-memory OAuth registry has
/// no record of — either it was cancelled already, completed +
/// consumed by a prior `outlook_sources_add` call, or never
/// existed (a stale id carried in from a crashed dialog). Distinct
/// from [`OAUTH_LOGIN_SESSION_NOT_FOUND`] so the UI can route to
/// Outlook-specific "let's start over" copy instead of the generic
/// sign-in timeout card. Surfaced as
/// [`DayseamError::InvalidConfig`].
pub const IPC_OUTLOOK_SESSION_NOT_FOUND: &str = "ipc.outlook.session_not_found";

/// DAY-203. `outlook_validate_credentials` was called while the
/// session is still `Pending` — the user clicked "Add source"
/// before the IdP loopback callback arrived. The UI should only
/// enable Validate/Add once `oauth_session_status` reports
/// `Completed`, so this code is defensive against a dialog bug or
/// a bespoke caller. Surfaced as [`DayseamError::InvalidConfig`].
pub const IPC_OUTLOOK_SESSION_NOT_READY: &str = "ipc.outlook.session_not_ready";

/// DAY-203. Writing the Outlook access or refresh token to the OS
/// keychain failed. Same rollback-and-retry semantics as
/// [`IPC_GITLAB_KEYCHAIN_WRITE_FAILED`]: `outlook_sources_add`
/// tears down the partial keychain row and the partial DB row (if
/// any) before returning, so a retry starts from a clean slate.
pub const IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED: &str = "ipc.outlook.keychain_write_failed";

/// DAY-203. We could not read a non-empty `tid` claim out of the
/// access token minted by the PKCE token-exchange — the JWT did
/// not have three segments, the payload segment was not valid
/// base64url, the decoded bytes were not JSON, or the JSON had no
/// `tid` field. Every failure mode collapses here because the
/// user-facing remediation is the same: retry the sign-in. See
/// [`apps/desktop/src-tauri/src/ipc/outlook_jwt.rs`] for the
/// detailed rationale. Surfaced as
/// [`DayseamError::InvalidConfig`].
pub const IPC_OUTLOOK_TENANT_UNRESOLVED: &str = "ipc.outlook.tenant_unresolved";

/// DAY-203. `outlook_sources_add` was called for a
/// `(tenant_id, user_principal_name)` tuple that already has a
/// row in the `sources` table. The UI surfaces this as "This
/// calendar is already connected" with a link to the existing
/// source card rather than silently producing a duplicate that
/// would race its twin on every sync. Surfaced as
/// [`DayseamError::InvalidConfig`].
pub const IPC_OUTLOOK_SOURCE_ALREADY_EXISTS: &str = "ipc.outlook.source_already_exists";

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

// -------- OAuth 2.0 (cross-connector) --------------------------------------
//
// Added in DAY-200 alongside `AuthDescriptor::OAuth` + `OAuthAuth`.
// These codes sit outside any individual connector's prefix because
// the strategy lives in `connectors-sdk` and will be shared across
// future OAuth-using connectors (Outlook v0.9, Slack/Teams/Linear
// post-v0.9). Connector-specific OAuth failures (admin-consent
// rejection, tenant-policy scope downgrade, etc.) still get their
// own `<connector>.oauth.*` codes so the UI can render
// provider-specific help text — these cross-connector codes cover
// the SDK-level invariants that have nothing to do with which IdP
// is on the other end of the wire.

/// Reserved for the narrow post-DAY-201 case where an
/// `OAuthAuth` handle is observed with an expired access token and
/// **no refresh path can fire** — either the persister hasn't been
/// wired yet on a freshly loaded strategy, or a test has
/// deliberately constructed an `OAuthAuth` without a refresh
/// endpoint to exercise the terminal branch. In the normal DAY-201
/// flow, `OAuthAuth::authenticate` first calls
/// `refresh_if_expired`, which either succeeds (no error emitted)
/// or fails with [`OAUTH_REFRESH_REJECTED`] (terminal re-auth
/// condition). This code stays in the registry because the
/// `insta` snapshot anchors a stable contract for the UI copy —
/// removing it later would be a breaking change for any consumer
/// that had code paths bound to it.
pub const OAUTH_TOKEN_EXPIRED: &str = "oauth.token_expired";

/// The SDK's [`OAuthAuth::new`] was handed a descriptor that is not
/// the [`AuthDescriptor::OAuth`] variant — a defensive guard that
/// catches orchestrator bugs where a PAT or Basic descriptor is
/// accidentally routed through the OAuth constructor on a round trip
/// through storage. Surfaced as [`DayseamError::InvalidConfig`]
/// because it's a programming error, not a user-facing failure.
pub const OAUTH_DESCRIPTOR_MISMATCH: &str = "oauth.descriptor_mismatch";

/// The OAuth token endpoint rejected our refresh-token request with
/// `invalid_grant` (or an equivalent 4xx that IdP's grammar says is
/// terminal). The refresh token is dead — the user consented a
/// while ago, has since revoked the app, rotated their password, or
/// the tenant admin removed the consent. DAY-201 surfaces this as
/// [`DayseamError::Auth`] with `retryable: false` and an
/// `action_hint` pointing at the Reconnect flow. Distinct from
/// [`OAUTH_TOKEN_EXPIRED`]: that one means "the access token expired
/// but we still have a refresh we just haven't spent yet"; this one
/// means "we tried to spend the refresh and the IdP said no".
pub const OAUTH_REFRESH_REJECTED: &str = "oauth.refresh_rejected";

/// A refresh response came back `200 OK` but the `scope` field in
/// the JSON was a *subset* of the scopes we originally consented to.
/// Microsoft Graph and other IdPs do this when a tenant admin
/// tightens the app's permission grant between consent and refresh:
/// the refresh succeeds, but the new access token can no longer do
/// what the connector was built around. DAY-201 does **not** treat
/// this as fatal at refresh time (we still hand back the narrower
/// token so an in-flight sync can finish what it was doing);
/// instead the orchestrator compares the granted-scope set with the
/// connector's declared requirement and, if the intersection is
/// short, surfaces a non-fatal reconnect nudge to the user on the
/// next sync boundary. The code is defined here so the refresh path
/// and the orchestrator can bind against the same string.
pub const OAUTH_SCOPE_DOWNGRADED: &str = "oauth.scope_downgraded";

// -------- OAuth 2.0 login (PKCE + loopback redirect) -----------------------
//
// Added in DAY-201 PR #2 alongside the Tauri-shell `oauth_begin_login`
// command. These codes describe failures of the *login* ceremony —
// the browser round-trip that mints the first `TokenPair` — and are
// distinct from the above refresh/exchange codes, which apply once
// the user has already consented at least once. The login flow lives
// in `apps/desktop/src-tauri/src/ipc/oauth.rs`; its unit tests bind
// against the same constants so a rename here breaks them.

/// The authorization callback did not arrive on the loopback listener
/// before the overall login timeout elapsed (default five minutes in
/// DAY-201 PR #2). Common causes: the user walked away mid-consent,
/// the IdP is slow, or an extension in the user's default browser is
/// rewriting redirect URIs and the callback never hits `127.0.0.1`.
/// Surfaced as [`DayseamError::Auth`] with a "try again" action hint
/// rather than a transient-network retry, because retrying requires
/// a fresh PKCE pair.
pub const OAUTH_LOGIN_TIMEOUT: &str = "oauth.login.timeout";

/// The `state` parameter on the authorization callback did not match
/// the opaque value we embedded in the outgoing authorization URL.
/// In practice this means either (a) a stale tab from a prior login
/// attempt raced the current one, or (b) a CSRF attempt. Either way
/// the callback is discarded and the session is marked failed so the
/// UI can prompt a fresh login rather than silently accept a code
/// that wasn't bound to this flow.
pub const OAUTH_LOGIN_STATE_MISMATCH: &str = "oauth.login.state_mismatch";

/// The loopback listener failed to bind a `127.0.0.1` port — usually
/// because a corporate firewall or endpoint-security agent blocks
/// inbound localhost listeners from non-allowlisted processes.
/// Surfaced as [`DayseamError::Internal`] since it's a local-machine
/// configuration failure, not an auth or network one.
pub const OAUTH_LOGIN_LOOPBACK_BIND_FAILED: &str = "oauth.login.loopback_bind_failed";

/// The user cancelled the login from the Dayseam UI (e.g. clicked
/// "Cancel" in the connecting modal), or the IdP's consent page
/// returned an explicit `error=access_denied`. Both paths collapse to
/// one code because the remediation — "start over" — is identical.
pub const OAUTH_LOGIN_USER_CANCELLED: &str = "oauth.login.user_cancelled";

/// `opener::open(authorization_url)` (or its platform equivalent)
/// returned an error — most commonly on a headless CI worker or a
/// Linux session without a default browser registered. Surfaced so
/// the UI can tell the user "we could not open your browser; copy
/// this URL manually" instead of silently stalling on the listener.
pub const OAUTH_LOGIN_BROWSER_OPEN_FAILED: &str = "oauth.login.browser_open_failed";

/// A `oauth_session_status` / `oauth_cancel_login` call carried a
/// session id the in-memory registry has no record of. The session
/// either completed already (and was reaped) or never existed.
/// Surfaced as [`DayseamError::InvalidConfig`] so the UI drops any
/// stale references and falls back to offering a fresh login.
pub const OAUTH_LOGIN_SESSION_NOT_FOUND: &str = "oauth.login.session_not_found";

/// The IdP's authorization callback carried `error=<…>` rather than
/// a `code`. Captures every OAuth 2.0 error response that isn't a
/// clean user-cancel (`access_denied`, `invalid_request`,
/// `server_error`, `temporarily_unavailable`, …) so the UI can show
/// the exact `error`/`error_description` pair rather than a generic
/// failure toast.
pub const OAUTH_LOGIN_AUTHORIZATION_ERROR: &str = "oauth.login.authorization_error";

/// `oauth_begin_login` was called with a `provider_id` that no
/// entry in the SDK's provider registry recognises. Defensive guard
/// against a typo'd invocation from a bespoke caller; the frontend's
/// dropdown only ever passes known-good ids.
pub const OAUTH_LOGIN_PROVIDER_UNKNOWN: &str = "oauth.login.provider_unknown";

/// `oauth_begin_login` was called for a provider whose `client_id`
/// is still the unregistered placeholder — the ship-with-placeholder
/// path documented in `docs/setup/azure-app-registration.md`. The
/// IPC refuses to open a browser window that would only ever hit an
/// IdP error page; the UI points the user at the registration
/// checklist instead.
pub const OAUTH_LOGIN_NOT_CONFIGURED: &str = "oauth.login.not_configured";

// -------- Outlook (Microsoft 365 / Graph) ----------------------------------
//
// Added in DAY-202. Mirrors the GitHub prefix family — `outlook.auth.*`
// for credential failures, `outlook.rate_limited` for 429s,
// `outlook.upstream_5xx` for transient server failures,
// `outlook.upstream_shape_changed` for contract drift (e.g. Graph
// returning a new event schema we can't normalise). The SDK-level
// `oauth.*` codes above cover the token-refresh failure modes;
// anything connector-specific (tenant admin pulled the app's
// permission grant, the user's mailbox is a shared mailbox Graph
// won't expand attendees for, etc.) lives here.
//
// The IPC-adjacent `ipc.outlook.*` codes (shape checks we perform
// before touching the network or the database) land in DAY-203
// alongside the `outlook_validate_credentials` /
// `outlook_sources_add` commands that raise them.

/// Microsoft Graph returned `401 Unauthorized` on a request a valid
/// access token should have satisfied. DAY-201 gave us automatic
/// refresh for expired access tokens; this code means either the
/// refresh also failed (in which case the SDK surfaces
/// [`OAUTH_REFRESH_REJECTED`] first and this code never fires), or
/// the tenant admin revoked the app between `authenticate` and the
/// next Graph call. Surfaced as [`DayseamError::Auth`] with an
/// `action_hint` pointing at the Reconnect flow.
pub const OUTLOOK_AUTH_INVALID_CREDENTIALS: &str = "outlook.auth.invalid_credentials";

/// Graph returned `403 Forbidden` on a request whose token carries
/// the scopes the connector declared, but the tenant's conditional-
/// access / app-role policy denies the specific endpoint (commonly
/// `Calendars.Read` narrowed to `Calendars.Read.Shared` by a tenant
/// admin). The refresh token is still good; the user needs a
/// tenant-admin action to widen the grant. Surfaced as
/// [`DayseamError::Auth`] with a distinct action hint so the UI
/// can tell the user "contact your IT admin" rather than the
/// generic Reconnect chip.
pub const OUTLOOK_AUTH_MISSING_SCOPE: &str = "outlook.auth.missing_scope";

/// DAY-203. Azure AD rejected the sign-in because the tenant is
/// configured to require admin-level consent for the app, and no
/// admin has granted it yet. Surfaces on the `/authorize` callback
/// as `error=consent_required` / `error=interaction_required` and
/// is distinct from [`OUTLOOK_AUTH_MISSING_SCOPE`]: a missing scope
/// means the user got a token the tenant then downgrades; consent
/// required means the user never got any token at all. Surfaced as
/// [`DayseamError::Auth`] so the dialog routes to the "contact
/// your IT admin with this admin-consent link" copy.
pub const OUTLOOK_CONSENT_REQUIRED: &str = "outlook.consent_required";

/// Graph returned `404 Not Found` for a resource the connector
/// expected to exist (most commonly a calendar that was deleted
/// between this walk and the previous one). Distinct from the
/// generic HTTP transport family because a 404 from Graph usually
/// means "keep going, the missing row is not fatal" — the walker
/// drops the shard and continues. Surfaced so observers can tell
/// apart a connector-level data-shape issue from a transient
/// network 404.
pub const OUTLOOK_RESOURCE_NOT_FOUND: &str = "outlook.resource_not_found";

/// Graph returned `429 Too Many Requests` after exhausting the
/// retry budget the walker is willing to spend inside one sync. The
/// SDK's HTTP client honours `Retry-After` on the first hit; this
/// code fires only when the same day's walk keeps getting throttled
/// beyond a sensible number of retries, which points at a
/// multi-client collision (the user is signed into Graph from
/// several tools). Surfaced as a non-fatal
/// [`DayseamError::Transient`] so the next scheduled walk retries
/// from scratch.
pub const OUTLOOK_RATE_LIMITED: &str = "outlook.rate_limited";

/// Graph returned a `5xx` the walker couldn't classify as an
/// auth / scope / not-found failure. Transient by default so the
/// next scheduled walk picks up where this one left off; chronic
/// 5xxs are visible via the source's `SourceHealth.last_error`.
pub const OUTLOOK_UPSTREAM_5XX: &str = "outlook.upstream_5xx";

/// Graph returned a successful response whose JSON did not match
/// the shape the walker normalises — e.g. a required field is
/// missing, a datetime parses as a string we can't interpret, or
/// an enum carries a value we don't have a branch for. The
/// specific offending field is captured in the error's `message`
/// so the author of the walker update has a trail. Surfaced as
/// [`DayseamError::Internal`] rather than auth: it is a contract
/// drift on Graph's side, not a user-actionable failure.
pub const OUTLOOK_UPSTREAM_SHAPE_CHANGED: &str = "outlook.upstream_shape_changed";

/// Graph returned `410 Gone` or an equivalent shape telling us the
/// mailbox itself has been removed — the account was terminated in
/// the tenant between walks. Terminal on this source; the UI
/// surfaces a Disconnect hint rather than a Reconnect one because
/// reconnecting the same credential cannot resurrect the mailbox.
pub const OUTLOOK_RESOURCE_GONE: &str = "outlook.resource_gone";

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
    HTTP_TRANSPORT_DNS,
    HTTP_TRANSPORT_TLS,
    HTTP_TRANSPORT_CONNECT,
    HTTP_TRANSPORT_TIMEOUT,
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
    IPC_OUTLOOK_SESSION_NOT_FOUND,
    IPC_OUTLOOK_SESSION_NOT_READY,
    IPC_OUTLOOK_KEYCHAIN_WRITE_FAILED,
    IPC_OUTLOOK_TENANT_UNRESOLVED,
    IPC_OUTLOOK_SOURCE_ALREADY_EXISTS,
    IPC_SINK_INVALID_CONFIG,
    IPC_INVALID_DISPLAY_NAME,
    IPC_SHELL_URL_DISALLOWED,
    IPC_SHELL_URL_INVALID,
    IPC_SHELL_OPEN_FAILED,
    OAUTH_TOKEN_EXPIRED,
    OAUTH_DESCRIPTOR_MISMATCH,
    OAUTH_REFRESH_REJECTED,
    OAUTH_SCOPE_DOWNGRADED,
    OAUTH_LOGIN_TIMEOUT,
    OAUTH_LOGIN_STATE_MISMATCH,
    OAUTH_LOGIN_LOOPBACK_BIND_FAILED,
    OAUTH_LOGIN_USER_CANCELLED,
    OAUTH_LOGIN_BROWSER_OPEN_FAILED,
    OAUTH_LOGIN_SESSION_NOT_FOUND,
    OAUTH_LOGIN_AUTHORIZATION_ERROR,
    OAUTH_LOGIN_PROVIDER_UNKNOWN,
    OAUTH_LOGIN_NOT_CONFIGURED,
    OUTLOOK_AUTH_INVALID_CREDENTIALS,
    OUTLOOK_AUTH_MISSING_SCOPE,
    OUTLOOK_CONSENT_REQUIRED,
    OUTLOOK_RESOURCE_NOT_FOUND,
    OUTLOOK_RATE_LIMITED,
    OUTLOOK_UPSTREAM_5XX,
    OUTLOOK_UPSTREAM_SHAPE_CHANGED,
    OUTLOOK_RESOURCE_GONE,
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
