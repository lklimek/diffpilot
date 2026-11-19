-- Jobs without a pull request (ADR-018): a job belongs to exactly one analysis or one dashboard refresh. SQLite cannot
-- relax NOT NULL or a CHECK in place, so `jobs` and its children are rebuilt and every row is copied: v1 jobs, their
-- events, usage and pending token revocations survive. Each stored spec moves its analysis, pull request and
-- repository into `target` (`JobTarget::PullRequest`). Runs in the migration's transaction with foreign keys on:
-- the new tables are filled first, the old ones dropped children first, then the new ones renamed (renaming
-- rewrites the children's references).

CREATE TABLE jobs_new (
    id                      TEXT NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    analysis_id             TEXT REFERENCES analyses (id) ON DELETE CASCADE,
    refresh_id              TEXT REFERENCES dashboard_refreshes (id) ON DELETE CASCADE,
    kind                    TEXT NOT NULL CHECK (kind IN ('plan', 'pr_context', 'analyse_flow', 'ask', 'judge_work')),
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
                                OR (kind = 'judge_work' AND stage LIKE 'judge:%')), -- JobStage::key
    generation              INTEGER NOT NULL DEFAULT 0 CHECK (generation BETWEEN 0 AND 2), -- 0 first run, 1..2 repairs
    round                   INTEGER NOT NULL DEFAULT 0, -- analysis round, or the refresh's ranking attempt, at enqueue
    github_access_granted   INTEGER NOT NULL DEFAULT 0 CHECK (github_access_granted IN (0, 1)), -- last claim had a token
    flow_id                 TEXT,
    question_id             TEXT,
    created_at              TEXT NOT NULL,
    claimed_at              TEXT,
    finished_at             TEXT,
    CHECK ((analysis_id IS NULL) <> (refresh_id IS NULL)),
    CHECK ((kind = 'judge_work') = (refresh_id IS NOT NULL))
) STRICT;

-- Encrypted installation tokens per job attempt, kept until their revocation is confirmed (ADR-005).
CREATE TABLE installation_tokens_new (
    job_id     TEXT NOT NULL REFERENCES jobs_new (id),
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    attempt    INTEGER NOT NULL,
    ciphertext BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY (job_id, attempt)
) STRICT;

-- Bounded event log (the last 500 events per job are kept).
CREATE TABLE job_events_new (
    job_id    TEXT NOT NULL REFERENCES jobs_new (id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    attempt   INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    at        TEXT NOT NULL,
    body      TEXT NOT NULL CHECK (json_valid(body)),
    PRIMARY KEY (job_id, attempt, seq)
) STRICT;

INSERT INTO jobs_new (id, tenant_id, analysis_id, refresh_id, kind, priority, state, spec, credential_id, attempt,
        max_attempts, lease_expires_at, job_token_hash, cancel_requested, resumes_at, ready_output,
        ready_output_attempt, result, stage, generation, round, github_access_granted, flow_id, question_id,
        created_at, claimed_at, finished_at)
    SELECT j.id, j.tenant_id, j.analysis_id, NULL, j.kind, j.priority, j.state,
        CASE WHEN json_type(j.spec, '$.target') IS NULL THEN
            json_set(json_remove(j.spec, '$.analysis_id', '$.pull_request_id', '$.repo'), '$.target',
                json_object('type', 'pull_request', 'analysis_id', j.analysis_id,
                    'pull_request_id', (SELECT a.pull_request_id FROM analyses a WHERE a.id = j.analysis_id),
                    'repo', json(COALESCE(json_extract(j.spec, '$.repo'), 'null'))))
        ELSE j.spec END,
        j.credential_id, j.attempt, j.max_attempts, j.lease_expires_at, j.job_token_hash, j.cancel_requested,
        j.resumes_at, j.ready_output, j.ready_output_attempt, j.result, j.stage, j.generation, j.round,
        j.github_access_granted, j.flow_id, j.question_id, j.created_at, j.claimed_at, j.finished_at
    FROM jobs j ORDER BY j.rowid;

INSERT INTO installation_tokens_new (job_id, tenant_id, attempt, ciphertext, expires_at)
    SELECT job_id, tenant_id, attempt, ciphertext, expires_at FROM installation_tokens ORDER BY rowid;

INSERT INTO job_events_new (job_id, tenant_id, attempt, seq, at, body)
    SELECT job_id, tenant_id, attempt, seq, at, body FROM job_events ORDER BY rowid;

DROP TABLE installation_tokens;
DROP TABLE job_events;
DROP TABLE jobs;

ALTER TABLE jobs_new RENAME TO jobs;
ALTER TABLE installation_tokens_new RENAME TO installation_tokens;
ALTER TABLE job_events_new RENAME TO job_events;

-- One running job per agent session (analysis) and per LLM credential (ADR-003, ADR-004); the credential index
-- also keeps judge jobs and analysis jobs apart.
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
