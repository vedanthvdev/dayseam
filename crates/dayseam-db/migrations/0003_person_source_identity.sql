-- Dayseam Phase 2 schema: canonical `persons` + `source_identities`.
-- Strictly additive. The legacy `identities` table (from
-- 0001_initial.sql) stays in place for v0.1 backwards compatibility;
-- v0.2 retires it.
--
-- The last statement backfills exactly one `persons` row from the
-- existing `identities.display_name` if the user previously went
-- through the Phase 1 setup wizard. Guarded by `WHERE NOT EXISTS` so
-- re-running the migration (idempotence test) is a no-op.

CREATE TABLE persons (
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  is_self INTEGER NOT NULL DEFAULT 0
);

-- Only one row may carry `is_self = 1`; enforced via a partial unique
-- index so a future phase can add non-self `persons` rows freely.
CREATE UNIQUE INDEX idx_persons_single_self
  ON persons(is_self) WHERE is_self = 1;

CREATE TABLE source_identities (
  id TEXT PRIMARY KEY,
  person_id TEXT NOT NULL REFERENCES persons(id) ON DELETE CASCADE,
  source_id TEXT REFERENCES sources(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  external_actor_id TEXT NOT NULL,
  UNIQUE(person_id, source_id, kind, external_actor_id)
);

CREATE INDEX idx_source_identities_person ON source_identities(person_id);
CREATE INDEX idx_source_identities_source ON source_identities(source_id);
CREATE INDEX idx_source_identities_kind_actor
  ON source_identities(kind, external_actor_id);

-- Seed the self-person from the legacy Phase 1 identity row, if any.
-- The v1 schema allowed multiple rows; we pick the first by display_name
-- for determinism. No backfill happens on a fresh Phase 2 install —
-- `PersonRepo::bootstrap_self` handles that path.
INSERT INTO persons (id, display_name, is_self)
SELECT id, display_name, 1
FROM identities
WHERE NOT EXISTS (SELECT 1 FROM persons WHERE is_self = 1)
ORDER BY display_name ASC
LIMIT 1;
