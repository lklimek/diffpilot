-- A refresh can be `automatic`: started while the user had My work open (docs/v1.1-dashboard-plan.md §M). SQLite
-- cannot change a CHECK in place, so `dashboard_refreshes` is rebuilt, and with it the tables that reference it:
-- `dashboard_judgements` and `jobs` (with its children `installation_tokens` and `job_events`); every row is copied
-- with its rowid. Runs in the migration's transaction with foreign keys on, where renaming a table rewrites the
-- references to it: the new tables reference each other's new names, and an old table is dropped only once nothing
-- new references it, so no drop cascades into copied rows (the pattern of 0018).

CREATE TABLE dashboard_refreshes_new (
    id               TEXT NOT NULL PRIMARY KEY,
    tenant_id        TEXT NOT NULL REFERENCES tenants (id),
    user_id          TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    trigger          TEXT NOT NULL CHECK (trigger IN ('manual', 'scheduled', 'automatic')),
    state            TEXT NOT NULL CHECK (state IN ('scanning', 'ranking', 'finished', 'failed', 'cancelled')),
    -- {steps: ScanStep[], percent}
    progress         TEXT NOT NULL DEFAULT '{"steps":[],"percent":0}' CHECK (json_valid(progress)),
    -- string[], at most 200 lines
    log              TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(log) AND json_type(log) = 'array'),
    failure_code     TEXT CHECK (failure_code IN ('github_rate_limited', 'github_unavailable', 'sign_in_required', 'timed_out', 'interrupted', 'internal')),
    failure_message  TEXT,
    retry_at         TEXT,
    ranking_state    TEXT CHECK (ranking_state IN ('waiting_for_analysis', 'running', 'paused', 'failed', 'interrupted', 'cancelled', 'finished')),
    judged           INTEGER NOT NULL DEFAULT 0 CHECK (judged >= 0),
    reused           INTEGER NOT NULL DEFAULT 0 CHECK (reused >= 0),
    left_provisional INTEGER NOT NULL DEFAULT 0 CHECK (left_provisional >= 0),
    snapshot         TEXT CHECK (snapshot IS NULL OR json_valid(snapshot)),
    started_at       TEXT NOT NULL,
    scanned_at       TEXT,
    finished_at      TEXT,
    ranking_attempt  INTEGER NOT NULL DEFAULT 0 CHECK (ranking_attempt >= 0),
    sent             INTEGER NOT NULL DEFAULT 0 CHECK (sent >= 0),
    CHECK ((failure_code IS NULL) = (failure_message IS NULL)),
    CHECK ((state = 'failed') = (failure_code IS NOT NULL)),
    CHECK ((state IN ('ranking', 'finished')) = (snapshot IS NOT NULL)),
    CHECK ((state IN ('ranking', 'finished')) = (ranking_state IS NOT NULL)),
    CHECK (state != 'ranking' OR ranking_state IN ('waiting_for_analysis', 'running', 'paused')),
    CHECK (state != 'finished' OR ranking_state IN ('failed', 'interrupted', 'cancelled', 'finished'))
) STRICT;

INSERT INTO dashboard_refreshes_new (rowid, id, tenant_id, user_id, trigger, state, progress, log, failure_code,
    failure_message, retry_at, ranking_state, judged, reused, left_provisional, snapshot, started_at, scanned_at,
    finished_at, ranking_attempt, sent)
SELECT rowid, id, tenant_id, user_id, trigger, state, progress, log, failure_code, failure_message, retry_at,
    ranking_state, judged, reused, left_provisional, snapshot, started_at, scanned_at, finished_at, ranking_attempt,
    sent
FROM dashboard_refreshes ORDER BY rowid;


CREATE TABLE dashboard_judgements_new (
    tenant_id         TEXT NOT NULL REFERENCES tenants (id),
    user_id           TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_key          TEXT NOT NULL,
    -- SHA-256 hex of JUDGE_INPUT_VERSION and the canonical judged inputs
    input_fingerprint TEXT NOT NULL,
    input_version     INTEGER NOT NULL,
    prompt_version    INTEGER NOT NULL,
    body              TEXT NOT NULL CHECK (json_valid(body)),
    refresh_id        TEXT REFERENCES dashboard_refreshes_new (id) ON DELETE SET NULL,
    -- no foreign key: job rows follow their own retention
    job_id            TEXT,
    model             TEXT NOT NULL,
    judged_at         TEXT NOT NULL,
    last_seen_at      TEXT NOT NULL,
    PRIMARY KEY (user_id, item_key)
) STRICT;

INSERT INTO dashboard_judgements_new (rowid, tenant_id, user_id, item_key, input_fingerprint, input_version,
        prompt_version, body, refresh_id, job_id, model, judged_at, last_seen_at)
    SELECT rowid, tenant_id, user_id, item_key, input_fingerprint, input_version, prompt_version, body, refresh_id,
        job_id, model, judged_at, last_seen_at
    FROM dashboard_judgements ORDER BY rowid;

CREATE TABLE jobs_new (
    id                      TEXT NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    analysis_id             TEXT REFERENCES analyses (id) ON DELETE CASCADE,
    refresh_id              TEXT REFERENCES dashboard_refreshes_new (id) ON DELETE CASCADE,
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
DROP TABLE dashboard_judgements;
DROP TABLE dashboard_refreshes;

ALTER TABLE dashboard_refreshes_new RENAME TO dashboard_refreshes;
ALTER TABLE dashboard_judgements_new RENAME TO dashboard_judgements;
ALTER TABLE jobs_new RENAME TO jobs;
ALTER TABLE installation_tokens_new RENAME TO installation_tokens;
ALTER TABLE job_events_new RENAME TO job_events;

CREATE UNIQUE INDEX dashboard_refreshes_one_live_per_user ON dashboard_refreshes (user_id)
    WHERE state IN ('scanning', 'ranking');
CREATE INDEX dashboard_refreshes_by_user ON dashboard_refreshes (user_id, started_at);
CREATE INDEX dashboard_judgements_by_judged_at ON dashboard_judgements (user_id, judged_at);
CREATE INDEX dashboard_judgements_by_last_seen ON dashboard_judgements (last_seen_at);

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
