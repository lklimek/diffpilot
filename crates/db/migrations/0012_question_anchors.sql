-- Questions about lines (Files view): a question is anchored to a step (optionally some of its lines) or to lines of
-- a file without a step. SQLite cannot relax NOT NULL on `step_id` in place, so `questions` (no child tables) is
-- rebuilt and every row copied as a step anchor. `start_line`/`end_line` narrow a step anchor or give the range of a
-- lines anchor (absent: the whole step or file).
CREATE TABLE questions_new (
    id          TEXT NOT NULL PRIMARY KEY,
    tenant_id   TEXT NOT NULL REFERENCES tenants (id),
    analysis_id TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    step_id     TEXT,
    path        TEXT,
    side        TEXT CHECK (side IN ('head', 'base')),
    start_line  INTEGER CHECK (start_line >= 1),
    end_line    INTEGER CHECK (end_line >= start_line),
    user_id     TEXT NOT NULL REFERENCES users (id),
    text        TEXT NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('pending', 'answering', 'answered', 'failed')),
    answer      TEXT CHECK (answer IS NULL OR json_valid(answer)),
    failure     TEXT,
    job_id      TEXT,
    created_at  TEXT NOT NULL,
    answered_at TEXT,
    CHECK ((step_id IS NULL) <> (path IS NULL)),
    CHECK ((path IS NULL) = (side IS NULL)),
    CHECK ((start_line IS NULL) = (end_line IS NULL))
) STRICT;

INSERT INTO questions_new (id, tenant_id, analysis_id, step_id, user_id, text, state, answer, failure, job_id,
    created_at, answered_at)
SELECT id, tenant_id, analysis_id, step_id, user_id, text, state, answer, failure, job_id, created_at, answered_at
FROM questions ORDER BY rowid;

DROP TABLE questions;
ALTER TABLE questions_new RENAME TO questions;
CREATE INDEX questions_by_analysis ON questions (analysis_id, created_at);
