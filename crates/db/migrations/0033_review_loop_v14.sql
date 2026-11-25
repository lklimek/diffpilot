ALTER TABLE comment_items ADD COLUMN status_comment INTEGER NOT NULL DEFAULT 0 CHECK (status_comment IN (0, 1));

-- The decision row uses 'question' for these origins. The origin supplies the wire variant.
CREATE TABLE comment_decision_origins (
    decision_id         TEXT NOT NULL PRIMARY KEY REFERENCES comment_decisions (id) ON DELETE CASCADE,
    origin              TEXT NOT NULL CHECK (origin IN ('imported_github', 'follows_primary')),
    primary_decision_id TEXT REFERENCES comment_decisions (id),
    by_login            TEXT,
    recorded_at         TEXT,
    evidence_url        TEXT,
    quote               TEXT
) STRICT;
CREATE TRIGGER comment_decision_origins_append_only BEFORE UPDATE ON comment_decision_origins
BEGIN SELECT RAISE(ABORT, 'decision origins are append-only'); END;
