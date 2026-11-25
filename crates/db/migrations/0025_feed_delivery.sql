-- Decision feed and notification delivery (ADR-023, docs/review-workflow-plan.md §5.2, §5.5). Upgrades in place:
-- two nullable columns, one new table, and `comment_effect_reports` rebuilt (SQLite cannot change a UNIQUE in
-- place; nothing references it; every row is copied with its id).

-- The token whose claim lapsed when the feed task returned the effect: that token's late report answers
-- `lease_expired`, a report of any other non-claimer `claimed`. Cleared by the next claim or an explicit release.
ALTER TABLE comment_effects ADD COLUMN lapsed_token_id TEXT REFERENCES access_tokens (id);

-- An idempotency key is scoped per effect: the same key on another effect is another report.
CREATE TABLE comment_effect_reports_new (
    id              INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    effect_id       TEXT NOT NULL REFERENCES comment_effects (id) ON DELETE CASCADE,
    token_id        TEXT NOT NULL REFERENCES access_tokens (id),
    outcome         TEXT NOT NULL CHECK (outcome IN ('done', 'failed', 'released')),
    reference       TEXT,
    message         TEXT,
    idempotency_key TEXT NOT NULL,
    at              TEXT NOT NULL,
    UNIQUE (token_id, effect_id, idempotency_key)
) STRICT;
INSERT INTO comment_effect_reports_new (id, tenant_id, effect_id, token_id, outcome, reference, message,
    idempotency_key, at)
SELECT id, tenant_id, effect_id, token_id, outcome, reference, message, idempotency_key, at
FROM comment_effect_reports;
DROP TABLE comment_effect_reports;
ALTER TABLE comment_effect_reports_new RENAME TO comment_effect_reports;
CREATE INDEX comment_effect_reports_by_effect ON comment_effect_reports (effect_id, id);

-- round_executed: the newest effect update among the decisions the event announces. A round is announced once per
-- user (the index), so a repeated or recovered check adds nothing; events written before this column count with
-- their `at`.
ALTER TABLE user_events ADD COLUMN round_through TEXT;
CREATE UNIQUE INDEX user_events_round_once ON user_events (user_id, pull_request_id, round_through)
    WHERE round_through IS NOT NULL;

-- One row per message sent to a channel or push target (tests and retries included); the hourly limit counts them,
-- so it holds across restarts and for a channel removed and saved again. `target`: `channel:<user id>/<kind>` or
-- `push:<token id>`.
CREATE TABLE notification_sends (
    id        INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    target    TEXT NOT NULL CHECK (length(target) > 0),
    at        TEXT NOT NULL
) STRICT;
CREATE INDEX notification_sends_by_target ON notification_sends (target, at);
