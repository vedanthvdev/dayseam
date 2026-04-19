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
    let repo_label = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| repo_path.to_string_lossy().to_string());
    format!("**{repo_label}** — {}", event.title)
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
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
}
