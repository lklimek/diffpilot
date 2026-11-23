-- Ended snoozes are kept (owner rule: keep all data): `ended_at` and `end_reason` mark a snooze that ended at its date
-- (`date`), on a review request to the user or a mention of the user newer than it (`review_request`, `mention`), or
-- when the user woke it (`woken`). At most one snooze per user and item has not ended; a new snooze of the item
-- replaces that one. SQLite cannot change a primary key in place, so `dashboard_snoozes` (no child tables) is rebuilt
-- and every row copied as not ended.
CREATE TABLE dashboard_snoozes_new (
    id         INTEGER PRIMARY KEY,
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_key   TEXT NOT NULL,
    snoozed_at TEXT NOT NULL,
    until      TEXT NOT NULL CHECK (until > snoozed_at),
    ended_at   TEXT,
    end_reason TEXT CHECK (end_reason IN ('date', 'review_request', 'mention', 'woken')),
    CHECK ((ended_at IS NULL) = (end_reason IS NULL))
) STRICT;

INSERT INTO dashboard_snoozes_new (tenant_id, user_id, item_key, snoozed_at, until)
SELECT tenant_id, user_id, item_key, snoozed_at, until FROM dashboard_snoozes ORDER BY rowid;

DROP TABLE dashboard_snoozes;
ALTER TABLE dashboard_snoozes_new RENAME TO dashboard_snoozes;
CREATE UNIQUE INDEX dashboard_snoozes_not_ended ON dashboard_snoozes (tenant_id, user_id, item_key)
    WHERE ended_at IS NULL;
