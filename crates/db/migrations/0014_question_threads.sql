-- Ask threads and wider anchors: a question is about a step, lines (or a whole file), an analysed flow or the whole
-- pull request (`kind`), and a follow-up names the first question of its thread (`parent_id`) and repeats its anchor.
-- SQLite cannot change a CHECK in place, so `questions` (no child tables) is rebuilt; every row is copied with the
-- kind of its anchor.
CREATE TABLE questions_new (
    id            TEXT NOT NULL PRIMARY KEY,
    tenant_id     TEXT NOT NULL REFERENCES tenants (id),
    analysis_id   TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    step_id       TEXT,
    path          TEXT,
    side          TEXT CHECK (side IN ('head', 'base')),
    start_line    INTEGER CHECK (start_line >= 1),
    end_line      INTEGER CHECK (end_line >= start_line),
    user_id       TEXT NOT NULL REFERENCES users (id),
    text          TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('pending', 'answering', 'answered', 'failed')),
    answer        TEXT CHECK (answer IS NULL OR json_valid(answer)),
    failure       TEXT,
    job_id        TEXT,
    created_at    TEXT NOT NULL,
    answered_at   TEXT,
    touched_steps TEXT CHECK (touched_steps IS NULL OR json_valid(touched_steps)),
    kind          TEXT NOT NULL CHECK (kind IN ('step', 'lines', 'flow', 'pr')),
    flow_id       TEXT,
    parent_id     TEXT REFERENCES questions (id) ON DELETE CASCADE CHECK (parent_id <> id),
    CHECK ((step_id IS NOT NULL) = (kind = 'step')),
    CHECK ((path IS NOT NULL) = (kind = 'lines')),
    CHECK ((flow_id IS NOT NULL) = (kind = 'flow')),
    CHECK ((path IS NULL) = (side IS NULL)),
    CHECK ((start_line IS NULL) = (end_line IS NULL)),
    CHECK (start_line IS NULL OR kind IN ('step', 'lines'))
) STRICT;

INSERT INTO questions_new (id, tenant_id, analysis_id, step_id, path, side, start_line, end_line, user_id, text, state,
    answer, failure, job_id, created_at, answered_at, touched_steps, kind)
SELECT id, tenant_id, analysis_id, step_id, path, side, start_line, end_line, user_id, text, state, answer, failure,
    job_id, created_at, answered_at, touched_steps, CASE WHEN step_id IS NOT NULL THEN 'step' ELSE 'lines' END
FROM questions ORDER BY rowid;

DROP TABLE questions;
ALTER TABLE questions_new RENAME TO questions;
CREATE INDEX questions_by_analysis ON questions (analysis_id, created_at);
CREATE INDEX questions_by_parent ON questions (parent_id) WHERE parent_id IS NOT NULL;
