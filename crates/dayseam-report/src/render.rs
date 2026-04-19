//! Stage 3: walk the rollup output, build bullets, run the template.
//!
//! The engine emits exactly one bullet per rolled-up artifact — a
//! merged MR would become one bullet, its commits the evidence
//! beneath it (`ARCHITECTURE.md` §10). Phase 2 ships only
//! [`dayseam_core::ArtifactKind::CommitSet`] so the section taxonomy
//! is a single "Commits" section; rollup already orders by kind so
//! additional sections (merge requests, issues) slot in without
//! touching this module.
//!
//! **Determinism.** `bullet_id` is a sha256 of
//! `(template_id || section_id || artifact_id || sorted_event_ids)`
//! so it never depends on the iteration order of a map, the system
//! clock, or a RNG. Tests lean on this heavily — see
//! `tests/golden.rs` + `tests/purity.rs`.

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
            SourceIdentityKind::GitLabUserId
            | SourceIdentityKind::GitLabUsername
            | SourceIdentityKind::GitHubLogin => {
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
    let mut bullets: Vec<RenderedBullet> = Vec::with_capacity(groups.len());
    let mut evidence: Vec<Evidence> = Vec::new();

    for group in groups {
        let (bullet, group_evidence) = render_group_bullet(
            group,
            registry,
            template_id,
            COMMITS_SECTION_ID,
            verbose_mode,
        )?;
        evidence.push(group_evidence);
        bullets.push(bullet);
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

fn render_group_bullet(
    group: &RolledUpArtifact,
    registry: &handlebars::Handlebars<'_>,
    template_id: &str,
    section_id: &str,
    verbose_mode: bool,
) -> Result<(RenderedBullet, Evidence), ReportError> {
    let event_ids: Vec<Uuid> = group.events.iter().map(|e| e.id).collect();
    let id = bullet_id(template_id, section_id, group.artifact.id, &event_ids);
    let reason = evidence_reason(&group.events);

    let any_redacted = group
        .events
        .iter()
        .any(|e| matches!(e.privacy, Privacy::RedactedPrivateRepo));

    let text = if any_redacted {
        format!(
            "{REDACTED_BULLET_TEXT} — {}",
            crate::templates::dev_eod::render_evidence_suffix(&reason)
        )
    } else {
        let ctx = BulletCtx {
            headline: headline(&group.artifact.payload, &group.events),
            evidence: reason.clone(),
            verbose_mode,
            verbose_lines: if verbose_mode {
                verbose_event_lines(&group.events)
            } else {
                Vec::new()
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

fn headline(payload: &ArtifactPayload, events: &[ActivityEvent]) -> String {
    match payload {
        ArtifactPayload::CommitSet { repo_path, .. } => {
            let repo_label = repo_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| repo_path.to_string_lossy().to_string());
            // The headline shows the first commit's subject as the
            // "what I did" summary. Count is intentionally *not* in
            // the headline — the evidence suffix owns the count so
            // the pluralisation rule lives in one place.
            let summary = events
                .first()
                .map(|e| e.title.clone())
                .unwrap_or_else(|| "(no commits)".to_string());
            format!("**{repo_label}** — {summary}")
        }
    }
}

fn verbose_event_lines(events: &[ActivityEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            let sha_short = short_sha(&e.external_id);
            format!("`{sha_short}` {}", e.title)
        })
        .collect()
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn evidence_reason(events: &[ActivityEvent]) -> String {
    match events.len() {
        0 => "no evidence".to_string(),
        1 => "1 commit".to_string(),
        n => format!("{n} commits"),
    }
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

#[derive(Serialize)]
struct BulletCtx {
    headline: String,
    evidence: String,
    verbose_mode: bool,
    verbose_lines: Vec<String>,
}
