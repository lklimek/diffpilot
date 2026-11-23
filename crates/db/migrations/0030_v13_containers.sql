-- v1.3 requires an empty runtime database; never purge a live deployment.
CREATE TEMP TABLE v13_empty_guard (n INTEGER NOT NULL CHECK (n = 0));
INSERT INTO v13_empty_guard SELECT count(*) FROM llm_credentials;
INSERT INTO v13_empty_guard SELECT count(*) FROM installation_tokens;
INSERT INTO v13_empty_guard SELECT count(*) FROM job_events;
INSERT INTO v13_empty_guard SELECT count(*) FROM jobs;
INSERT INTO v13_empty_guard SELECT count(*) FROM comment_assessments;
INSERT INTO v13_empty_guard SELECT count(*) FROM working_sessions;
DROP TABLE v13_empty_guard;

DROP TABLE installation_tokens;
DROP TABLE job_events;
DROP TABLE jobs;
DROP TABLE llm_credentials;
DROP TABLE comment_assessments;
DROP TABLE working_sessions;

-- KL1: common status only; credential modes, tokens and rotation state are absent.
CREATE TABLE llm_credentials (
    user_id           TEXT NOT NULL PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    tenant_id         TEXT NOT NULL REFERENCES tenants (id),
    state             TEXT NOT NULL CHECK (state IN ('linking', 'linked', 'relink_needed')),
    relink_needed_at  TEXT,
    relink_reason     TEXT CHECK (relink_reason IN ('terminal', 'quarantined')),
    plan              TEXT,
    label             TEXT,
    paused_until      TEXT,
    last_call_at      TEXT,
    last_call_ok      INTEGER CHECK (last_call_ok IN (0, 1)),
    last_call_message TEXT,
    last_call_failure TEXT CHECK (last_call_failure IN ('usage_limited', 'provider_failed', 'output_not_json', 'credential_unusable', 'checkout_failed', 'timeout', 'internal', 'configuration', 'worker_unavailable', 'worker_lost', 'credential_missing', 'credential_relink', 'model_unavailable', 'upstream_rejected', 'report_version_unsupported', 'invalid_json_schema')),
    CHECK ((state = 'relink_needed') = (relink_reason IS NOT NULL)),
    CHECK ((last_call_at IS NULL) = (last_call_ok IS NULL)),
    CHECK (last_call_failure IS NULL OR last_call_ok = 0)
) STRICT;

ALTER TABLE analyses ADD COLUMN payer_id TEXT REFERENCES users (id);
ALTER TABLE pull_requests DROP COLUMN agent_thread_id;
ALTER TABLE pull_requests DROP COLUMN analytic_generation;
ALTER TABLE pull_requests DROP COLUMN session_state;

CREATE TABLE analytic_threads (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES pull_requests (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    thread_id TEXT,
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    state TEXT NOT NULL CHECK (state IN ('none', 'active', 'drop_requested', 'dropped')),
    last_context_tokens INTEGER CHECK (last_context_tokens >= 0),
    last_context_generation INTEGER CHECK (last_context_generation >= 0),
    PRIMARY KEY (pull_request_id, user_id),
    CHECK ((last_context_tokens IS NULL) = (last_context_generation IS NULL)),
    CHECK (last_context_generation IS NULL OR last_context_generation = generation),
    CHECK (state <> 'active' OR thread_id IS NOT NULL)
) STRICT;

-- Ask and review Ask keep distinct immutable thread policies for one PR/user.
CREATE TABLE working_sessions (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES pull_requests (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    agent_thread_id TEXT NOT NULL,
    analytic_generation INTEGER NOT NULL CHECK (analytic_generation >= 0),
    seed TEXT NOT NULL DEFAULT 'context_pack' CHECK (seed IN ('fork', 'context_pack')),
    thread_kind TEXT NOT NULL DEFAULT 'ask' CHECK (thread_kind IN ('ask', 'verify')),
    preflight_state TEXT NOT NULL DEFAULT 'pending' CHECK (preflight_state IN ('not_needed', 'pending', 'done')),
    seed_generation INTEGER NOT NULL DEFAULT 0 CHECK (seed_generation >= 0 AND seed_generation = analytic_generation),
    PRIMARY KEY (pull_request_id,user_id,thread_kind),
    UNIQUE (pull_request_id,user_id,agent_thread_id)
) STRICT;

CREATE TABLE comment_sources (
    id INTEGER NOT NULL PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES comment_prs (pull_request_id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('review_body', 'pr_comment')),
    url TEXT NOT NULL,
    author TEXT NOT NULL,
    body TEXT NOT NULL CHECK (length(body) > 0),
    body_hash TEXT NOT NULL CHECK (length(body_hash) = 64 AND body_hash NOT GLOB '*[^0-9a-f]*'),
    role TEXT NOT NULL CHECK (role IN ('context', 'ignored', 'source')),
    read_at TEXT,
    UNIQUE (pull_request_id, url, body_hash)
) STRICT;
CREATE INDEX comment_sources_by_pr ON comment_sources (pull_request_id, role, read_at);
ALTER TABLE comment_items ADD COLUMN source_id INTEGER REFERENCES comment_sources (id);
ALTER TABLE comment_items ADD COLUMN quote TEXT CHECK (quote IS NULL OR length(quote) BETWEEN 1 AND 600);
ALTER TABLE comment_items ADD COLUMN duplicate_of TEXT REFERENCES comment_items (id) CHECK (duplicate_of <> id);
CREATE INDEX comment_items_by_source ON comment_items (source_id);

CREATE TABLE comment_assessments (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    item_id TEXT NOT NULL REFERENCES comment_items (id) ON DELETE CASCADE,
    job_id TEXT NOT NULL,
    head TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    body TEXT NOT NULL CHECK (json_valid(body)),
    prompt_version INTEGER NOT NULL,
    assessed_at TEXT NOT NULL,
    CHECK (json_extract(body, '$.validity') IN ('valid', 'partly_valid', 'invalid', 'already_fixed')),
    CHECK (json_type(body, '$.pre_existing') IS NULL OR json_type(body, '$.pre_existing') IN ('null', 'true', 'false')),
    CHECK (json_type(body, '$.code') IS NULL OR json_type(body, '$.code') IN ('null', 'object')),
    CHECK (json_extract(body, '$.fix_size') IS NULL OR json_extract(body, '$.fix_size') IN ('XS', 'S', 'M')),
    CHECK (json_type(body, '$.evidence_text') IS NULL OR json_type(body, '$.evidence_text') IN ('null', 'text')),
    CHECK (json_type(body, '$.recorded_decision') IS NULL OR json_type(body, '$.recorded_decision') IN ('null', 'object'))
) STRICT;
CREATE INDEX comment_assessments_by_item ON comment_assessments (item_id, id);

CREATE TABLE jobs (
    id                      TEXT NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    analysis_id             TEXT REFERENCES analyses (id) ON DELETE CASCADE,
    refresh_id              TEXT REFERENCES dashboard_refreshes (id) ON DELETE CASCADE,
    pass_id                 TEXT REFERENCES comment_passes (id) ON DELETE CASCADE,
    kind                    TEXT NOT NULL CHECK (kind IN ('plan', 'pr_context', 'analyse_flow', 'ask', 'judge_work', 'verify_comments')),
    priority                INTEGER NOT NULL CHECK (priority IN (0, 1)), -- 0 interactive, 1 batch
    state                   TEXT NOT NULL CHECK (state IN ('queued', 'claimed', 'running', 'succeeded', 'failed', 'cancelled', 'paused')),
    spec                    TEXT NOT NULL CHECK (json_valid(spec)),
    payer_id                TEXT NOT NULL REFERENCES users (id),
    pull_request_id         INTEGER REFERENCES pull_requests (id) ON DELETE CASCADE,
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
    CHECK ((kind = 'verify_comments') = (pass_id IS NOT NULL)),
    CHECK ((kind = 'judge_work') = (pull_request_id IS NULL)),
    CHECK (attempt >= 0 AND max_attempts > 0),
    CHECK (round >= 0)
) STRICT;

CREATE TABLE installation_tokens (
    job_id     TEXT NOT NULL REFERENCES jobs (id),
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    attempt    INTEGER NOT NULL,
    ciphertext BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY (job_id, attempt)
) STRICT;

CREATE TABLE job_events (
    job_id    TEXT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    attempt   INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    at        TEXT NOT NULL,
    body      TEXT NOT NULL CHECK (json_valid(body)),
    PRIMARY KEY (job_id, attempt, seq)
) STRICT;

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

CREATE UNIQUE INDEX jobs_one_running_per_pr_payer ON jobs (pull_request_id, payer_id)
    WHERE pull_request_id IS NOT NULL AND state IN ('claimed', 'running');
CREATE UNIQUE INDEX jobs_one_running_per_judge_payer ON jobs (payer_id)
    WHERE kind = 'judge_work' AND state IN ('claimed', 'running');
CREATE TRIGGER jobs_container_target_insert BEFORE INSERT ON jobs
WHEN (NEW.analysis_id IS NOT NULL AND NEW.pull_request_id IS NOT
          (SELECT pull_request_id FROM analyses WHERE id = NEW.analysis_id))
  OR (NEW.pass_id IS NOT NULL AND NEW.pull_request_id IS NOT
          (SELECT pull_request_id FROM comment_passes WHERE id = NEW.pass_id))
BEGIN SELECT RAISE(ABORT, 'job container does not match target'); END;
CREATE TRIGGER jobs_container_target_update BEFORE UPDATE OF analysis_id, pass_id, pull_request_id, kind ON jobs
WHEN (NEW.analysis_id IS NOT NULL AND NEW.pull_request_id IS NOT
          (SELECT pull_request_id FROM analyses WHERE id = NEW.analysis_id))
  OR (NEW.pass_id IS NOT NULL AND NEW.pull_request_id IS NOT
          (SELECT pull_request_id FROM comment_passes WHERE id = NEW.pass_id))
BEGIN SELECT RAISE(ABORT, 'job container does not match target'); END;

CREATE TRIGGER comment_items_source_insert BEFORE INSERT ON comment_items
WHEN (NEW.source_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM comment_sources s WHERE s.id=NEW.source_id AND s.pull_request_id=NEW.pull_request_id AND s.tenant_id=NEW.tenant_id))
 OR (NEW.duplicate_of IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM comment_items i WHERE i.id=NEW.duplicate_of AND i.pull_request_id=NEW.pull_request_id AND i.tenant_id=NEW.tenant_id))
BEGIN SELECT RAISE(ABORT, 'point source or duplicate does not match pull request'); END;

CREATE TRIGGER comment_items_source_update BEFORE UPDATE OF source_id, quote, duplicate_of, pull_request_id, tenant_id ON comment_items
WHEN (NEW.source_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM comment_sources s WHERE s.id=NEW.source_id AND s.pull_request_id=NEW.pull_request_id AND s.tenant_id=NEW.tenant_id))
 OR (NEW.duplicate_of IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM comment_items i WHERE i.id=NEW.duplicate_of AND i.pull_request_id=NEW.pull_request_id AND i.tenant_id=NEW.tenant_id))
BEGIN SELECT RAISE(ABORT, 'point source or duplicate does not match pull request'); END;
