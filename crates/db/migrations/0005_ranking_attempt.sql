-- Attempt of a refresh's ranking (docs/contracts/db-schema.md): Retry ranking increments it, and a ranking result
-- updates the refresh only for the current attempt, so a late result of an earlier attempt is ignored. Additive:
-- existing refreshes start at attempt 0.
ALTER TABLE dashboard_refreshes ADD COLUMN ranking_attempt INTEGER NOT NULL DEFAULT 0 CHECK (ranking_attempt >= 0);
