-- Owner-approved v1.3 cut-over (2026-11-23): clean runtime/analysis data in place.
-- Identity, stored GitHub auth and configuration stay untouched; see docs/contracts/containers.md.
-- SQLx runs the entire cleanup/rebuild atomically with foreign keys enabled. Codex must be relinked.
DROP TABLE installation_tokens;
DROP TABLE job_events;
DROP TABLE jobs;
DROP TABLE llm_credentials;
DROP TABLE comment_assessments;
DROP TABLE working_sessions;

-- Retained watches must rediscover the cleaned review loop and notify for its first round.
UPDATE pr_watches SET probe = NULL, last_probe_at = NULL,
    last_auto_pass_at = NULL, last_auto_pass_questions = NULL,
    pass_due_at = NULL, notified_for = NULL,
    state = CASE WHEN state IN ('job_running', 'waiting_for_bot_round', 'limit_reached')
        THEN 'active' ELSE state END;

-- Children before parents; keep PR/repository identity so watches retain their original keys.
DELETE FROM notification_deliveries;
DELETE FROM notification_sends;
DELETE FROM user_events;
DELETE FROM comment_effect_reports;
DELETE FROM comment_effects;
DELETE FROM comment_carry_events;
DELETE FROM comment_decision_undos;
DELETE FROM comment_item_skips;
DELETE FROM comment_decisions;
DELETE FROM comment_passes;
DELETE FROM questions;
DELETE FROM comment_items;
DELETE FROM comment_prs;
DELETE FROM documents;
DELETE FROM review_states;
DELETE FROM feedback;
DELETE FROM comment_requests;
DELETE FROM flow_work;
DELETE FROM analyses;
DELETE FROM dashboard_judgements;
DELETE FROM dashboard_refreshes;

-- Only non-secret ChatGPT-plan status is stored; rotation state stays gateway-only.
CREATE TABLE llm_credentials (
    user_id           TEXT NOT NULL PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    tenant_id         TEXT NOT NULL REFERENCES tenants (id),
    mode              TEXT NOT NULL DEFAULT 'chatgpt_plan' CHECK (mode = 'chatgpt_plan'),
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
    pass_id TEXT,
    method TEXT CHECK (method IN ('claudius_host','prompt_v4')),
    payer_id TEXT REFERENCES users(id),
    head TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    body TEXT NOT NULL CHECK (json_valid(body)),
    prompt_version INTEGER NOT NULL,
    assessed_at TEXT NOT NULL,
    CHECK ((pass_id IS NULL AND method IS NULL AND payer_id IS NULL) OR
           (pass_id IS NOT NULL AND method IS NOT NULL AND payer_id IS NOT NULL)),
    FOREIGN KEY (pass_id,method,tenant_id,payer_id)
        REFERENCES assessment_passes(id,primary_method,tenant_id,payer_id),
    FOREIGN KEY (job_id,pass_id,method,tenant_id,payer_id)
        REFERENCES assessment_method_runs(job_id,pass_id,method,tenant_id,payer_id),
    CHECK (json_extract(body, '$.validity') IN ('valid', 'partly_valid', 'invalid', 'already_fixed')),
    CHECK (json_type(body, '$.pre_existing') IS NULL OR json_type(body, '$.pre_existing') IN ('null', 'true', 'false')),
    CHECK (json_type(body, '$.code') IS NULL OR json_type(body, '$.code') IN ('null', 'object')),
    CHECK (json_extract(body, '$.fix_size') IS NULL OR json_extract(body, '$.fix_size') IN ('XS', 'S', 'M')),
    CHECK (json_type(body, '$.evidence_text') IS NULL OR json_type(body, '$.evidence_text') IN ('null', 'text')),
    CHECK (json_type(body, '$.recorded_decision') IS NULL OR json_type(body, '$.recorded_decision') IN ('null', 'object'))
) STRICT;
CREATE INDEX comment_assessments_by_item ON comment_assessments (item_id, id);

-- Distinguish private Ask recovery from legacy assessment-question continuations.
ALTER TABLE questions ADD COLUMN private_ask INTEGER NOT NULL DEFAULT 0
    CHECK (private_ask IN (0,1) AND (private_ask=0 OR
        (analysis_id IS NULL AND kind='comment_item' AND pull_request_id IS NOT NULL)));
CREATE UNIQUE INDEX questions_private_owner ON questions(id,pull_request_id,user_id,tenant_id);

CREATE TABLE jobs (
    id                      TEXT NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    analysis_id             TEXT REFERENCES analyses (id) ON DELETE CASCADE,
    refresh_id              TEXT REFERENCES dashboard_refreshes (id) ON DELETE CASCADE,
    pass_id                 TEXT REFERENCES comment_passes (id) ON DELETE CASCADE,
    private_question_id     TEXT,
    assessment_pass_id      TEXT,
    assessment_method       TEXT CHECK (assessment_method IN ('claudius_host','prompt_v4')),
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
    UNIQUE (id,assessment_pass_id,assessment_method,tenant_id,payer_id),
    FOREIGN KEY (assessment_pass_id,assessment_method,tenant_id,payer_id)
        REFERENCES assessment_method_runs(pass_id,method,tenant_id,payer_id),
    FOREIGN KEY (private_question_id,pull_request_id,payer_id,tenant_id)
        REFERENCES questions(id,pull_request_id,user_id,tenant_id) ON DELETE CASCADE,
    CHECK (private_question_id IS NULL OR
        (kind='ask' AND question_id IS private_question_id AND stage='ask:' || private_question_id
         AND json_extract(spec,'$.target.type') IS 'pull_request_ask'
         AND json_extract(spec,'$.target.pull_request_id') IS pull_request_id
         AND json_extract(spec,'$.payer') IS payer_id AND json_type(spec,'$.assessment') IS NULL)),
    CHECK ((json_extract(spec,'$.target.type') IS 'pull_request_ask') = (private_question_id IS NOT NULL)),
    CHECK ((assessment_pass_id IS NULL) = (assessment_method IS NULL)),
    CHECK ((analysis_id IS NOT NULL) + (refresh_id IS NOT NULL) + (pass_id IS NOT NULL) + (assessment_pass_id IS NOT NULL) + (private_question_id IS NOT NULL) = 1),
    CHECK ((kind = 'judge_work') = (refresh_id IS NOT NULL)),
    CHECK ((kind = 'verify_comments') = (pass_id IS NOT NULL OR assessment_pass_id IS NOT NULL)),
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
CREATE UNIQUE INDEX jobs_one_live_per_private_question ON jobs(private_question_id,generation)
    WHERE private_question_id IS NOT NULL AND state NOT IN ('failed','cancelled');
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

CREATE UNIQUE INDEX pull_requests_ownership ON pull_requests(id,tenant_id);
CREATE UNIQUE INDEX comment_items_ownership ON comment_items(id,tenant_id,pull_request_id);
ALTER TABLE comment_decisions ADD COLUMN source_version TEXT;
CREATE UNIQUE INDEX comment_decisions_ownership ON comment_decisions(id,item_id,tenant_id,user_id);

CREATE TABLE assessment_config (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    primary_method TEXT NOT NULL DEFAULT 'claudius_host' CHECK (primary_method IN ('claudius_host','prompt_v4')),
    ab_prs TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(ab_prs) AND json_type(ab_prs) = 'array'),
    ab_until TEXT CHECK (ab_until IS NULL OR (julianday(ab_until) IS NOT NULL AND substr(ab_until,-1) = 'Z'))
) STRICT;
CREATE TRIGGER assessment_config_prs_insert BEFORE INSERT ON assessment_config
WHEN EXISTS (SELECT 1 FROM json_each(NEW.ab_prs) WHERE type <> 'text')
BEGIN SELECT RAISE(ABORT,'A/B PR selections must be strings'); END;
CREATE TRIGGER assessment_config_prs_update BEFORE UPDATE OF ab_prs ON assessment_config
WHEN EXISTS (SELECT 1 FROM json_each(NEW.ab_prs) WHERE type <> 'text')
BEGIN SELECT RAISE(ABORT,'A/B PR selections must be strings'); END;

CREATE TABLE assessment_daily_usage (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day_utc TEXT NOT NULL CHECK (length(day_utc)=10 AND date(day_utc,'+0 days') IS day_utc),
    passes INTEGER NOT NULL DEFAULT 0 CHECK (passes BETWEEN 0 AND 80),
    PRIMARY KEY (user_id,day_utc)
) STRICT;

CREATE TABLE assessment_passes (
    id TEXT PRIMARY KEY NOT NULL,
    tenant_id TEXT NOT NULL,
    payer_id TEXT NOT NULL,
    pull_request_id INTEGER NOT NULL,
    request_key TEXT NOT NULL CHECK (length(request_key)>0),
    head TEXT NOT NULL,
    base TEXT NOT NULL,
    input_hash TEXT NOT NULL CHECK (length(input_hash)=64 AND input_hash NOT GLOB '*[^0-9a-f]*'),
    scope_hash TEXT NOT NULL CHECK (length(scope_hash)=64 AND scope_hash NOT GLOB '*[^0-9a-f]*'),
    snapshot_ref TEXT NOT NULL CHECK (length(snapshot_ref)>0),
    primary_method TEXT NOT NULL CHECK (primary_method IN ('claudius_host','prompt_v4')),
    paired INTEGER NOT NULL CHECK (paired IN (0,1)),
    config TEXT NOT NULL CHECK (json_valid(config) AND json_type(config)='object'),
    input TEXT NOT NULL CHECK (json_valid(input) AND json_type(input)='object'),
    day_utc TEXT NOT NULL,
    admitted_at TEXT NOT NULL CHECK (julianday(admitted_at) IS NOT NULL AND substr(admitted_at,1,10)=day_utc AND substr(admitted_at,-1)='Z'),
    UNIQUE (payer_id,request_key),
    UNIQUE (id,tenant_id,payer_id),
    UNIQUE (id,tenant_id,payer_id,pull_request_id),
    UNIQUE (id,primary_method,tenant_id,payer_id),
    FOREIGN KEY (tenant_id,payer_id) REFERENCES memberships(tenant_id,user_id),
    FOREIGN KEY (pull_request_id,tenant_id) REFERENCES pull_requests(id,tenant_id),
    FOREIGN KEY (payer_id,day_utc) REFERENCES assessment_daily_usage(user_id,day_utc),
    CHECK (json_extract(config,'$.primary_method') IS primary_method),
    CHECK (json_extract(input,'$.head') IS head AND json_extract(input,'$.base') IS base),
    CHECK (json_extract(input,'$.input_hash') IS input_hash AND json_extract(input,'$.scope_hash') IS scope_hash),
    CHECK (json_extract(input,'$.snapshot_ref') IS snapshot_ref AND json_extract(input,'$.pull_request_id') IS pull_request_id)
) STRICT;
CREATE TRIGGER assessment_passes_immutable BEFORE UPDATE ON assessment_passes
BEGIN SELECT RAISE(ABORT,'assessment input is frozen'); END;

CREATE TABLE assessment_method_runs (
    pass_id TEXT NOT NULL,
    method TEXT NOT NULL CHECK (method IN ('claudius_host','prompt_v4')),
    tenant_id TEXT NOT NULL,
    payer_id TEXT NOT NULL,
    job_id TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    reported_head TEXT,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','running','succeeded','failed','cancelled')),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt>=0),
    accepted_attempt INTEGER CHECK (accepted_attempt=attempt AND accepted_attempt>0),
    input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens>=0),
    cached_input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cached_input_tokens BETWEEN 0 AND input_tokens),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens>=0),
    requests INTEGER DEFAULT 0 CHECK (requests>=0),
    duration_ms INTEGER NOT NULL DEFAULT 0 CHECK (duration_ms>=0),
    failure TEXT,
    PRIMARY KEY (pass_id,method),
    UNIQUE (pass_id,method,tenant_id,payer_id),
    UNIQUE (job_id,pass_id,method,tenant_id,payer_id),
    FOREIGN KEY (pass_id,tenant_id,payer_id) REFERENCES assessment_passes(id,tenant_id,payer_id),
    FOREIGN KEY (job_id,pass_id,method,tenant_id,payer_id)
        REFERENCES jobs(id,assessment_pass_id,assessment_method,tenant_id,payer_id) DEFERRABLE INITIALLY DEFERRED,
    CHECK ((status='succeeded')=(accepted_attempt IS NOT NULL))
) STRICT;
CREATE TRIGGER assessment_method_selection BEFORE INSERT ON assessment_method_runs
WHEN NOT EXISTS (SELECT 1 FROM assessment_passes p WHERE p.id=NEW.pass_id AND (p.paired=1 OR p.primary_method=NEW.method))
BEGIN SELECT RAISE(ABORT,'method is not selected for this pass'); END;
CREATE TRIGGER assessment_method_identity BEFORE UPDATE OF pass_id,method,tenant_id,payer_id,job_id,version ON assessment_method_runs
BEGIN SELECT RAISE(ABORT,'assessment method identity is frozen'); END;

-- Deduplicate metering by the existing job/attempt fence, including failed retries.
CREATE TABLE assessment_attempt_usage (
    pass_id TEXT NOT NULL,
    method TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    payer_id TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK (attempt>0),
    input_tokens INTEGER NOT NULL CHECK (input_tokens>=0),
    cached_input_tokens INTEGER NOT NULL CHECK (cached_input_tokens BETWEEN 0 AND input_tokens),
    output_tokens INTEGER NOT NULL CHECK (output_tokens>=0),
    requests INTEGER CHECK (requests>=0),
    duration_ms INTEGER NOT NULL CHECK (duration_ms>=0),
    known INTEGER NOT NULL DEFAULT 1 CHECK (known IN (0,1)),
    PRIMARY KEY (pass_id,method,attempt),
    FOREIGN KEY (pass_id,method,tenant_id,payer_id) REFERENCES assessment_method_runs(pass_id,method,tenant_id,payer_id)
) STRICT;

CREATE TABLE assessment_comparison_items (
    pass_id TEXT NOT NULL,
    method TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    payer_id TEXT NOT NULL,
    pull_request_id INTEGER NOT NULL,
    comparison_key TEXT NOT NULL CHECK (length(comparison_key)>0),
    local_id TEXT NOT NULL,
    method_version TEXT NOT NULL,
    source TEXT NOT NULL CHECK (json_valid(source) AND json_type(source)='object'),
    primary_item_id TEXT,
    primary_item_hash TEXT CHECK (primary_item_hash IS NULL OR (length(primary_item_hash)=64 AND primary_item_hash NOT GLOB '*[^0-9a-f]*')),
    status TEXT NOT NULL CHECK (status IN ('matched','missing_row','duplicate','unmatched','method_failed','head_mismatch','context')),
    duplicate_of TEXT CHECK (duplicate_of<>comparison_key),
    verdict TEXT CHECK (verdict IN ('valid','invalid','already_fixed','out_of_scope')),
    native_verdict TEXT,
    recorded_decision TEXT CHECK (recorded_decision IS NULL OR (json_valid(recorded_decision) AND json_type(recorded_decision)='object')),
    proposal TEXT CHECK (proposal IS NULL OR (json_valid(proposal) AND json_type(proposal)='object')),
    CHECK ((primary_item_id IS NULL)=(primary_item_hash IS NULL)),
    PRIMARY KEY (pass_id,method,comparison_key),
    UNIQUE (pass_id,method,comparison_key,tenant_id,payer_id,primary_item_id),
    FOREIGN KEY (pass_id,method,tenant_id,payer_id) REFERENCES assessment_method_runs(pass_id,method,tenant_id,payer_id),
    FOREIGN KEY (pass_id,tenant_id,payer_id,pull_request_id) REFERENCES assessment_passes(id,tenant_id,payer_id,pull_request_id),
    FOREIGN KEY (primary_item_id,tenant_id,pull_request_id) REFERENCES comment_items(id,tenant_id,pull_request_id),
    FOREIGN KEY (pass_id,method,duplicate_of) REFERENCES assessment_comparison_items(pass_id,method,comparison_key),
    CHECK ((status='duplicate')=(duplicate_of IS NOT NULL)),
    CHECK (status NOT IN ('duplicate','missing_row','method_failed') OR verdict IS NULL)
) STRICT;
CREATE TRIGGER assessment_duplicate_direct BEFORE INSERT ON assessment_comparison_items
WHEN NEW.duplicate_of IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM assessment_comparison_items c WHERE c.pass_id=NEW.pass_id AND c.method=NEW.method
    AND c.comparison_key=NEW.duplicate_of AND c.duplicate_of IS NULL)
BEGIN SELECT RAISE(ABORT,'duplicate must reference a direct canonical row'); END;
CREATE TRIGGER assessment_comparison_immutable BEFORE UPDATE ON assessment_comparison_items
BEGIN SELECT RAISE(ABORT,'accepted comparison rows are immutable'); END;

CREATE TABLE assessment_owner_decisions (
    pass_id TEXT NOT NULL,
    method TEXT NOT NULL,
    comparison_key TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    payer_id TEXT NOT NULL,
    primary_item_id TEXT NOT NULL,
    decision_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision>0),
    owner_id TEXT NOT NULL,
    decided_at TEXT NOT NULL,
    kind TEXT NOT NULL,
    target TEXT CHECK (target IS NULL OR json_valid(target)),
    PRIMARY KEY (pass_id,comparison_key,revision),
    UNIQUE (pass_id,comparison_key,decision_id),
    FOREIGN KEY (pass_id,method,tenant_id,payer_id) REFERENCES assessment_passes(id,primary_method,tenant_id,payer_id),
    FOREIGN KEY (pass_id,method,comparison_key,tenant_id,payer_id,primary_item_id)
        REFERENCES assessment_comparison_items(pass_id,method,comparison_key,tenant_id,payer_id,primary_item_id),
    FOREIGN KEY (decision_id,primary_item_id,tenant_id,owner_id) REFERENCES comment_decisions(id,item_id,tenant_id,user_id)
) STRICT;
CREATE TRIGGER assessment_owner_truth BEFORE INSERT ON assessment_owner_decisions
WHEN NOT EXISTS (
    SELECT 1 FROM comment_decisions d JOIN assessment_passes p ON p.id=NEW.pass_id
    JOIN assessment_comparison_items c ON c.pass_id=p.id AND c.method=p.primary_method AND c.comparison_key=NEW.comparison_key
    WHERE d.id=NEW.decision_id AND d.head=p.head AND d.kind=NEW.kind AND d.target IS NEW.target
    AND d.decided_at=NEW.decided_at AND d.body_hash=c.primary_item_hash
    AND (json_extract(c.source,'$.kind')='inline' OR d.source_version=json_extract(c.source,'$.version')))
BEGIN SELECT RAISE(ABORT,'only actual owner choices at this head and source version may be attached'); END;
CREATE TRIGGER assessment_owner_immutable BEFORE UPDATE ON assessment_owner_decisions
BEGIN SELECT RAISE(ABORT,'owner decision history is append only'); END;
CREATE TRIGGER assessment_owner_no_delete BEFORE DELETE ON assessment_owner_decisions
BEGIN SELECT RAISE(ABORT,'owner decision history is append only'); END;

CREATE TRIGGER assessment_primary_item BEFORE INSERT ON comment_assessments
WHEN NEW.pass_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM assessment_passes p JOIN comment_items i ON i.pull_request_id=p.pull_request_id AND i.tenant_id=p.tenant_id
    WHERE p.id=NEW.pass_id AND i.id=NEW.item_id AND p.head=NEW.head
    AND EXISTS(SELECT 1 FROM assessment_method_runs r JOIN jobs j ON j.id=r.job_id
        WHERE r.pass_id=p.id AND r.method=p.primary_method AND r.job_id=NEW.job_id
        AND r.status='succeeded' AND r.accepted_attempt=j.attempt))
BEGIN SELECT RAISE(ABORT,'primary assessment must match frozen head and PR'); END;
CREATE TRIGGER assessment_primary_item_update BEFORE UPDATE ON comment_assessments
WHEN NEW.pass_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM assessment_passes p JOIN comment_items i ON i.pull_request_id=p.pull_request_id AND i.tenant_id=p.tenant_id
    WHERE p.id=NEW.pass_id AND i.id=NEW.item_id AND p.head=NEW.head
    AND EXISTS(SELECT 1 FROM assessment_method_runs r JOIN jobs j ON j.id=r.job_id
        WHERE r.pass_id=p.id AND r.method=p.primary_method AND r.job_id=NEW.job_id
        AND r.status='succeeded' AND r.accepted_attempt=j.attempt))
BEGIN SELECT RAISE(ABORT,'primary assessment must match frozen head and PR'); END;
CREATE UNIQUE INDEX jobs_one_assessment_method ON jobs(assessment_pass_id,assessment_method) WHERE assessment_pass_id IS NOT NULL;
CREATE TRIGGER assessment_job_target BEFORE INSERT ON jobs
WHEN NEW.assessment_pass_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM assessment_passes p WHERE p.id=NEW.assessment_pass_id AND p.pull_request_id=NEW.pull_request_id
    AND json_extract(NEW.spec,'$.assessment.pass_id') IS p.id
    AND json_extract(NEW.spec,'$.assessment.method') IS NEW.assessment_method
    AND json_extract(NEW.spec,'$.payer') IS p.payer_id
    AND json_extract(NEW.spec,'$.target.pass_id') IS p.id
    AND json_extract(NEW.spec,'$.target.repo.head_sha') IS p.head
    AND json_extract(NEW.spec,'$.target.repo.base_sha') IS p.base)
BEGIN SELECT RAISE(ABORT,'assessment job must use frozen input'); END;
CREATE TRIGGER assessment_job_identity BEFORE UPDATE OF assessment_pass_id,assessment_method,payer_id,pull_request_id,tenant_id ON jobs
WHEN OLD.assessment_pass_id IS NOT NULL OR NEW.assessment_pass_id IS NOT NULL
BEGIN SELECT RAISE(ABORT,'assessment job identity is frozen'); END;
