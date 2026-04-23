//! The input side of the report engine.
//!
//! Everything the engine needs to produce a byte-deterministic
//! [`crate::ReportDraft`] lives on a single [`ReportInput`] struct.
//! This is deliberate: it makes the "pure function" contract visible
//! at the type level, it keeps the golden-snapshot tests tidy, and
//! it lets the orchestrator build the input once from database state
//! rather than threading five positional arguments into `render`.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use dayseam_core::{
    ActivityEvent, Artifact, Person, SourceId, SourceIdentity, SourceKind, SourceRunState,
};
use uuid::Uuid;

/// Everything [`crate::render`] needs to produce a [`crate::ReportDraft`].
///
/// The engine reads this struct and only this struct. Any field that
/// might otherwise require a side-effect (UUIDs, `Utc::now()`) is
/// passed in by the caller so the engine can stay pure.
#[derive(Debug, Clone)]
pub struct ReportInput {
    /// Predetermined id for the resulting [`crate::ReportDraft`].
    /// Supplied by the orchestrator so retries / re-renders produce
    /// the same row.
    pub id: Uuid,
    /// The date the report covers, in the user's local timezone. The
    /// engine never consults a clock; `occurred_at` filtering is the
    /// orchestrator's job, and this field is purely metadata.
    pub date: NaiveDate,
    /// The `template_id` to render with. Must be registered; see
    /// [`crate::DEV_EOD_TEMPLATE_ID`] for the Phase 2 default.
    pub template_id: String,
    /// The `template_version` to record in the draft. Not used for
    /// dispatch (the id already selects the template) but surfaces
    /// downstream so stale drafts can be re-rendered deliberately.
    pub template_version: String,
    /// The "self" [`Person`] the report is attributed to. Drives the
    /// authorship filter below.
    pub person: Person,
    /// The [`SourceIdentity`] rows that map external actors back to
    /// [`Self::person`]. An event is "mine" iff its
    /// `(source_id, actor.external_id_or_email)` pair matches a row
    /// here whose `person_id == person.id`.
    pub source_identities: Vec<SourceIdentity>,
    /// Every [`ActivityEvent`] the orchestrator fetched for
    /// [`Self::date`]. The engine does **not** filter by date — it
    /// trusts the caller.
    pub events: Vec<ActivityEvent>,
    /// The canonical [`Artifact`]s produced by the same sync runs.
    /// Rollup joins events to artifacts via
    /// [`dayseam_core::ArtifactPayload::CommitSet::event_ids`].
    pub artifacts: Vec<Artifact>,
    /// Per-source run state from the orchestrator. Surfaces in the
    /// rendered draft so "I ran the report but source X failed" is
    /// never silent.
    pub per_source_state: HashMap<SourceId, SourceRunState>,
    /// Map from [`SourceId`] to its [`SourceKind`] for every source
    /// participating in this render. The orchestrator already holds
    /// this information on each `SourceHandle`; `ReportInput`
    /// materialises it as a map so the engine can stamp
    /// `RenderedBullet::source_kind` without re-loading `Source`
    /// rows from the database.
    ///
    /// Added in DAY-104 to power the `### <emoji> <Label>`
    /// per-source subheadings. A missing entry is tolerated —
    /// `render` falls back to `None` on the bullet, the sink and
    /// preview render that bullet without a subheading, and the
    /// engine logs a debug trace with the orphan `source_id`. This
    /// mirrors the deliberately defensive stance on pre-DAY-104
    /// drafts read from SQLite (`RenderedBullet::source_kind` is
    /// `Option<SourceKind>` specifically so the UI degrades rather
    /// than panics on orphan data).
    pub source_kinds: HashMap<SourceId, SourceKind>,
    /// Verbose mode toggle. Additive-only: turning it on never
    /// changes an existing bullet's id or evidence (see invariant #2
    /// in the Phase 2 plan).
    pub verbose_mode: bool,
    /// The timestamp to stamp on the rendered draft. The engine never
    /// consults `Utc::now()`; the orchestrator passes in a captured
    /// value so golden snapshots stay byte-stable.
    pub generated_at: DateTime<Utc>,
}
