//! Atlassian-specific failure taxonomy.
//!
//! This module is the connector-local classifier DAY-74 deferred here
//! per the Phase-3 CORR-01 invariant: `HttpClient` must not
//! auto-classify non-retriable statuses. `map_status` takes the raw
//! `reqwest::Response` status the SDK returned and maps it to a typed
//! [`AtlassianError`], which `From` converts to the right
//! [`DayseamError`] variant carrying one of the nine registry codes
//! DAY-73 reserved. The nine codes are:
//!
//! | Code | Variant |
//! |---|---|
//! | `atlassian.auth.invalid_credentials` | `Auth` |
//! | `atlassian.auth.missing_scope` | `Auth` |
//! | `atlassian.cloud.resource_not_found` | `Network` |
//! | `atlassian.identity.malformed_account_id` | `UpstreamChanged` |
//! | `atlassian.adf.unrenderable_node` | `UpstreamChanged` |
//! | `jira.walk.upstream_shape_changed` | `UpstreamChanged` |
//! | `jira.walk.rate_limited` | `RateLimited` |
//! | `jira.upstream_5xx` | `Network` |
//! | `jira.resource_gone` | `Network` |
//! | `confluence.walk.upstream_shape_changed` | `UpstreamChanged` |
//! | `confluence.walk.rate_limited` | `RateLimited` |
//! | `confluence.upstream_5xx` | `Network` |
//! | `confluence.resource_gone` | `Network` |
//!
//! DAY-89 CONS-v0.2-06 introduced `{jira,confluence}.upstream_5xx` and
//! `{jira,confluence}.resource_gone` so Atlassian's 5xx + 410 mapping
//! reaches the same `Network` category GitLab's `gitlab.upstream_5xx` /
//! `gitlab.resource_gone` already use. Without the new codes, an
//! Atlassian 500 would still surface as `UpstreamChanged`, which the
//! orchestrator treats as a walker shape bug rather than a transient
//! server outage.
//!
//! [`map_status`] itself is scope-agnostic — it takes a
//! [`Product`] hint so the 429 buckets route to `jira.walk.*` vs
//! `confluence.walk.*` correctly. The caller (Jira / Confluence
//! connector in DAY-76 / DAY-79) always knows which product it's
//! working in.

use dayseam_core::{error_codes, DayseamError};
use reqwest::StatusCode;

/// Which Atlassian product the caller is classifying for. Routes the
/// 429 + `UpstreamChanged` codes between the `jira.walk.*` and
/// `confluence.walk.*` buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    Jira,
    Confluence,
}

/// Connector-local categorisation of Atlassian failure modes. Not a
/// public trait; it's the structured switch [`map_status`] drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtlassianError {
    /// 401 — `Authorization: Basic <…>` was rejected. The email or API
    /// token is wrong, was rotated, or the account was deactivated.
    /// Maps to `DayseamError::Auth` + `atlassian.auth.invalid_credentials`.
    AuthInvalidCredentials,
    /// 403 — the token authenticated but the account lacks the scope
    /// the endpoint requires (e.g. a personal API token on a project
    /// the user can't see). Maps to `DayseamError::Auth` +
    /// `atlassian.auth.missing_scope`.
    AuthMissingScope { product: Product },
    /// 404 from a cloud-discovery probe (`GET /rest/api/3/myself` or
    /// the workspace URL itself). Usually means the user mistyped the
    /// `*.atlassian.net` hostname. Maps to `DayseamError::Network` +
    /// `atlassian.cloud.resource_not_found`.
    CloudResourceNotFound { message: String },
    /// The upstream returned a shape we don't understand — a
    /// changelog field rename, an unknown issue type, a missing
    /// required field. Maps to `DayseamError::UpstreamChanged` with
    /// `{jira,confluence}.walk.upstream_shape_changed`.
    WalkShapeChanged { product: Product, message: String },
    /// 5xx after the SDK's retry budget is exhausted — the upstream
    /// service is down, degraded, or mid-deploy. Maps to
    /// `DayseamError::Network` with `{jira,confluence}.upstream_5xx`,
    /// symmetric with GitLab's `gitlab.upstream_5xx`. Kept separate
    /// from `WalkShapeChanged` so the orchestrator can treat 500s as
    /// transient (retry the next run) without masking genuine walker
    /// shape drift.
    Server5xx {
        product: Product,
        status: StatusCode,
        message: String,
    },
    /// 410 Gone — the upstream resource was deleted and the URL will
    /// never resolve again. Distinct from 404 so retries never fire.
    /// Maps to `DayseamError::Network` with `{jira,confluence}.resource_gone`,
    /// symmetric with GitLab's `gitlab.resource_gone`.
    ResourceGone { product: Product, message: String },
    /// 429 after the SDK's retry budget is exhausted. Maps to
    /// `DayseamError::RateLimited` with `{jira,confluence}.walk.rate_limited`.
    RateLimited {
        product: Product,
        retry_after_secs: u64,
    },
    /// An `accountId` returned by the upstream failed the sanity
    /// check. Maps to `DayseamError::UpstreamChanged` with
    /// `atlassian.identity.malformed_account_id`.
    IdentityMalformedAccountId { observed: String },
    /// ADF walker saw a node type it doesn't know. Degrades to
    /// `[unsupported content]` in the rendered body; this variant
    /// exists so the observability path still has a typed error to
    /// emit (as `UpstreamChanged` with
    /// `atlassian.adf.unrenderable_node`) when the caller opts in.
    AdfUnrenderableNode { node_type: String },
}

impl From<AtlassianError> for DayseamError {
    fn from(value: AtlassianError) -> Self {
        match value {
            AtlassianError::AuthInvalidCredentials => DayseamError::Auth {
                code: error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS.to_string(),
                message: "Atlassian rejected the email + API token combination".to_string(),
                retryable: false,
                action_hint: Some(
                    "Open Settings, select this Atlassian source, and click Reconnect \
                     to paste a fresh API token (or update the email if the account \
                     was renamed)."
                        .to_string(),
                ),
            },
            AtlassianError::AuthMissingScope { product } => DayseamError::Auth {
                code: error_codes::ATLASSIAN_AUTH_MISSING_SCOPE.to_string(),
                message: format!(
                    "API token authenticates but lacks the scope Dayseam needs for {}",
                    product_label(product)
                ),
                retryable: false,
                action_hint: Some(
                    "Generate a new Atlassian API token from an account that can read \
                     the relevant projects / spaces, then reconnect this source."
                        .to_string(),
                ),
            },
            AtlassianError::CloudResourceNotFound { message } => DayseamError::Network {
                code: error_codes::ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND.to_string(),
                message,
            },
            AtlassianError::WalkShapeChanged { product, message } => {
                DayseamError::UpstreamChanged {
                    code: match product {
                        Product::Jira => error_codes::JIRA_WALK_UPSTREAM_SHAPE_CHANGED.to_string(),
                        Product::Confluence => {
                            error_codes::CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED.to_string()
                        }
                    },
                    message,
                }
            }
            AtlassianError::Server5xx {
                product,
                status,
                message,
            } => DayseamError::Network {
                code: match product {
                    Product::Jira => error_codes::JIRA_UPSTREAM_5XX.to_string(),
                    Product::Confluence => error_codes::CONFLUENCE_UPSTREAM_5XX.to_string(),
                },
                message: format!("{} returned {status}: {message}", product_label(product)),
            },
            AtlassianError::ResourceGone { product, message } => DayseamError::Network {
                code: match product {
                    Product::Jira => error_codes::JIRA_RESOURCE_GONE.to_string(),
                    Product::Confluence => error_codes::CONFLUENCE_RESOURCE_GONE.to_string(),
                },
                message: format!(
                    "{} resource returned 410 Gone: {message}",
                    product_label(product)
                ),
            },
            AtlassianError::RateLimited {
                product,
                retry_after_secs,
            } => DayseamError::RateLimited {
                code: match product {
                    Product::Jira => error_codes::JIRA_WALK_RATE_LIMITED.to_string(),
                    Product::Confluence => error_codes::CONFLUENCE_WALK_RATE_LIMITED.to_string(),
                },
                retry_after_secs,
            },
            AtlassianError::IdentityMalformedAccountId { observed } => {
                DayseamError::UpstreamChanged {
                    code: error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID.to_string(),
                    message: format!(
                        "upstream returned accountId={observed:?} which fails the shape check \
                         (expected non-empty ASCII, ≤ 128 chars)"
                    ),
                }
            }
            AtlassianError::AdfUnrenderableNode { node_type } => DayseamError::UpstreamChanged {
                code: error_codes::ATLASSIAN_ADF_UNRENDERABLE_NODE.to_string(),
                message: format!(
                    "ADF walker hit an unknown node type {node_type:?}; body degraded to \
                     [unsupported content]"
                ),
            },
        }
    }
}

fn product_label(p: Product) -> &'static str {
    match p {
        Product::Jira => "Jira",
        Product::Confluence => "Confluence",
    }
}

/// Map a non-success HTTP status from an Atlassian endpoint to a typed
/// [`AtlassianError`]. Callers have already read the body (or chosen
/// to skip it) by the time they call this; `message` carries whatever
/// context the caller wants surfaced in the UI.
///
/// Per CORR-01, this function is called by each connector *after*
/// `HttpClient::send` returns a raw response — the SDK itself no
/// longer pre-classifies 401 / 403.
pub fn map_status(
    product: Product,
    status: StatusCode,
    message: impl Into<String>,
) -> AtlassianError {
    let message = message.into();
    match status {
        StatusCode::UNAUTHORIZED => AtlassianError::AuthInvalidCredentials,
        StatusCode::FORBIDDEN => AtlassianError::AuthMissingScope { product },
        StatusCode::NOT_FOUND => AtlassianError::CloudResourceNotFound { message },
        StatusCode::GONE => AtlassianError::ResourceGone { product, message },
        StatusCode::TOO_MANY_REQUESTS => AtlassianError::RateLimited {
            product,
            // The SDK's retry loop already honoured the `Retry-After`
            // header before calling us — we only see 429 here when the
            // retry budget was exhausted, and by that point the
            // original header value is no longer authoritative. Zero
            // is the conservative default; callers that have a fresher
            // value can override by constructing `RateLimited` directly.
            retry_after_secs: 0,
        },
        s if s.is_server_error() => AtlassianError::Server5xx {
            product,
            status: s,
            message,
        },
        _ => AtlassianError::WalkShapeChanged {
            product,
            message: format!("unexpected status {status}: {message}"),
        },
    }
}

/// The maximum number of characters an Atlassian `accountId` is
/// expected to contain. Atlassian Cloud account IDs are 24-char
/// opaque strings in practice (e.g. `5d53f3cbc6b9320d9ea5bdc2`), but
/// older formats and edge cases (migrated accounts, GDPR-compliant
/// re-issuance) can produce longer values. 128 is the safety margin
/// — anything longer is a shape change we want to warn-and-drop on.
pub const MAX_ACCOUNT_ID_LEN: usize = 128;

/// Validate an Atlassian `accountId` string. The shape contract —
/// non-empty, ASCII, ≤ [`MAX_ACCOUNT_ID_LEN`] chars — mirrors the
/// DAY-72 CORR-addendum-08 fix for GitLab's numeric user id: a
/// malformed row must not silently propagate through the walker's
/// self-filter, because a silent malformed-id filter collapses every
/// self-event into "unknown actor" the way DAY-71 documented.
pub fn validate_account_id(candidate: &str) -> Result<(), AtlassianError> {
    if candidate.is_empty() || candidate.len() > MAX_ACCOUNT_ID_LEN || !candidate.is_ascii() {
        return Err(AtlassianError::IdentityMalformedAccountId {
            observed: candidate.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_invalid_credentials_maps_to_atlassian_code_and_auth_variant() {
        let err: DayseamError = AtlassianError::AuthInvalidCredentials.into();
        assert_eq!(err.code(), error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS);
        assert_eq!(err.variant(), "Auth");
    }

    #[test]
    fn auth_missing_scope_routes_by_product_label_but_shares_code() {
        let jira: DayseamError = AtlassianError::AuthMissingScope {
            product: Product::Jira,
        }
        .into();
        let confluence: DayseamError = AtlassianError::AuthMissingScope {
            product: Product::Confluence,
        }
        .into();
        assert_eq!(jira.code(), error_codes::ATLASSIAN_AUTH_MISSING_SCOPE);
        assert_eq!(confluence.code(), error_codes::ATLASSIAN_AUTH_MISSING_SCOPE);
        // Message differs so the UI can say "Jira" vs "Confluence" in
        // the reconnect flow.
        let DayseamError::Auth { message: jm, .. } = jira else {
            panic!("expected Auth variant")
        };
        let DayseamError::Auth { message: cm, .. } = confluence else {
            panic!("expected Auth variant")
        };
        assert!(jm.contains("Jira"));
        assert!(cm.contains("Confluence"));
    }

    #[test]
    fn walk_shape_changed_routes_to_product_code() {
        let jira: DayseamError = AtlassianError::WalkShapeChanged {
            product: Product::Jira,
            message: "nope".into(),
        }
        .into();
        let confluence: DayseamError = AtlassianError::WalkShapeChanged {
            product: Product::Confluence,
            message: "nope".into(),
        }
        .into();
        assert_eq!(jira.code(), error_codes::JIRA_WALK_UPSTREAM_SHAPE_CHANGED);
        assert_eq!(
            confluence.code(),
            error_codes::CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED
        );
    }

    #[test]
    fn rate_limited_routes_to_product_code_and_preserves_retry_after() {
        let err: DayseamError = AtlassianError::RateLimited {
            product: Product::Jira,
            retry_after_secs: 42,
        }
        .into();
        assert_eq!(err.code(), error_codes::JIRA_WALK_RATE_LIMITED);
        if let DayseamError::RateLimited {
            retry_after_secs, ..
        } = err
        {
            assert_eq!(retry_after_secs, 42);
        } else {
            panic!("expected RateLimited variant");
        }
    }

    #[test]
    fn cloud_resource_not_found_maps_to_network_variant() {
        let err: DayseamError = AtlassianError::CloudResourceNotFound {
            message: "foo.atlassian.net is not a resolvable workspace".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND);
        assert_eq!(err.variant(), "Network");
    }

    #[test]
    fn identity_malformed_account_id_maps_to_upstream_changed() {
        let err: DayseamError = AtlassianError::IdentityMalformedAccountId {
            observed: "".into(),
        }
        .into();
        assert_eq!(
            err.code(),
            error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID
        );
        assert_eq!(err.variant(), "UpstreamChanged");
    }

    #[test]
    fn adf_unrenderable_node_maps_to_upstream_changed() {
        let err: DayseamError = AtlassianError::AdfUnrenderableNode {
            node_type: "futurePanel".into(),
        }
        .into();
        assert_eq!(err.code(), error_codes::ATLASSIAN_ADF_UNRENDERABLE_NODE);
        assert_eq!(err.variant(), "UpstreamChanged");
    }

    #[test]
    fn map_status_routes_401_and_403_to_auth_buckets() {
        assert_eq!(
            map_status(Product::Jira, StatusCode::UNAUTHORIZED, "nope"),
            AtlassianError::AuthInvalidCredentials
        );
        assert_eq!(
            map_status(Product::Jira, StatusCode::FORBIDDEN, "nope"),
            AtlassianError::AuthMissingScope {
                product: Product::Jira
            }
        );
        assert_eq!(
            map_status(Product::Confluence, StatusCode::FORBIDDEN, "nope"),
            AtlassianError::AuthMissingScope {
                product: Product::Confluence
            }
        );
    }

    #[test]
    fn map_status_routes_404_to_cloud_resource_not_found() {
        let err = map_status(Product::Jira, StatusCode::NOT_FOUND, "no such workspace");
        assert!(matches!(err, AtlassianError::CloudResourceNotFound { .. }));
    }

    #[test]
    fn map_status_routes_429_to_rate_limited_with_zero_retry_after_as_conservative_default() {
        let err = map_status(Product::Jira, StatusCode::TOO_MANY_REQUESTS, "");
        assert_eq!(
            err,
            AtlassianError::RateLimited {
                product: Product::Jira,
                retry_after_secs: 0
            }
        );
    }

    /// DAY-89 CONS-v0.2-06. Atlassian 5xx now routes to the typed
    /// `Server5xx` variant, not `WalkShapeChanged`. This is a behaviour
    /// change from v0.2.1 where 500s surfaced as
    /// `jira.walk.upstream_shape_changed` (a walker-bug code); they now
    /// surface as `jira.upstream_5xx` (a transient-network code),
    /// symmetric with GitLab.
    #[test]
    fn map_status_routes_5xx_to_server_5xx_with_status_preserved() {
        let e = map_status(
            Product::Confluence,
            StatusCode::INTERNAL_SERVER_ERROR,
            "down",
        );
        match e {
            AtlassianError::Server5xx {
                product: Product::Confluence,
                status: StatusCode::INTERNAL_SERVER_ERROR,
                ..
            } => {}
            other => panic!("expected Confluence Server5xx@500, got {other:?}"),
        }

        // 502 / 503 / 504 all hit the same arm; the status code is
        // preserved so the rendered error carries the exact upstream
        // reply.
        for s in [
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            let e = map_status(Product::Jira, s, "maintenance");
            match e {
                AtlassianError::Server5xx {
                    product: Product::Jira,
                    status,
                    ..
                } => assert_eq!(status, s),
                other => panic!("expected Jira Server5xx@{s}, got {other:?}"),
            }
        }
    }

    /// DAY-89 CONS-v0.2-06. 5xx now carries a `Network`-category
    /// `DayseamError` with `{product}.upstream_5xx`, symmetric with
    /// `gitlab.upstream_5xx`. The message includes the upstream status
    /// so log parsers can tell 500/502/503/504 apart.
    #[test]
    fn server_5xx_maps_to_network_variant_with_product_code() {
        let jira: DayseamError = AtlassianError::Server5xx {
            product: Product::Jira,
            status: StatusCode::BAD_GATEWAY,
            message: "bad gateway".into(),
        }
        .into();
        assert_eq!(jira.code(), error_codes::JIRA_UPSTREAM_5XX);
        assert_eq!(jira.variant(), "Network");
        let DayseamError::Network { message, .. } = jira else {
            panic!("expected Network variant");
        };
        assert!(message.contains("502"), "message must carry status");

        let confluence: DayseamError = AtlassianError::Server5xx {
            product: Product::Confluence,
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "down for maintenance".into(),
        }
        .into();
        assert_eq!(confluence.code(), error_codes::CONFLUENCE_UPSTREAM_5XX);
        assert_eq!(confluence.variant(), "Network");
    }

    /// DAY-89 CONS-v0.2-06. 410 now routes through `ResourceGone`, not
    /// the `_ => WalkShapeChanged` catch-all. This matters because 410
    /// is a *terminal* status — retries never succeed — and letting
    /// the orchestrator treat 410 as "walker is confused" hid genuine
    /// deleted-upstream resources under noise.
    #[test]
    fn map_status_routes_410_to_resource_gone() {
        let e = map_status(Product::Jira, StatusCode::GONE, "issue CAR-1 removed");
        match e {
            AtlassianError::ResourceGone {
                product: Product::Jira,
                ..
            } => {}
            other => panic!("expected Jira ResourceGone, got {other:?}"),
        }
    }

    /// DAY-89 CONS-v0.2-06. `ResourceGone` carries
    /// `{product}.resource_gone` on the `Network` variant, symmetric
    /// with `gitlab.resource_gone`.
    #[test]
    fn resource_gone_maps_to_network_variant_with_product_code() {
        let jira: DayseamError = AtlassianError::ResourceGone {
            product: Product::Jira,
            message: "issue CAR-1 deleted".into(),
        }
        .into();
        assert_eq!(jira.code(), error_codes::JIRA_RESOURCE_GONE);
        assert_eq!(jira.variant(), "Network");

        let confluence: DayseamError = AtlassianError::ResourceGone {
            product: Product::Confluence,
            message: "space ENG deleted".into(),
        }
        .into();
        assert_eq!(confluence.code(), error_codes::CONFLUENCE_RESOURCE_GONE);
        assert_eq!(confluence.variant(), "Network");
    }

    #[test]
    fn validate_account_id_accepts_typical_atlassian_shape() {
        assert!(validate_account_id("5d53f3cbc6b9320d9ea5bdc2").is_ok());
        assert!(validate_account_id("712020:abc-123-def").is_ok());
    }

    #[test]
    fn validate_account_id_rejects_empty_non_ascii_and_overlong() {
        assert!(matches!(
            validate_account_id(""),
            Err(AtlassianError::IdentityMalformedAccountId { .. })
        ));
        assert!(matches!(
            validate_account_id("naïve-account-id"),
            Err(AtlassianError::IdentityMalformedAccountId { .. })
        ));
        let too_long = "a".repeat(MAX_ACCOUNT_ID_LEN + 1);
        assert!(matches!(
            validate_account_id(&too_long),
            Err(AtlassianError::IdentityMalformedAccountId { .. })
        ));
    }

    /// The nine codes this module maps into must exist in the central
    /// [`error_codes::ALL`] registry — a rename on either side of the
    /// edge is caught by the `registry_snapshot` test in `dayseam-core`,
    /// but a *silent drop* here (adding a code without mapping it, or
    /// a registry code without any variant that produces it) would
    /// not be. This test holds the taxonomy-completeness line.
    #[test]
    fn error_taxonomy_matches_design() {
        let expected = [
            error_codes::ATLASSIAN_AUTH_INVALID_CREDENTIALS,
            error_codes::ATLASSIAN_AUTH_MISSING_SCOPE,
            error_codes::ATLASSIAN_CLOUD_RESOURCE_NOT_FOUND,
            error_codes::ATLASSIAN_IDENTITY_MALFORMED_ACCOUNT_ID,
            error_codes::ATLASSIAN_ADF_UNRENDERABLE_NODE,
            error_codes::JIRA_WALK_UPSTREAM_SHAPE_CHANGED,
            error_codes::JIRA_WALK_RATE_LIMITED,
            error_codes::CONFLUENCE_WALK_UPSTREAM_SHAPE_CHANGED,
            error_codes::CONFLUENCE_WALK_RATE_LIMITED,
        ];
        for code in expected {
            assert!(
                error_codes::ALL.contains(&code),
                "{code} missing from registry"
            );
        }
    }

    /// Proof of the other direction: every `AtlassianError` variant,
    /// when converted to `DayseamError`, produces a code that lives in
    /// `ALL`. Drives the plan's `atlassian_error_codes_all_registered`
    /// invariant.
    #[test]
    fn every_variant_produces_registered_code() {
        let cases: Vec<AtlassianError> = vec![
            AtlassianError::AuthInvalidCredentials,
            AtlassianError::AuthMissingScope {
                product: Product::Jira,
            },
            AtlassianError::AuthMissingScope {
                product: Product::Confluence,
            },
            AtlassianError::CloudResourceNotFound { message: "".into() },
            AtlassianError::WalkShapeChanged {
                product: Product::Jira,
                message: "".into(),
            },
            AtlassianError::WalkShapeChanged {
                product: Product::Confluence,
                message: "".into(),
            },
            AtlassianError::RateLimited {
                product: Product::Jira,
                retry_after_secs: 0,
            },
            AtlassianError::RateLimited {
                product: Product::Confluence,
                retry_after_secs: 0,
            },
            AtlassianError::IdentityMalformedAccountId {
                observed: "".into(),
            },
            AtlassianError::AdfUnrenderableNode {
                node_type: "".into(),
            },
        ];
        for variant in cases {
            let labelled = format!("{variant:?}");
            let err: DayseamError = variant.into();
            assert!(
                error_codes::ALL.contains(&err.code()),
                "variant {labelled} produced unregistered code {}",
                err.code()
            );
        }
    }
}
