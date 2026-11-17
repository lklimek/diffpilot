-- smart-review schema (v1, rewritten in place until deployment; docs/contracts/db-schema.md).
-- Every table carries tenant_id except tenants and users (users belong to tenants through memberships).
-- Timestamps are RFC 3339 UTC text. CHECK lists mirror the closed enums of crates/contracts.

CREATE TABLE tenants (
    id         TEXT NOT NULL PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

INSERT INTO tenants (id, name, created_at) VALUES ('default', 'Default', '2026-11-16T00:00:00Z');

-- Sign-in allowlist (ADR-007). github_id is bound at the first successful sign-in; later sign-ins must match.
CREATE TABLE tenant_allowed_logins (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    login     TEXT NOT NULL COLLATE NOCASE,
    github_id INTEGER,
    bound_at  TEXT,
    PRIMARY KEY (tenant_id, login),
    UNIQUE (tenant_id, github_id),
    CHECK ((github_id IS NULL) = (bound_at IS NULL))
) STRICT;

INSERT INTO tenant_allowed_logins (tenant_id, login) VALUES ('default', 'lklimek'), ('default', 'Claudius-Maginificent');

CREATE TABLE users (
    id         TEXT NOT NULL PRIMARY KEY,
    github_id  INTEGER NOT NULL UNIQUE,
    login      TEXT NOT NULL,
    name       TEXT,
    avatar_url TEXT,
    language   TEXT NOT NULL DEFAULT 'en' CHECK (language IN ('en', 'pl')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE memberships (
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role       TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member')),
    created_at TEXT NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
) STRICT;

-- OAuth web flow in progress: state and the encrypted PKCE verifier (short-lived).
CREATE TABLE oauth_logins (
    state            TEXT NOT NULL PRIMARY KEY,
    tenant_id        TEXT NOT NULL REFERENCES tenants (id),
    code_verifier_ct BLOB NOT NULL,
    created_at       TEXT NOT NULL,
    expires_at       TEXT NOT NULL
) STRICT;

-- Cookie sessions; only the hash of the session token is stored.
CREATE TABLE sessions (
    token_hash   BLOB NOT NULL PRIMARY KEY,
    tenant_id    TEXT NOT NULL REFERENCES tenants (id),
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    csrf_token   TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at   TEXT NOT NULL
) STRICT;

CREATE INDEX sessions_by_user ON sessions (user_id);

-- GitHub user tokens, AES-GCM ciphertext; version is the compare-and-swap counter of the serialised refresh.
CREATE TABLE user_tokens (
    user_id            TEXT NOT NULL PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    tenant_id          TEXT NOT NULL REFERENCES tenants (id),
    access_ct          BLOB NOT NULL,
    access_expires_at  TEXT NOT NULL,
    refresh_ct         BLOB NOT NULL,
    refresh_expires_at TEXT NOT NULL,
    version            INTEGER NOT NULL DEFAULT 0,
    updated_at         TEXT NOT NULL
) STRICT;

CREATE TABLE repos (
    id                      INTEGER NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    owner                   TEXT NOT NULL COLLATE NOCASE,
    name                    TEXT NOT NULL COLLATE NOCASE,
    github_id               INTEGER,
    installation_id         INTEGER,
    installation_checked_at TEXT,
    UNIQUE (tenant_id, owner, name)
) STRICT;

CREATE TABLE pull_requests (
    id         INTEGER NOT NULL PRIMARY KEY,
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    repo_id    INTEGER NOT NULL REFERENCES repos (id),
    number     INTEGER NOT NULL CHECK (number > 0),
    title      TEXT NOT NULL,
    author     TEXT NOT NULL,
    html_url   TEXT NOT NULL,
    base_sha   TEXT NOT NULL,
    head_sha   TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    UNIQUE (repo_id, number)
) STRICT;

CREATE TABLE analyses (
    id                  TEXT NOT NULL PRIMARY KEY,
    tenant_id           TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests (id),
    base_sha            TEXT NOT NULL,
    head_sha            TEXT NOT NULL,
    language            TEXT NOT NULL CHECK (language IN ('en', 'pl')),
    state               TEXT NOT NULL CHECK (state IN ('planning', 'running', 'paused', 'failed', 'finished', 'cancelled')),
    flows_total         INTEGER,
    flows_done          INTEGER NOT NULL DEFAULT 0,
    paused_until        TEXT,
    failure_flow_id     TEXT,
    failure_flow_title  TEXT,
    failure_message     TEXT,
    agent_thread_id     TEXT,
    session_state       TEXT NOT NULL DEFAULT 'none' CHECK (session_state IN ('none', 'active', 'drop_requested', 'dropped')),
    skills_commit       TEXT,
    round               INTEGER NOT NULL DEFAULT 0 CHECK (round >= 0), -- incremented by each explicit Retry
    created_by          TEXT NOT NULL REFERENCES users (id),
    created_at          TEXT NOT NULL,
    finished_at         TEXT
) STRICT;

-- One live analysis per pull request head and language; a cancelled one may be analysed again.
CREATE UNIQUE INDEX analyses_one_live_per_head ON analyses (pull_request_id, head_sha, language)
    WHERE state <> 'cancelled';

-- Validated agent output and pre-pass results as JSON documents (key: '' or the flow id).
CREATE TABLE documents (
    analysis_id TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    tenant_id   TEXT NOT NULL REFERENCES tenants (id),
    kind        TEXT NOT NULL CHECK (kind IN ('prepass', 'plan', 'pr_context', 'flow', 'github_snapshot')),
    key         TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL CHECK (json_valid(body)),
    -- An agent output's attempt ran with a GitHub installation token; NULL for documents the server built.
    github_access INTEGER CHECK (github_access IS NULL OR github_access IN (0, 1)),
    created_at  TEXT NOT NULL,
    PRIMARY KEY (analysis_id, kind, key)
) STRICT;

-- LLM credentials the runners hold (never the credential itself); last call outcome feeds Settings.
CREATE TABLE llm_credentials (
    id                TEXT NOT NULL PRIMARY KEY,
    tenant_id         TEXT NOT NULL REFERENCES tenants (id),
    kind              TEXT NOT NULL CHECK (kind IN ('codex_chatgpt')),
    paused_until      TEXT,
    unusable_since    TEXT,
    last_call_at      TEXT,
    last_call_ok      INTEGER CHECK (last_call_ok IN (0, 1)),
    last_call_message TEXT, -- raw provider text: logs and diagnosis only, never shown to users
    last_call_failure TEXT CHECK (last_call_failure IN ('usage_limited', 'provider_failed', 'output_not_json', 'credential_unusable', 'checkout_failed', 'timeout', 'internal')),
    CHECK ((last_call_at IS NULL) = (last_call_ok IS NULL)),
    CHECK (last_call_failure IS NULL OR last_call_ok = 0)
) STRICT;

-- The job queue. spec never holds a secret; the installation token is kept encrypted only to revoke it.
CREATE TABLE jobs (
    id                      TEXT NOT NULL PRIMARY KEY,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    analysis_id             TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    kind                    TEXT NOT NULL CHECK (kind IN ('plan', 'pr_context', 'analyse_flow', 'ask')),
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
                                OR (kind = 'ask' AND stage LIKE 'ask:%')), -- JobStage::key
    generation              INTEGER NOT NULL DEFAULT 0 CHECK (generation BETWEEN 0 AND 2), -- 0 first run, 1..2 repairs
    round                   INTEGER NOT NULL DEFAULT 0, -- the analysis's round at enqueue (Retry starts a new one)
    github_access_granted   INTEGER NOT NULL DEFAULT 0 CHECK (github_access_granted IN (0, 1)), -- last claim had a token
    flow_id                 TEXT,
    question_id             TEXT,
    created_at              TEXT NOT NULL,
    claimed_at              TEXT,
    finished_at             TEXT
) STRICT;

-- One running job per agent session (analysis) and per LLM credential (ADR-003, ADR-004).
CREATE UNIQUE INDEX jobs_one_running_per_analysis ON jobs (analysis_id) WHERE state IN ('claimed', 'running');
CREATE UNIQUE INDEX jobs_one_running_per_credential ON jobs (credential_id) WHERE state IN ('claimed', 'running');
CREATE INDEX jobs_claim_order ON jobs (state, priority, created_at);
-- Idempotent submit: one live or succeeded job per pipeline stage, generation (repair run) and round (Retry); a
-- failed or cancelled one may be submitted again.
CREATE UNIQUE INDEX jobs_one_live_per_stage ON jobs (analysis_id, stage, generation, round)
    WHERE state NOT IN ('failed', 'cancelled');
CREATE INDEX jobs_by_analysis ON jobs (analysis_id, state);

-- Encrypted installation tokens per job attempt, kept until their revocation is confirmed (ADR-005).
CREATE TABLE installation_tokens (
    job_id     TEXT NOT NULL REFERENCES jobs (id),
    tenant_id  TEXT NOT NULL REFERENCES tenants (id),
    attempt    INTEGER NOT NULL,
    ciphertext BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY (job_id, attempt)
) STRICT;

-- Bounded event log (the last 500 events per job are kept).
CREATE TABLE job_events (
    job_id    TEXT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    attempt   INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    at        TEXT NOT NULL,
    body      TEXT NOT NULL CHECK (json_valid(body)),
    PRIMARY KEY (job_id, attempt, seq)
) STRICT;

CREATE TABLE review_states (
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    analysis_id  TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    tenant_id    TEXT NOT NULL REFERENCES tenants (id),
    ticked_steps TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(ticked_steps)),
    last_viewed  TEXT CHECK (last_viewed IS NULL OR json_valid(last_viewed)),
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (user_id, analysis_id)
) STRICT;

CREATE TABLE questions (
    id          TEXT NOT NULL PRIMARY KEY,
    tenant_id   TEXT NOT NULL REFERENCES tenants (id),
    analysis_id TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    step_id     TEXT NOT NULL,
    user_id     TEXT NOT NULL REFERENCES users (id),
    text        TEXT NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('pending', 'answering', 'answered', 'failed')),
    answer      TEXT CHECK (answer IS NULL OR json_valid(answer)),
    failure     TEXT,
    job_id      TEXT,
    created_at  TEXT NOT NULL,
    answered_at TEXT
) STRICT;

CREATE INDEX questions_by_analysis ON questions (analysis_id, created_at);

-- One entry per user, target and value (several values may coexist on a target); posting the value again
-- replaces its note and increments revision. A removed entry stays as a tombstone (removed_at) until MemCan has
-- deleted its memory. memcan_state tracks the MemCan write of the current revision; an ack for an older revision
-- leaves the row pending. attempts / next_attempt_at schedule retries per entry; unsettled_until is the time
-- until which a MemCan write of unknown outcome (no answer) may still land, so a confirmation sent before it is
-- re-asserted after it.
CREATE TABLE feedback (
    id           TEXT NOT NULL PRIMARY KEY,
    tenant_id    TEXT NOT NULL REFERENCES tenants (id),
    analysis_id  TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    user_id      TEXT NOT NULL REFERENCES users (id),
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('step', 'annotation', 'line')),
    target_key   TEXT NOT NULL,
    value        TEXT NOT NULL CHECK (value IN ('correct', 'wrong', 'irrelevant', 'overrated', 'underrated', 'missed_by_llm')),
    note         TEXT,
    memcan_state TEXT NOT NULL DEFAULT 'pending' CHECK (memcan_state IN ('pending', 'written', 'failed')),
    revision     INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    removed_at   TEXT,
    attempts     INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TEXT,
    unsettled_until TEXT,
    UNIQUE (user_id, analysis_id, target_kind, target_key, value)
) STRICT;

CREATE INDEX feedback_memcan_queue ON feedback (memcan_state, next_attempt_at) WHERE memcan_state != 'written';

-- Idempotency of pending-review comment creation (ADR-006).
CREATE TABLE comment_requests (
    user_id         TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    analysis_id     TEXT NOT NULL REFERENCES analyses (id) ON DELETE CASCADE,
    request_hash    BLOB NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('in_progress', 'done', 'failed')),
    response        TEXT CHECK (response IS NULL OR json_valid(response)),
    created_at      TEXT NOT NULL,
    PRIMARY KEY (user_id, idempotency_key)
) STRICT;
