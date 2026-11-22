-- `verify_comments` jobs and Ask about review-comment items (ADR-023, docs/review-workflow-plan.md §4.2, §4.3 T-L4).
-- SQLite cannot change a CHECK or NOT NULL in place, so `questions`, `comment_passes` and `jobs` (with its children
-- `installation_tokens` and `job_events`) are rebuilt in this order; every row is copied with its rowid. Runs in the
-- migration's transaction with foreign keys on: the new tables reference their own new names (renaming rewrites
-- them), and an old table is dropped only once nothing new references it, so no drop cascades into copied rows.
--
-- questions: a `comment_item` anchor names its item; without an analysis of the head the question belongs to the
--   pull request only (`analysis_id` NULL) and a `verify_comments` question pass answers it. `pull_request_id` is
--   stored for every question (backfilled from the analysis) so a pull request's comment questions list in one read.
-- comment_passes: `user_id` NULL = a pass nobody is charged for (opened or read without a user); `items` lists the
--   items sent with the fingerprint each was sent at; `question_id` names the question a question pass answers.
-- jobs: a job belongs to exactly one analysis, dashboard refresh or assessment pass (`pass_id`, kind
--   `verify_comments`, stage `verify:<pass>`).

CREATE TABLE questions_new (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    analysis_id     TEXT REFERENCES analyses (id) ON DELETE CASCADE,
    pull_request_id INTEGER REFERENCES pull_requests (id) ON DELETE CASCADE,
    step_id         TEXT,
    path            TEXT,
    side            TEXT CHECK (side IN ('head', 'base')),
    start_line      INTEGER CHECK (start_line >= 1),
    end_line        INTEGER CHECK (end_line >= start_line),
    user_id         TEXT NOT NULL REFERENCES users (id),
    text            TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('pending', 'answering', 'answered', 'failed')),
    answer          TEXT CHECK (answer IS NULL OR json_valid(answer)),
    failure         TEXT,
    job_id          TEXT,
    created_at      TEXT NOT NULL,
    answered_at     TEXT,
    touched_steps   TEXT CHECK (touched_steps IS NULL OR json_valid(touched_steps)),
    kind            TEXT NOT NULL CHECK (kind IN ('step', 'lines', 'flow', 'pr', 'comment_item')),
    flow_id         TEXT,
    parent_id       TEXT REFERENCES questions_new (id) ON DELETE CASCADE CHECK (parent_id <> id),
    item_id         TEXT REFERENCES comment_items (id) ON DELETE CASCADE,
    CHECK ((step_id IS NOT NULL) = (kind = 'step')),
    CHECK ((path IS NOT NULL) = (kind = 'lines')),
    CHECK ((flow_id IS NOT NULL) = (kind = 'flow')),
    CHECK ((item_id IS NOT NULL) = (kind = 'comment_item')),
    CHECK ((path IS NULL) = (side IS NULL)),
    CHECK ((start_line IS NULL) = (end_line IS NULL)),
    CHECK (start_line IS NULL OR kind IN ('step', 'lines')),
    CHECK (analysis_id IS NOT NULL OR (kind = 'comment_item' AND pull_request_id IS NOT NULL))
) STRICT;

INSERT INTO questions_new (rowid, id, tenant_id, analysis_id, pull_request_id, step_id, path, side, start_line,
    end_line, user_id, text, state, answer, failure, job_id, created_at, answered_at, touched_steps, kind, flow_id,
    parent_id)
SELECT q.rowid, q.id, q.tenant_id, q.analysis_id, (SELECT a.pull_request_id FROM analyses a WHERE a.id = q.analysis_id),
    q.step_id, q.path, q.side, q.start_line, q.end_line, q.user_id, q.text, q.state, q.answer, q.failure, q.job_id,
    q.created_at, q.answered_at, q.touched_steps, q.kind, q.flow_id, q.parent_id
FROM questions q ORDER BY q.rowid;

DROP TABLE questions;
ALTER TABLE questions_new RENAME TO questions;
CREATE INDEX questions_by_analysis ON questions (analysis_id, created_at);
CREATE INDEX questions_by_parent ON questions (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX questions_by_pull_request ON questions (pull_request_id, user_id) WHERE kind = 'comment_item';

CREATE TABLE comment_passes_new (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES comment_prs (pull_request_id) ON DELETE CASCADE,
    user_id         TEXT REFERENCES users (id) ON DELETE CASCADE,
    cause           TEXT NOT NULL CHECK (cause IN ('opened', 'read', 'assess_again', 'question', 'watch')),
    head            TEXT NOT NULL,
    items           INTEGER NOT NULL CHECK (items >= 0),
    state           TEXT NOT NULL CHECK (state IN ('queued', 'running', 'finished', 'failed')),
    job_id          TEXT,
    created_at      TEXT NOT NULL,
    finished_at     TEXT,
    sent            TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(sent) AND json_type(sent) = 'array'), -- SentItem
    question_id     TEXT REFERENCES questions (id) ON DELETE SET NULL,
    CHECK ((state IN ('finished', 'failed')) = (finished_at IS NOT NULL))
) STRICT;

INSERT INTO comment_passes_new (rowid, id, tenant_id, pull_request_id, user_id, cause, head, items, state, job_id,
    created_at, finished_at)
SELECT rowid, id, tenant_id, pull_request_id, user_id, cause, head, items, state, job_id, created_at, finished_at
FROM comment_passes ORDER BY rowid;

DROP TABLE comment_passes;
ALTER TABLE comment_passes_new RENAME TO comment_passes;
CREATE UNIQUE INDEX comment_passes_one_live_per_pr ON comment_passes (pull_request_id)
    WHERE state IN ('queued', 'running');
CREATE INDEX comment_passes_by_user ON comment_passes (user_id, created_at);

CREATE TABLE jobs_new (
    id                      TEXT NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    analysis_id             TEXT REFERENCES analyses (id) ON DELETE CASCADE,
    refresh_id              TEXT REFERENCES dashboard_refreshes (id) ON DELETE CASCADE,
    pass_id                 TEXT REFERENCES comment_passes (id) ON DELETE CASCADE,
    kind                    TEXT NOT NULL CHECK (kind IN ('plan', 'pr_context', 'analyse_flow', 'ask', 'judge_work', 'verify_comments')),
    priority                INTEGER NOT NULL CHECK (priority IN (0, 1)), -- 0 interactive, 1 batch
    state                   TEXT NOT NULL CHECK (state IN ('queued', 'claimed', 'running', 'succeeded', 'failed', 'cancelled', 'paused')),
    spec                    TEXT NOT NULL CHECK (json_valid(spec)),
    credential_id           TEXT NOT NULL,
    attempt                 INTEGER NOT NULL DEFAULT 0,
    max_attempts            INTEGER NOT NULL DEFAULT 3,
    lease_expires_at        TEXT,
    job_token_hash          BLOB,
    cancel_requested        INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
    resumes_at              TEXT,
    ready_output            TEXT CHECK (ready_output IS NULL OR json_valid(ready_output)), -- posted before compaction
    ready_output_attempt    INTEGER CHECK ((ready_output IS NULL) = (ready_output_attempt IS NULL)),
    result                  TEXT CHECK (result IS NULL OR json_valid(result)),
    stage                   TEXT NOT NULL CHECK ((kind = 'plan' AND stage LIKE 'plan:%')
                                OR (kind = 'pr_context' AND stage = 'pr_context')
                                OR (kind = 'analyse_flow' AND stage LIKE 'flow:%')
                                OR (kind = 'ask' AND stage LIKE 'ask:%')
                                OR (kind = 'judge_work' AND stage LIKE 'judge:%')
                                OR (kind = 'verify_comments' AND stage LIKE 'verify:%')), -- JobStage::key
    generation              INTEGER NOT NULL DEFAULT 0 CHECK (generation BETWEEN 0 AND 2), -- 0 first run, 1..2 repairs
    round                   INTEGER NOT NULL DEFAULT 0, -- analysis round, or the refresh's ranking attempt, at enqueue
    github_access_granted   INTEGER NOT NULL DEFAULT 0 CHECK (github_access_granted IN (0, 1)), -- last claim had a token
    flow_id                 TEXT,
    question_id             TEXT,
    created_at              TEXT NOT NULL,
    claimed_at              TEXT,
    finished_at             TEXT,
    CHECK ((analysis_id IS NOT NULL) + (refresh_id IS NOT NULL) + (pass_id IS NOT NULL) = 1),
    CHECK ((kind = 'judge_work') = (refresh_id IS NOT NULL)),
    CHECK ((kind = 'verify_comments') = (pass_id IS NOT NULL))
) STRICT;

CREATE TABLE installation_tokens_new (
    job_id     TEXT NOT NULL REFERENCES jobs_new (id),
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    attempt    INTEGER NOT NULL,
    ciphertext BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY (job_id, attempt)
) STRICT;

CREATE TABLE job_events_new (
    job_id    TEXT NOT NULL REFERENCES jobs_new (id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    attempt   INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    at        TEXT NOT NULL,
    body      TEXT NOT NULL CHECK (json_valid(body)),
    PRIMARY KEY (job_id, attempt, seq)
) STRICT;

INSERT INTO jobs_new (rowid, id, tenant_id, analysis_id, refresh_id, kind, priority, state, spec, credential_id,
        attempt, max_attempts, lease_expires_at, job_token_hash, cancel_requested, resumes_at, ready_output,
        ready_output_attempt, result, stage, generation, round, github_access_granted, flow_id, question_id,
        created_at, claimed_at, finished_at)
    SELECT rowid, id, tenant_id, analysis_id, refresh_id, kind, priority, state, spec, credential_id, attempt,
        max_attempts, lease_expires_at, job_token_hash, cancel_requested, resumes_at, ready_output,
        ready_output_attempt, result, stage, generation, round, github_access_granted, flow_id, question_id,
        created_at, claimed_at, finished_at
    FROM jobs ORDER BY rowid;

INSERT INTO installation_tokens_new (rowid, job_id, tenant_id, attempt, ciphertext, expires_at)
    SELECT rowid, job_id, tenant_id, attempt, ciphertext, expires_at FROM installation_tokens ORDER BY rowid;

INSERT INTO job_events_new (rowid, job_id, tenant_id, attempt, seq, at, body)
    SELECT rowid, job_id, tenant_id, attempt, seq, at, body FROM job_events ORDER BY rowid;

DROP TABLE installation_tokens;
DROP TABLE job_events;
DROP TABLE jobs;

ALTER TABLE jobs_new RENAME TO jobs;
ALTER TABLE installation_tokens_new RENAME TO installation_tokens;
ALTER TABLE job_events_new RENAME TO job_events;

-- One running job per agent session (analysis) and per LLM credential (ADR-003, ADR-004); the credential index
-- also keeps judge, verify and analysis jobs apart.
CREATE UNIQUE INDEX jobs_one_running_per_analysis ON jobs (analysis_id) WHERE state IN ('claimed', 'running');
CREATE UNIQUE INDEX jobs_one_running_per_credential ON jobs (credential_id) WHERE state IN ('claimed', 'running');
CREATE INDEX jobs_claim_order ON jobs (state, priority, created_at);
-- Idempotent submit: one live or succeeded job per pipeline stage, generation (repair run) and round (Retry); a
-- failed or cancelled one may be submitted again.
CREATE UNIQUE INDEX jobs_one_live_per_stage ON jobs (analysis_id, stage, generation, round)
    WHERE state NOT IN ('failed', 'cancelled');
CREATE INDEX jobs_by_analysis ON jobs (analysis_id, state);
-- The same for judge batches: one live or succeeded job per refresh, batch and ranking attempt (Retry ranking).
CREATE UNIQUE INDEX jobs_one_live_per_judge_stage ON jobs (refresh_id, stage, generation, round)
    WHERE refresh_id IS NOT NULL AND state NOT IN ('failed', 'cancelled');
CREATE INDEX jobs_by_refresh ON jobs (refresh_id, state) WHERE refresh_id IS NOT NULL;
-- One live or succeeded job per assessment pass.
CREATE UNIQUE INDEX jobs_one_live_per_pass ON jobs (pass_id, stage, generation)
    WHERE pass_id IS NOT NULL AND state NOT IN ('failed', 'cancelled');

-- Different analyses of one PR share a single agent thread, even across credentials.
CREATE TRIGGER jobs_one_running_per_pr_insert BEFORE INSERT ON jobs
WHEN NEW.state IN ('claimed', 'running') AND NEW.analysis_id IS NOT NULL AND EXISTS (
    SELECT 1 FROM jobs j JOIN analyses a ON a.id = j.analysis_id
    WHERE j.id <> NEW.id AND j.state IN ('claimed', 'running')
      AND a.pull_request_id = (SELECT pull_request_id FROM analyses WHERE id = NEW.analysis_id)
)
BEGIN SELECT RAISE(ABORT, 'one running job per pull request'); END;

-- Different analyses of one PR share a single agent thread, even across credentials.
CREATE TRIGGER jobs_one_running_per_pr_update BEFORE UPDATE OF state, analysis_id ON jobs
WHEN NEW.state IN ('claimed', 'running') AND NEW.analysis_id IS NOT NULL AND EXISTS (
    SELECT 1 FROM jobs j JOIN analyses a ON a.id = j.analysis_id
    WHERE j.id <> NEW.id AND j.state IN ('claimed', 'running')
      AND a.pull_request_id = (SELECT pull_request_id FROM analyses WHERE id = NEW.analysis_id)
)
BEGIN SELECT RAISE(ABORT, 'one running job per pull request'); END;
