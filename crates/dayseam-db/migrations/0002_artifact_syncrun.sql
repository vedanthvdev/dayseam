-- Dayseam Phase 2 schema: Artifact + SyncRun tables, plus additive columns
-- on `activity_events` and `report_drafts`. Strictly additive so a laptop
-- that shipped Phase 1 upgrades in place with no data loss.
--
-- Conventions (carried from 0001_initial.sql):
--   * Timestamps are ISO-8601 UTC strings.
--   * Dates are `YYYY-MM-DD` local-timezone strings.
--   * JSON columns are plain TEXT and named `<field>_json`.
--   * UUIDs are stored as hyphenated strings.

CREATE TABLE artifacts (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  external_id TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(source_id, kind, external_id)
);

CREATE INDEX idx_artifacts_source_kind ON artifacts(source_id, kind);
CREATE INDEX idx_artifacts_created_at ON artifacts(created_at);

CREATE TABLE sync_runs (
  id TEXT PRIMARY KEY,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  trigger_json TEXT NOT NULL,
  status TEXT NOT NULL,
  cancel_reason_json TEXT,
  superseded_by TEXT REFERENCES sync_runs(id) ON DELETE SET NULL,
  per_source_state_json TEXT NOT NULL
);

CREATE INDEX idx_sync_runs_started_at ON sync_runs(started_at);
CREATE INDEX idx_sync_runs_status ON sync_runs(status);

-- `activity_events.artifact_id` is nullable on purpose: events from the
-- Phase 1 shipped schema have no artefact; `ON DELETE SET NULL` means an
-- artefact sweep never orphans events as dead rows.
ALTER TABLE activity_events
  ADD COLUMN artifact_id TEXT REFERENCES artifacts(id) ON DELETE SET NULL;
CREATE INDEX idx_events_artifact ON activity_events(artifact_id);

-- Same story for drafts: drafts created before Phase 2 had no run to
-- point at. New drafts always carry a `sync_run_id`.
ALTER TABLE report_drafts
  ADD COLUMN sync_run_id TEXT REFERENCES sync_runs(id) ON DELETE SET NULL;
CREATE INDEX idx_drafts_sync_run ON report_drafts(sync_run_id);
