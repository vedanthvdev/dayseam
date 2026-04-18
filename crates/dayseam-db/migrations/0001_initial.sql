-- Dayseam v1 schema. Mirrors §5.2 of the design doc verbatim.
--
-- Conventions:
--   * Timestamps are ISO-8601 UTC strings.
--   * Dates are `YYYY-MM-DD` local-timezone strings.
--   * JSON columns are plain TEXT and named `<field>_json`.
--   * UUIDs are stored as hyphenated strings.

CREATE TABLE sources (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  label TEXT NOT NULL,
  config_json TEXT NOT NULL,
  secret_ref TEXT,
  created_at TEXT NOT NULL,
  last_sync_at TEXT,
  last_health_json TEXT NOT NULL
);

CREATE TABLE identities (
  id TEXT PRIMARY KEY,
  emails_json TEXT NOT NULL,
  gitlab_user_ids_json TEXT NOT NULL,
  display_name TEXT NOT NULL
);

CREATE TABLE local_repos (
  path TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  label TEXT NOT NULL,
  is_private INTEGER NOT NULL DEFAULT 0,
  discovered_at TEXT NOT NULL
);

CREATE TABLE activity_events (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  external_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  actor_json TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT,
  links_json TEXT NOT NULL,
  entities_json TEXT NOT NULL,
  parent_external_id TEXT,
  metadata_json TEXT NOT NULL,
  raw_ref TEXT NOT NULL,
  privacy TEXT NOT NULL DEFAULT 'Normal',
  UNIQUE(source_id, external_id, kind)
);

CREATE INDEX idx_events_occurred_at ON activity_events(occurred_at);
CREATE INDEX idx_events_source_date ON activity_events(source_id, occurred_at);
CREATE INDEX idx_events_parent ON activity_events(parent_external_id);

CREATE TABLE raw_payloads (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  endpoint TEXT NOT NULL,
  fetched_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  payload_sha256 TEXT NOT NULL
);

CREATE INDEX idx_raw_payloads_fetched_at ON raw_payloads(fetched_at);

CREATE TABLE report_drafts (
  id TEXT PRIMARY KEY,
  date TEXT NOT NULL,
  template_id TEXT NOT NULL,
  template_version TEXT NOT NULL,
  sections_json TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  per_source_state_json TEXT NOT NULL,
  verbose_mode INTEGER NOT NULL,
  generated_at TEXT NOT NULL
);

CREATE INDEX idx_drafts_date ON report_drafts(date);
CREATE INDEX idx_drafts_generated_at ON report_drafts(generated_at);

CREATE TABLE log_entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  level TEXT NOT NULL,
  source TEXT NOT NULL,
  message TEXT NOT NULL,
  context_json TEXT
);

CREATE INDEX idx_logs_ts ON log_entries(ts);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL
);
