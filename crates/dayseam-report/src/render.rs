//! Stage 3: walk the rollup output, build bullets, run the template.
//!
//! For [`dayseam_core::ArtifactKind::CommitSet`] groups — the only
//! kind Phase 2 ships — the engine emits **one bullet per commit**.
//! The earlier design was "one bullet per artifact" (one bullet per
//! repo-day CommitSet, with a `_N commits_` evidence suffix), but
//! that collapsed N distinct pieces of work behind whichever commit
//! happened to be earliest on the day and hid all the rest. Phase 3
//! artifact kinds (`MergeRequest`, `Issue`) will still be one bullet
//! per artifact; each kind owns its own rendering rule (see
//! `render_group` below).
//!
//! **Determinism.** `bullet_id` is a sha256 of
//! `(template_id || section_id || artifact_id || sorted_event_ids)`
//! so it never depends on the iteration order of a map, the system
//! clock, or a RNG. Per-commit bullets key on `[event.id]` — a
//! one-element vector — which is still deterministic per commit.
//! Tests lean on this heavily — see `tests/golden.rs` +
//! `tests/invariants.rs`.

use dayseam_core::{
    ActivityEvent, ArtifactId, ArtifactPayload, Evidence, Privacy, RenderedBullet, RenderedSection,
    ReportDraft, SourceIdentity, SourceIdentityKind,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ReportError;
use crate::input::ReportInput;
use crate::rollup::{roll_up, RolledUpArtifact};
use crate::templates::{build_registry, DEV_EOD_TEMPLATE_ID};

const COMMITS_SECTION_ID: &str = "commits";
const COMMITS_SECTION_TITLE: &str = "Commits";
const REDACTED_BULLET_TEXT: &str = "(private work)";
const PARTIAL_SECTION_COMMITS: &str = "section_commits";

/// Engine entry point used by [`crate::render`].
pub(crate) fn render(input: ReportInput) -> Result<ReportDraft, ReportError> {
    if input.template_id != DEV_EOD_TEMPLATE_ID {
        return Err(ReportError::UnknownTemplate(input.template_id));
    }

    let registry = build_registry()?;
    let filtered_events = filter_events_by_self(&input);
    let groups = roll_up(&filtered_events, &input.artifacts, input.date);

    let (sections, evidence) = if groups.is_empty() {
        (vec![empty_section(input.date)], Vec::new())
    } else {
        build_sections(&groups, &registry, &input.template_id, input.verbose_mode)?
    };

    Ok(ReportDraft {
        id: input.id,
        date: input.date,
        template_id: input.template_id,
        template_version: input.template_version,
        sections,
        evidence,
        per_source_state: input.per_source_state,
        verbose_mode: input.verbose_mode,
        generated_at: input.generated_at,
    })
}

// ---- event filtering ------------------------------------------------------

fn filter_events_by_self(input: &ReportInput) -> Vec<ActivityEvent> {
    let identities = identities_for_person(&input.source_identities, input.person.id);
    input
        .events
        .iter()
        .filter(|e| event_is_self(e, &identities))
        .cloned()
        .collect()
}

fn identities_for_person(rows: &[SourceIdentity], person_id: Uuid) -> Vec<&SourceIdentity> {
    rows.iter().filter(|r| r.person_id == person_id).collect()
}

fn event_is_self(event: &ActivityEvent, identities: &[&SourceIdentity]) -> bool {
    identities.iter().any(|id| {
        let source_matches = match id.source_id {
            Some(sid) => sid == event.source_id,
            None => true,
        };
        if !source_matches {
            return false;
        }
        match id.kind {
            SourceIdentityKind::GitEmail => event
                .actor
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(&id.external_actor_id)),
            // Atlassian Cloud populates `actor.external_id` with the
            // workspace-scoped `accountId` returned by
            // `GET /rest/api/3/myself`, so the match is the same shape
            // as the GitLab / GitHub identifier families. Added in
            // DAY-73 so the v0.2 Jira / Confluence walkers (DAY-77,
            // DAY-80) don't need to amend the self-filter in their
            // own PRs.
            SourceIdentityKind::GitLabUserId
            | SourceIdentityKind::GitLabUsername
            | SourceIdentityKind::GitHubLogin
            | SourceIdentityKind::AtlassianAccountId => {
                event.actor.external_id.as_deref() == Some(id.external_actor_id.as_str())
            }
        }
    })
}

// ---- section + bullet construction ---------------------------------------

fn build_sections(
    groups: &[RolledUpArtifact],
    registry: &handlebars::Handlebars<'_>,
    template_id: &str,
    verbose_mode: bool,
) -> Result<(Vec<RenderedSection>, Vec<Evidence>), ReportError> {
    // Pre-size for the common case where every group is a CommitSet
    // with a handful of events. Under-allocating is fine; the
    // allocator will grow the vec as needed.
    let mut bullets: Vec<RenderedBullet> = Vec::with_capacity(groups.len());
    let mut evidence: Vec<Evidence> = Vec::new();

    for group in groups {
        let rendered = render_group(
            group,
            registry,
            template_id,
            COMMITS_SECTION_ID,
            verbose_mode,
        )?;
        for (bullet, ev) in rendered {
            evidence.push(ev);
            bullets.push(bullet);
        }
    }

    let section = RenderedSection {
        id: COMMITS_SECTION_ID.to_string(),
        title: COMMITS_SECTION_TITLE.to_string(),
        bullets,
    };

    Ok((vec![section], evidence))
}

fn empty_section(date: chrono::NaiveDate) -> RenderedSection {
    RenderedSection {
        id: COMMITS_SECTION_ID.to_string(),
        title: COMMITS_SECTION_TITLE.to_string(),
        bullets: vec![RenderedBullet {
            id: empty_state_bullet_id(date),
            text: format!("*No tracked activity for {}.*", format_date_long(date)),
        }],
    }
}

/// Render every bullet this group contributes, in order.
///
/// For `CommitSet` groups, that means one bullet per commit — the
/// rule that moved here in DAY-52 to stop collapsing N unrelated
/// commits behind whichever happened to be earliest on the day.
/// Evidence is emitted one edge per commit too (`event_ids = [e.id]`)
/// so callers clicking the bullet land on exactly the commit that
/// produced the summary text.
///
/// Empty CommitSet groups (a claimed artifact whose events all got
/// filtered out before reaching the rollup) render as zero bullets;
/// the orchestrator treats a fully-empty day via the `empty_section`
/// path above, not here.
fn render_group(
    group: &RolledUpArtifact,
    registry: &handlebars::Handlebars<'_>,
    template_id: &str,
    section_id: &str,
    verbose_mode: bool,
) -> Result<Vec<(RenderedBullet, Evidence)>, ReportError> {
    match &group.artifact.payload {
        ArtifactPayload::CommitSet { repo_path, .. } => {
            let mut out = Vec::with_capacity(group.events.len());
            for event in &group.events {
                out.push(render_commit_bullet(
                    group.artifact.id,
                    repo_path,
                    event,
                    registry,
                    template_id,
                    section_id,
                    verbose_mode,
                )?);
            }
            Ok(out)
        }
        // DAY-73. Atlassian payloads are recognised by the type
        // system but not yet rendered — DAY-77 (Jira) and DAY-80
        // (Confluence) plus the generalised `group_key_from_event`
        // in DAY-78 add dedicated renderers. Returning an empty
        // bullet vec here means a Jira or Confluence artefact that
        // somehow reaches the current renderer (e.g. because a
        // future DB hand-edit dropped one in) produces an empty
        // section rather than a panic, matching the "no tracked
        // activity" fallthrough the orchestrator already expects.
        ArtifactPayload::JiraIssue { .. } | ArtifactPayload::ConfluencePage { .. } => Ok(vec![]),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_commit_bullet(
    artifact_id: ArtifactId,
    repo_path: &std::path::Path,
    event: &ActivityEvent,
    registry: &handlebars::Handlebars<'_>,
    template_id: &str,
    section_id: &str,
    verbose_mode: bool,
) -> Result<(RenderedBullet, Evidence), ReportError> {
    let event_ids = vec![event.id];
    let id = bullet_id(template_id, section_id, artifact_id, &event_ids);
    let reason = "1 commit".to_string();

    let redacted = matches!(event.privacy, Privacy::RedactedPrivateRepo);

    let text = if redacted {
        // No repo_label, no commit title, no SHA: private repo
        // contents must never leak through the report draft, and
        // `(private work)` already tells the reader "there is
        // content here, it is redacted". See
        // `tests/invariants.rs::redacted_events_render_without_message`.
        REDACTED_BULLET_TEXT.to_string()
    } else {
        let ctx = CommitBulletCtx {
            headline: commit_headline(repo_path, event),
            verbose_mode,
            short_sha: if verbose_mode {
                Some(short_sha(&event.external_id))
            } else {
                None
            },
            rolled_into_mr: if verbose_mode {
                rolled_into_mr_label(event)
            } else {
                None
            },
        };
        registry
            .render(PARTIAL_SECTION_COMMITS, &ctx)
            .map_err(|source| ReportError::Render {
                template_id: template_id.to_string(),
                source,
            })?
    };

    let bullet = RenderedBullet {
        id: id.clone(),
        text,
    };
    let evidence = Evidence {
        bullet_id: id,
        event_ids,
        reason,
    };
    Ok((bullet, evidence))
}

// ---- bullet body helpers --------------------------------------------------

fn commit_headline(repo_path: &std::path::Path, event: &ActivityEvent) -> String {
    // Belt-and-braces for DAY-71: when the rollup could not resolve a
    // human-readable repo path (no `repo` entity on the event, or one
    // whose `external_id` was empty / just `/`), the previous shape
    // rendered `**/** — <title>` because `"/".file_name()` is `None`
    // and the fallback `to_string_lossy()` returned `"/"`. Drop the
    // bolded prefix entirely in that degenerate case so the bullet at
    // least reads cleanly — the upstream fix lives in
    // [`connector_gitlab::normalise::compose_entities`], this is the
    // safety net.
    //
    // DAY-72 CONS-addendum-06: the GitLab connector also emits a
    // synthetic `project-<id>` external_id when `/projects/:id`
    // returned 404 or the field was missing. The normaliser's
    // docstring promised the render layer would drop the bolded
    // prefix for this shape; without this branch the bullet
    // rendered as `**project-42** — <title>`, which is worse than
    // useless (the user cannot act on a synthetic token).
    let raw = repo_path.to_string_lossy();
    if raw.is_empty() || raw.as_ref() == "/" {
        return event.title.clone();
    }
    if is_synthetic_project_token(&raw) {
        return event.title.clone();
    }
    let repo_label = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| raw.into_owned());
    if repo_label.is_empty() || is_synthetic_project_token(&repo_label) {
        return event.title.clone();
    }
    format!("**{repo_label}** — {}", event.title)
}

/// Recognise the synthetic `project-<digits>` token the GitLab
/// connector emits when it could not resolve `path_with_namespace`.
/// Kept local to the render layer so every place that stringifies a
/// repo path applies the same normalisation.
fn is_synthetic_project_token(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("project-") {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Format the `rolled_into_mr` label for the verbose-mode suffix.
///
/// Returns `Some("!42")` when the event carries a GitLab-style MR
/// iid (a string starting with `!`). Plain parent ids without a
/// leading `!` (e.g. GitHub's `#123`, future connectors' own
/// schemes) are passed through verbatim so the template stays
/// connector-agnostic. `None` means "no MR annotation to render".
///
/// A future connector adding a non-iid parent on a `CommitAuthored`
/// would show up here as-is; the template-level contract is "if
/// `parent_external_id` is set, show it in parentheses". Mis-set
/// parents are a connector bug, not a render concern.
fn rolled_into_mr_label(event: &ActivityEvent) -> Option<String> {
    event.parent_external_id.clone()
}

// ---- id computation -------------------------------------------------------

fn bullet_id(
    template_id: &str,
    section_id: &str,
    artifact_id: ArtifactId,
    event_ids: &[Uuid],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(template_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(section_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(artifact_id.to_string().as_bytes());
    hasher.update(b"\0");

    let mut sorted: Vec<Uuid> = event_ids.to_vec();
    sorted.sort();
    for id in sorted {
        hasher.update(id.as_bytes());
    }

    let bytes = hasher.finalize();
    format!("b_{}", hex_encode_short(&bytes[..8]))
}

fn empty_state_bullet_id(date: chrono::NaiveDate) -> String {
    // Stable id so the empty-state bullet has the same shape as a
    // real bullet and can be targeted by evidence-less tests.
    let mut hasher = Sha256::new();
    hasher.update(DEV_EOD_TEMPLATE_ID.as_bytes());
    hasher.update(b"\0");
    hasher.update(COMMITS_SECTION_ID.as_bytes());
    hasher.update(b"\0");
    hasher.update(b"empty\0");
    hasher.update(date.to_string().as_bytes());
    let bytes = hasher.finalize();
    format!("b_{}", hex_encode_short(&bytes[..8]))
}

fn hex_encode_short(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn format_date_long(date: chrono::NaiveDate) -> String {
    // `%A, %b %-d, %Y` is locale-stable and matches the design doc
    // wireframe ("Fri, Apr 17").
    date.format("%A, %b %-d, %Y").to_string()
}

// ---- handlebars context ---------------------------------------------------

/// Render context for the `section_commits` partial in per-commit
/// mode. `short_sha` is only populated when `verbose_mode` is true;
/// the template gates on `verbose_mode` so non-verbose bullets
/// never leak the SHA even if the field were populated by mistake.
#[derive(Serialize)]
struct CommitBulletCtx {
    headline: String,
    verbose_mode: bool,
    short_sha: Option<String>,
    /// When the event's `parent_external_id` points at an MR (set by
    /// the orchestrator's `annotate_rolled_into_mr` pass), the
    /// verbose-mode template renders `(rolled into !42)` after the
    /// short-SHA suffix. `None` in non-verbose mode or when the
    /// commit is not part of any MR.
    rolled_into_mr: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dayseam_core::{ActivityEvent, ActivityKind, Actor, Link, Privacy, RawRef};
    use std::path::Path;

    fn fixture_event(title: &str) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::nil(),
            source_id: Uuid::nil(),
            external_id: "external".into(),
            kind: ActivityKind::CommitAuthored,
            occurred_at: Utc.with_ymd_and_hms(2026, 4, 20, 10, 0, 0).unwrap(),
            actor: Actor {
                display_name: "Vedanth".into(),
                email: None,
                external_id: Some("17".into()),
            },
            title: title.into(),
            body: None,
            links: Vec::<Link>::new(),
            entities: Vec::new(),
            parent_external_id: None,
            metadata: serde_json::json!({}),
            raw_ref: RawRef {
                storage_key: "r".into(),
                content_type: "application/json".into(),
            },
            privacy: Privacy::Normal,
        }
    }

    /// DAY-71 regression: when the rollup couldn't resolve a
    /// repo-friendly path for an event, [`commit_headline`] used to
    /// render `**/** — <title>` because `"/".file_name()` is `None`
    /// and the fallback `to_string_lossy()` returned `"/"`. The fix
    /// drops the bolded prefix in that degenerate case so the bullet
    /// at least reads cleanly — the upstream enrichment
    /// ([`connector_gitlab::normalise::compose_entities`]) is the
    /// primary fix; this is the safety net.
    #[test]
    fn commit_headline_drops_prefix_when_repo_unknown() {
        let event = fixture_event("Merged MR: KTON-4552");
        let with_slash = commit_headline(Path::new("/"), &event);
        assert_eq!(
            with_slash, "Merged MR: KTON-4552",
            "`/` repo path must not render as `**/** — …`"
        );
        let with_empty = commit_headline(Path::new(""), &event);
        assert_eq!(
            with_empty, "Merged MR: KTON-4552",
            "empty repo path must not render a prefix"
        );
    }

    #[test]
    fn commit_headline_renders_bold_repo_prefix_for_real_paths() {
        let event = fixture_event("feat: land payments slice");
        let got = commit_headline(Path::new("modulr/modulo-local-infra"), &event);
        assert_eq!(got, "**modulo-local-infra** — feat: land payments slice");
    }

    /// DAY-72 CONS-addendum-06: the GitLab connector emits a
    /// synthetic `project-<digits>` token when `/projects/:id`
    /// returned 404 or the field was missing. The normaliser's
    /// docstring promised the render layer would strip the prefix
    /// for that shape; without this branch the bullet rendered as
    /// `**project-42** — …`, which is worse than useless.
    #[test]
    fn commit_headline_drops_prefix_for_synthetic_project_token() {
        let event = fixture_event("Opened MR: feat: land payments slice");
        assert_eq!(
            commit_headline(Path::new("project-42"), &event),
            "Opened MR: feat: land payments slice",
            "synthetic project-<digits> token must not render a bolded prefix"
        );
        assert_eq!(
            commit_headline(Path::new("project-9999"), &event),
            "Opened MR: feat: land payments slice"
        );
        // Sanity: `project-foo` (non-digits suffix) is not the
        // synthetic shape and is rendered as a regular repo label.
        assert_eq!(
            commit_headline(Path::new("project-foo"), &event),
            "**project-foo** — Opened MR: feat: land payments slice"
        );
    }
}
