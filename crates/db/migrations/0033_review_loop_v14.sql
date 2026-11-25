ALTER TABLE comment_items ADD COLUMN status_comment INTEGER NOT NULL DEFAULT 0 CHECK (status_comment IN (0, 1));

-- The decision row uses 'question' for these origins. The origin supplies the wire variant.
CREATE TABLE comment_decision_origins (
    decision_id         TEXT NOT NULL PRIMARY KEY REFERENCES comment_decisions (id) ON DELETE CASCADE,
    origin              TEXT NOT NULL CHECK (origin IN ('imported_github', 'follows_primary')),
    primary_decision_id TEXT REFERENCES comment_decisions (id),
    by_login            TEXT,
    by_github_id        INTEGER CHECK (by_github_id > 0),
    relay_login         TEXT,
    recorded_at         TEXT,
    evidence_url        TEXT,
    quote               TEXT,
    CHECK (
        (origin = 'imported_github' AND primary_decision_id IS NULL
            AND by_login IS NOT NULL AND recorded_at IS NOT NULL AND evidence_url IS NOT NULL
            AND quote IS NOT NULL AND length(quote) BETWEEN 1 AND 600)
        OR
        (origin = 'follows_primary' AND primary_decision_id IS NOT NULL
            AND by_login IS NULL AND by_github_id IS NULL AND relay_login IS NULL
            AND recorded_at IS NULL AND evidence_url IS NULL AND quote IS NULL)
    )
) STRICT;
CREATE TRIGGER comment_decision_origins_primary_scope BEFORE INSERT ON comment_decision_origins
WHEN NEW.primary_decision_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM comment_decisions AS follower
    JOIN comment_decisions AS primary_decision
      ON primary_decision.id = NEW.primary_decision_id AND primary_decision.tenant_id = follower.tenant_id
    JOIN comment_items AS follower_item
      ON follower_item.id = follower.item_id AND follower_item.tenant_id = follower.tenant_id
    JOIN comment_items AS primary_item
      ON primary_item.id = primary_decision.item_id AND primary_item.tenant_id = primary_decision.tenant_id
    WHERE follower.id = NEW.decision_id AND primary_item.pull_request_id = follower_item.pull_request_id
      AND primary_item.id <> follower_item.id
)
BEGIN SELECT RAISE(ABORT, 'primary decision must belong to another item of the same tenant and pull request'); END;
CREATE TRIGGER comment_decision_origins_append_only BEFORE UPDATE ON comment_decision_origins
BEGIN SELECT RAISE(ABORT, 'decision origins are append-only'); END;
