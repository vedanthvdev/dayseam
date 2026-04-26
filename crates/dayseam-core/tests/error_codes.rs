//! Cross-connector invariants over the [`dayseam_core::error_codes`]
//! registry.
//!
//! Unit tests inside each connector crate prove their local
//! `map_status` → `DayseamError` routing. This file holds the
//! *cross*-connector invariants that no single connector crate can
//! observe on its own.
//!
//! ## Why the `resource_gone` quadruplet matters
//!
//! The DAY-89 Jira / Confluence / GitLab triplet established a
//! contract: every registered connector family that talks to a remote
//! API has a dedicated `{family}.resource_gone` code routed through
//! `DayseamError::Network`. The shape was picked so the
//! orchestrator's retry loop never retries 410 (resource is
//! permanently gone; retrying wastes quota and silently masks the
//! deletion), the UI surfaces "this resource has been deleted
//! upstream" copy keyed on the `_gone` suffix, and log parsers can
//! grep `*.resource_gone` across all connectors at once.
//!
//! DAY-95 adds GitHub to the registered set, so the triplet is now a
//! quadruplet. DAY-202 adds Outlook, making it a quintuplet. A
//! regression that silently drops one family's `resource_gone` code
//! from the registry (or adds a sixth family without extending the
//! cross-check) stays invisible in any single connector's test
//! surface — this file fails it loudly.
//!
//! The test below is deliberately written as an equality assertion
//! over a `HashSet`, not a `.contains(&…)` chain: an **extra** code
//! that leaks into the set (a hypothetical future
//! `slack.resource_gone` that ships without a connector to match it)
//! is just as much a registry bug as a missing one, because it means
//! the UI copy and log-parser grep will fire on a code no connector
//! actually produces.

use dayseam_core::error_codes;

/// Every registered connector family that maps upstream 410s has a
/// `{family}.resource_gone` code present in `error_codes::ALL`. The
/// cross-cutting invariant — nothing should be able to silently drop
/// one of the four codes without flipping this test red.
#[test]
fn resource_gone_code_coverage_matches_registered_connectors() {
    use std::collections::HashSet;

    let registered: HashSet<&str> = error_codes::ALL
        .iter()
        .copied()
        .filter(|c| c.ends_with(".resource_gone"))
        .collect();
    let expected: HashSet<&str> = HashSet::from([
        error_codes::GITLAB_RESOURCE_GONE,
        error_codes::JIRA_RESOURCE_GONE,
        error_codes::CONFLUENCE_RESOURCE_GONE,
        error_codes::GITHUB_RESOURCE_GONE,
        error_codes::OUTLOOK_RESOURCE_GONE,
    ]);
    assert_eq!(
        registered, expected,
        "`*.resource_gone` codes in error_codes::ALL must match the registered connector \
         families exactly; a drift here means either a connector silently lost its 410 \
         mapping or a new family shipped without updating this cross-check",
    );
}

/// Each registered `resource_gone` constant points at the documented
/// `{family}.resource_gone` wire string. Constants are what the
/// connector crates import; literal strings are what log parsers,
/// UI strings, and tests grep. A rename on either side of that edge
/// is exactly the silent-failure mode this test exists to catch.
#[test]
fn resource_gone_wire_strings_match_constant_names() {
    assert_eq!(error_codes::GITLAB_RESOURCE_GONE, "gitlab.resource_gone");
    assert_eq!(error_codes::JIRA_RESOURCE_GONE, "jira.resource_gone");
    assert_eq!(
        error_codes::CONFLUENCE_RESOURCE_GONE,
        "confluence.resource_gone"
    );
    assert_eq!(error_codes::GITHUB_RESOURCE_GONE, "github.resource_gone");
    assert_eq!(error_codes::OUTLOOK_RESOURCE_GONE, "outlook.resource_gone");
}
