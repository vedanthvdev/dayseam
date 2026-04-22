//! DAY-89 CONS-v0.2-06. Cross-connector property test that pins the
//! "surface area invariant" the CONS lens was built to protect: every
//! connector must map the same HTTP status to the same
//! `DayseamError` category and a `{service}.*` code shape that
//! matches its sibling connectors.
//!
//! Why this lives in the orchestrator crate: `dayseam-orchestrator` is
//! the lowest point in the dep graph that already depends on all three
//! connector crates, so this is where a compile-or-test regression in
//! any one of them breaks first. If a future connector forgets to map
//! 500 / 502 / 503 / 504 / 410, this test fails with a message naming
//! the connector and the missing status.
//!
//! The invariant has three parts:
//! 1. 5xx statuses (500, 502, 503, 504) each map to a
//!    `DayseamError::Network` with a code of shape
//!    `{service}.upstream_5xx`. Before DAY-89, Atlassian 5xx mapped to
//!    `UpstreamChanged` + `{service}.walk.upstream_shape_changed` — a
//!    lie that treated server outages as walker-shape bugs.
//! 2. 410 Gone maps to `DayseamError::Network` with
//!    `{service}.resource_gone`. Before DAY-89 both connector families
//!    lumped 410 into the catch-all 5xx/shape-changed arm, so a
//!    permanently-deleted upstream resource kept retrying.
//! 3. The registered `error_codes::ALL` set contains each of the six
//!    derived codes (`{jira,confluence,gitlab} × {upstream_5xx,
//!    resource_gone}`). This keeps the taxonomy-completeness check
//!    honest even after a future rename.

use connector_atlassian_common::{map_status as map_atlassian_status, Product};
use connector_gitlab::map_gitlab_status;
use dayseam_core::{error_codes, DayseamError};
use reqwest::StatusCode;

/// The four 5xx statuses the SDK's retry budget can surface to the
/// connector-local classifier. 501 is omitted deliberately — Atlassian
/// and GitLab have never returned it and the retry loop treats it as a
/// non-retriable permanent error that would skew the symmetry claim.
const SERVER_ERROR_STATUSES: &[StatusCode] = &[
    StatusCode::INTERNAL_SERVER_ERROR,
    StatusCode::BAD_GATEWAY,
    StatusCode::SERVICE_UNAVAILABLE,
    StatusCode::GATEWAY_TIMEOUT,
];

/// Expected code for a given `(service, status)` pair. Kept explicit
/// rather than computed so a rename on either side is flagged by the
/// human author, not silently papered over.
fn expected_code(service: &str, status: StatusCode) -> &'static str {
    match (service, status) {
        ("jira", StatusCode::GONE) => error_codes::JIRA_RESOURCE_GONE,
        ("jira", _) => error_codes::JIRA_UPSTREAM_5XX,
        ("confluence", StatusCode::GONE) => error_codes::CONFLUENCE_RESOURCE_GONE,
        ("confluence", _) => error_codes::CONFLUENCE_UPSTREAM_5XX,
        ("gitlab", StatusCode::GONE) => error_codes::GITLAB_RESOURCE_GONE,
        ("gitlab", _) => error_codes::GITLAB_UPSTREAM_5XX,
        (svc, s) => unreachable!("unexpected service/status pair: {svc}/{s}"),
    }
}

/// Run the atlassian or gitlab `map_status` helper and return the
/// resolved `DayseamError`. The three connectors expose `map_status`
/// with different signatures (atlassian takes a `Product`; gitlab
/// doesn't); this helper normalises the edge.
fn map_for(service: &str, status: StatusCode) -> DayseamError {
    match service {
        "jira" => map_atlassian_status(Product::Jira, status, "simulated").into(),
        "confluence" => map_atlassian_status(Product::Confluence, status, "simulated").into(),
        "gitlab" => map_gitlab_status(status, "simulated").into(),
        other => unreachable!("unexpected service {other}"),
    }
}

#[test]
fn server_error_mapping_is_symmetric_across_connectors() {
    for service in ["jira", "confluence", "gitlab"] {
        for status in SERVER_ERROR_STATUSES {
            let err = map_for(service, *status);
            let expected = expected_code(service, *status);
            assert_eq!(
                err.code(),
                expected,
                "{service} + {status} must map to {expected} (got {})",
                err.code()
            );
            assert_eq!(
                err.variant(),
                "Network",
                "{service} + {status} must be Network-category (transient) so the \
                 orchestrator can retry the next run; got {}",
                err.variant()
            );
        }
    }
}

#[test]
fn resource_gone_mapping_is_symmetric_across_connectors() {
    for service in ["jira", "confluence", "gitlab"] {
        let err = map_for(service, StatusCode::GONE);
        let expected = expected_code(service, StatusCode::GONE);
        assert_eq!(
            err.code(),
            expected,
            "{service} + 410 must map to {expected} (got {})",
            err.code()
        );
        assert_eq!(
            err.variant(),
            "Network",
            "{service} + 410 must be Network-category (terminal, non-retriable); got {}",
            err.variant()
        );
    }
}

#[test]
fn all_six_derived_codes_are_registered() {
    let expected = [
        error_codes::JIRA_UPSTREAM_5XX,
        error_codes::JIRA_RESOURCE_GONE,
        error_codes::CONFLUENCE_UPSTREAM_5XX,
        error_codes::CONFLUENCE_RESOURCE_GONE,
        error_codes::GITLAB_UPSTREAM_5XX,
        error_codes::GITLAB_RESOURCE_GONE,
    ];
    for code in expected {
        assert!(
            error_codes::ALL.contains(&code),
            "{code} missing from error_codes::ALL registry"
        );
    }
}
