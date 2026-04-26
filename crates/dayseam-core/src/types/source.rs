//! Source connectors — the read-only side of Dayseam. Each configured
//! source represents one place we pull activity from (a GitLab instance, a
//! set of local git scan roots).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::DayseamError;

/// Opaque id for a configured source. We use `Uuid` rather than a string
/// slug so connectors can be reconfigured (e.g. rename a GitLab instance)
/// without breaking primary-key invariants in the activity store.
pub type SourceId = Uuid;

/// The persisted record describing one configured source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Source {
    pub id: SourceId,
    pub kind: SourceKind,
    /// Human-readable label shown in the UI ("gitlab.internal.acme.com",
    /// "Work laptop repos"). Not required to be unique.
    pub label: String,
    pub config: SourceConfig,
    pub secret_ref: Option<SecretRef>,
    pub created_at: DateTime<Utc>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_health: SourceHealth,
}

/// The high-level category of a source. Used for UI grouping and so the
/// dispatcher knows which connector implementation to call.
///
/// `Jira` and `Confluence` were added in DAY-73 (v0.2 Atlassian connectors).
/// A single email + API-token credential can back one source of each kind
/// for the same workspace — the sources share a `secret_ref` pointing at
/// one keychain row (ref-counted on delete in DAY-81). Neither connector
/// implementation ships in DAY-73: this PR only lands the discriminant so
/// later tasks can register themselves into the dispatcher without a
/// core-types amendment. The connector scaffolds in DAY-76 / DAY-79
/// add the matching [`SourceConfig`] variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SourceKind {
    GitLab,
    LocalGit,
    Jira,
    Confluence,
    /// GitHub account — the v0.4 fifth connector. One PAT
    /// authenticates one GitHub account; unlike the Atlassian case
    /// there's no shared-credential-across-products situation to
    /// model, because GitHub is single-product. Added in DAY-93.
    GitHub,
    /// Microsoft 365 / Outlook calendar — the v0.9 sixth connector.
    ///
    /// One OAuth 2.0 PKCE consent authenticates one work/school
    /// account against Microsoft Graph. Unlike GitHub's single-PAT
    /// model, Outlook carries an access/refresh token pair in the
    /// keychain; the access token rotates every ~60–90 minutes and
    /// the refresh token rotates on every successful refresh
    /// (per Microsoft's rotation policy). `AuthDescriptor::OAuth`
    /// is the durable shape the orchestrator rebuilds at boot, and
    /// `OAuthAuth` in `connectors-sdk` does the refresh dance
    /// single-flighted. Added in DAY-202.
    Outlook,
}

/// Per-kind configuration. The enum is externally tagged so the on-disk
/// JSON carries the variant name, which makes schema migrations obvious
/// when we add new source kinds later.
///
/// `LocalGit` intentionally only carries `scan_roots` — approved repos are
/// first-class rows in the `local_repos` table so we never have two
/// sources of truth for the same list.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, dayseam_macros::SerdeDefaultAudit,
)]
#[ts(export)]
pub enum SourceConfig {
    GitLab {
        base_url: String,
        user_id: i64,
        username: String,
    },
    LocalGit {
        scan_roots: Vec<PathBuf>,
    },
    /// Atlassian Jira Cloud workspace.
    ///
    /// `workspace_url` is the tenant base URL the connector joins
    /// `/rest/api/3/*` onto — e.g. `https://acme.atlassian.net`. The
    /// `email` is the account identity the per-source
    /// [`connectors_sdk::BasicAuth`] is constructed from at the IPC
    /// layer (DAY-82); the API token itself lives behind the source's
    /// `secret_ref` and never touches this row. Keeping `email` on the
    /// config (rather than the auth strategy) is what lets two
    /// sources — one `Jira`, one `Confluence` — share a single
    /// keychain entry in the "shared PAT" flow while still being
    /// addressable as independent auth contexts.
    ///
    /// Added in DAY-76 (v0.2 Atlassian scaffold). The sibling
    /// `Confluence` variant lands in DAY-79.
    Jira {
        workspace_url: String,
        email: String,
    },
    /// Atlassian Confluence Cloud workspace.
    ///
    /// `workspace_url` is the same tenant base URL [`Self::Jira`]
    /// carries — e.g. `https://acme.atlassian.net`. The Confluence
    /// connector joins `/wiki/rest/api/*` and `/wiki/api/v2/*` onto
    /// it, but the auth probe (`GET /rest/api/3/myself`) shares the
    /// Jira endpoint because a single Atlassian Cloud credential
    /// authenticates both products.
    ///
    /// `email` is the account identity the per-source
    /// [`connectors_sdk::BasicAuth`] is constructed from at the IPC
    /// layer. Mirrors [`Self::Jira::email`] so a Confluence-only
    /// source (Journey C in the Add-Source dialog) can authenticate
    /// without relying on a paired Jira sibling for the email —
    /// v0.2.0 shipped without this field, which broke every
    /// Confluence-only install the moment it tried to run a report
    /// (DOG-v0.2-01 in the v0.2.1 capstone). The `#[serde(default)]`
    /// keeps `sources_list` decoding any stray v0.2.0 row; the IPC
    /// auth builder then returns a clear `confluence.auth.*` error
    /// that routes the UI to the Reconnect flow.
    ///
    /// Added in DAY-79 (v0.2 Atlassian Confluence scaffold); `email`
    /// added in DAY-84 (v0.2.1 capstone).
    Confluence {
        workspace_url: String,
        #[serde(default)]
        #[serde_default_audit(repair = "confluence_email")]
        email: String,
    },
    /// GitHub account.
    ///
    /// `api_base_url` is the REST API root the connector joins
    /// endpoints onto — `https://api.github.com` for github.com
    /// tenants (the common case) and `https://<host>/api/v3` for
    /// GitHub Enterprise Server. Storing it per-source (rather than
    /// inferring github.com) is what lets a single laptop connect
    /// to both a personal github.com account and a work Enterprise
    /// instance simultaneously without ambiguity.
    ///
    /// The PAT itself lives behind the source's `secret_ref` and
    /// never touches this row; the IPC auth builder reads the token
    /// out of the keychain and wraps it in
    /// `connectors_sdk::PatAuth::github(..)` at request time.
    ///
    /// Added in DAY-93 (v0.4 GitHub connector core-types). The
    /// connector scaffold in DAY-95 consumes this variant; no
    /// production code emits it yet.
    GitHub {
        api_base_url: String,
    },
    /// Microsoft 365 / Outlook calendar.
    ///
    /// `tenant_id` is the AAD tenant GUID the connector targets
    /// (e.g. `"consumers"` for personal MSA, a GUID string for a
    /// work tenant). The Graph walker uses `graph.microsoft.com` as
    /// a global endpoint — it does not vary by tenant — but the
    /// tenant id is the stable identifier we bind the source's
    /// self-identity against, and it is also the authority segment
    /// used on `/{tenant}/oauth2/v2.0/token` during the refresh
    /// dance. Storing it per-source (rather than inferring from the
    /// id token on each refresh) keeps the orchestrator pure: the
    /// `AuthDescriptor::OAuth` rebuild at boot is a straight DB
    /// read, no network round-trip required.
    ///
    /// `user_principal_name` is the UPN (e.g. `"alice@contoso.com"`)
    /// the walker filters on when seeding the self-identity. Unlike
    /// GitHub's numeric id anchor, Outlook does not have an
    /// immutable numeric primary key we can address — the stable
    /// anchor is the pair `(tenant_id, user_object_id)`, where
    /// `user_object_id` is captured separately as
    /// `SourceIdentityKind::OutlookUserObjectId`. The UPN lives on
    /// the config for human-readable display and to scope
    /// keychain account names.
    ///
    /// The access + refresh tokens themselves live behind the
    /// source's `secret_ref` — specifically, two sibling keychain
    /// entries (one per token) under the same service — and never
    /// touch this row. The IPC auth builder rebuilds the
    /// `OAuthAuth` strategy from the descriptor at request time.
    ///
    /// Added in DAY-202 (v0.9 Outlook calendar scaffold).
    Outlook {
        tenant_id: String,
        user_principal_name: String,
    },
}

impl SourceConfig {
    /// Project a [`SourceConfig`] down to its [`SourceKind`] discriminant.
    /// Used by the IPC layer to reject patches that would secretly
    /// widen a `LocalGit` source into a `GitLab` one.
    #[must_use]
    pub fn kind(&self) -> SourceKind {
        match self {
            SourceConfig::GitLab { .. } => SourceKind::GitLab,
            SourceConfig::LocalGit { .. } => SourceKind::LocalGit,
            SourceConfig::Jira { .. } => SourceKind::Jira,
            SourceConfig::Confluence { .. } => SourceKind::Confluence,
            SourceConfig::GitHub { .. } => SourceKind::GitHub,
            SourceConfig::Outlook { .. } => SourceKind::Outlook,
        }
    }
}

impl SourceKind {
    /// Stable, human-facing label for a [`SourceKind`]. Used in the
    /// report's per-source subheadings (DAY-104) and anywhere else
    /// we render the kind as prose.
    ///
    /// The serde wire form is still the PascalCase variant name
    /// (`"GitLab"`, `"LocalGit"`, …); this helper is **display-only**
    /// and intentionally diverges from serde for the `LocalGit` case
    /// — the wire form is a programming identifier, the display form
    /// is English.
    #[must_use]
    pub const fn display_label(&self) -> &'static str {
        match self {
            SourceKind::GitLab => "GitLab",
            SourceKind::LocalGit => "Local git",
            SourceKind::Jira => "Jira",
            SourceKind::Confluence => "Confluence",
            SourceKind::GitHub => "GitHub",
            SourceKind::Outlook => "Outlook",
        }
    }

    /// Single-glyph emoji used in the report's per-source subheadings
    /// (DAY-104 — the "hybrid display" the v0.4 dogfood asked for).
    /// Chosen to be unambiguous on modern macOS / Linux / Windows
    /// markdown viewers and intentionally avoiding product logos so
    /// we don't ship a trademark surface.
    #[must_use]
    pub const fn display_emoji(&self) -> &'static str {
        match self {
            SourceKind::GitLab => "🦊",
            SourceKind::LocalGit => "💻",
            SourceKind::Jira => "📋",
            SourceKind::Confluence => "📄",
            SourceKind::GitHub => "🐙",
            SourceKind::Outlook => "📅",
        }
    }

    /// Pre-joined "`<emoji> <label>`" form, used verbatim as the
    /// `### ...` subheading text in both the markdown sink and the
    /// in-app `StreamingPreview`. Kept as a single helper so the
    /// emoji / label contract lives in one place — changing the
    /// separator or dropping an emoji is a one-line diff.
    #[must_use]
    pub fn display_with_emoji(&self) -> String {
        format!("{} {}", self.display_emoji(), self.display_label())
    }

    /// Iteration order for rendering per-kind subgroups inside a
    /// report section (DAY-104). Matches the enum's declaration
    /// order so the markdown output is deterministic and a future
    /// variant addition is a single edit site. A `Vec` (not a
    /// slice) because callers frequently want to `retain` or `map`
    /// over it; the five-element `Vec` allocation is negligible
    /// next to a report render.
    #[must_use]
    pub fn render_order() -> Vec<SourceKind> {
        vec![
            SourceKind::LocalGit,
            SourceKind::GitHub,
            SourceKind::GitLab,
            SourceKind::Jira,
            SourceKind::Confluence,
            SourceKind::Outlook,
        ]
    }
}

/// Partial update payload for the `sources_update` IPC command. Both
/// fields are optional so the frontend can update just the label,
/// just the config, or both in one round-trip. The command enforces
/// that any supplied `config.kind()` matches the persisted source's
/// `kind`; otherwise the call is rejected before any write happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourcePatch {
    pub label: Option<String>,
    pub config: Option<SourceConfig>,
}

/// Opaque handle the secrets crate resolves against the OS keychain. The
/// actual secret bytes never touch the database or IPC layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SecretRef {
    pub keychain_service: String,
    pub keychain_account: String,
}

/// Last observed health of a source. `ok == true` with no error means the
/// last probe succeeded; `ok == false` surfaces the specific
/// `DayseamError` so the UI can display an actionable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceHealth {
    pub ok: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<DayseamError>,
}

impl SourceHealth {
    /// Sensible default for a freshly created source that has never been
    /// probed — we mark it as "ok unless proven otherwise" so the UI
    /// doesn't show a spurious red badge before the first sync.
    pub fn unchecked() -> Self {
        Self {
            ok: true,
            checked_at: None,
            last_error: None,
        }
    }
}

/// Successful return shape of the `gitlab_validate_pat` IPC command. The
/// frontend's add-source dialog captures these two fields onto the new
/// [`SourceConfig::GitLab`] row before persisting the source, so the
/// identity the connector walks by (`user_id`) is the one GitLab itself
/// echoed back, not whatever the user typed. The username is returned
/// alongside purely for UI display — the authoritative match is on the
/// numeric id, which never changes when a username is renamed upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitlabValidationResult {
    pub user_id: i64,
    pub username: String,
}

/// Successful return shape of the `atlassian_validate_credentials` IPC
/// command. Mirrors the internal `connector_atlassian_common::cloud::
/// AtlassianAccountInfo` but lives here because only `dayseam-core`
/// types are routed through `ts-rs` (and the upstream struct does not
/// implement `Serialize` to keep the walker crate free of IPC
/// concerns).
///
/// The dialog uses `display_name` + `email` for the "Connected as …"
/// confirmation ribbon and stashes `account_id` so the subsequent
/// `atlassian_sources_add` call can seed the `AtlassianAccountId`
/// self-identity without a second `/rest/api/3/myself` round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AtlassianValidationResult {
    /// Opaque Atlassian Cloud account id returned by
    /// `GET /rest/api/3/myself`. Used as the
    /// `SourceIdentity::external_actor_id` under kind
    /// [`crate::SourceIdentityKind::AtlassianAccountId`].
    pub account_id: String,
    /// Display name the workspace shows for this account. Surfaced in
    /// the dialog's confirmation ribbon ("Connected as Vedanth V").
    pub display_name: String,
    /// Email the workspace has on file for this account. Optional —
    /// Atlassian accounts whose email privacy is enabled omit it.
    pub email: Option<String>,
}

/// Successful return shape of the `github_validate_credentials` IPC
/// command (DAY-99). Mirrors the internal
/// `connector_github::auth::GithubUserInfo` but lives here because
/// only `dayseam-core` types are routed through `ts-rs` (and the
/// upstream struct intentionally does not implement `Serialize` to
/// keep the walker crate free of IPC concerns).
///
/// The dialog renders `name.unwrap_or(login)` in the "Connected as
/// …" confirmation ribbon and stashes `user_id` so the subsequent
/// `github_sources_add` call can seed the
/// [`crate::SourceIdentityKind::GitHubUserId`] self-identity without
/// a second `/user` round-trip. The numeric `user_id` is the stable
/// identity anchor; `login` can be renamed by the user upstream and
/// is kept only for human-readable display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GithubValidationResult {
    /// Numeric user id returned by `GET <api_base_url>/user`.`id`.
    /// Stable for the lifetime of the account — survives rename.
    /// Used as the `SourceIdentity::external_actor_id` under kind
    /// [`crate::SourceIdentityKind::GitHubUserId`].
    pub user_id: i64,
    /// Login handle (`@handle`). Mutable upstream; surfaced in the
    /// dialog's confirmation ribbon and in bullet attribution, but
    /// not the identity anchor.
    pub login: String,
    /// Display name. Optional — GitHub users can leave it blank; the
    /// dialog falls back to `login` when `None`.
    pub name: Option<String>,
}
