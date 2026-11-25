-- Retain sends when an old refresh or job is pruned, so today's usage stays accurate.
CREATE TABLE dashboard_sends (
    id          INTEGER NOT NULL PRIMARY KEY,
    tenant_id   TEXT NOT NULL REFERENCES tenants (id),
    user_id     TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    refresh_id  TEXT NOT NULL,
    attempt     INTEGER NOT NULL CHECK (attempt >= 0),
    items       INTEGER NOT NULL CHECK (items > 0),
    job_id      TEXT,
    sent_at     TEXT NOT NULL,
    released_at TEXT
) STRICT;
CREATE INDEX dashboard_sends_by_time ON dashboard_sends (tenant_id, user_id, sent_at) WHERE released_at IS NULL;
CREATE INDEX dashboard_sends_by_refresh ON dashboard_sends (refresh_id);
CREATE INDEX dashboard_sends_by_job ON dashboard_sends (job_id) WHERE job_id IS NOT NULL;

-- Legacy refreshes only recorded a total. Attribute it to the refresh start.
INSERT INTO dashboard_sends (tenant_id, user_id, refresh_id, attempt, items, sent_at)
SELECT tenant_id, user_id, id, ranking_attempt, sent, started_at
FROM dashboard_refreshes
WHERE sent > 0 AND julianday(started_at) BETWEEN julianday('now', '-48 hours') AND julianday('now');

CREATE TABLE dashboard_ranking_resets (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    user_id   TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    reset_at  TEXT NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
) STRICT;

ALTER TABLE dashboard_judgements ADD COLUMN rejudge_requested_at TEXT;
