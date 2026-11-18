-- GitHub work dashboard (docs/v1.1-dashboard-plan.md §D.1 T-1..T-4). Additive: existing rows are kept.
-- JSON columns hold the serde form of the crates/contracts types named in the comments.

-- Per-user dashboard settings (DashboardSettings); an absent row means the defaults of plan §F.6.
CREATE TABLE dashboard_settings (
    user_id                TEXT NOT NULL PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    tenant_id              TEXT NOT NULL REFERENCES tenants (id),
    linked_logins          TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(linked_logins) AND json_type(linked_logins) = 'array'),
    enumerate_owners       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(enumerate_owners) AND json_type(enumerate_owners) = 'array'),
    -- RepositoryRule[]
    repositories           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(repositories) AND json_type(repositories) = 'array'),
    lookback_days          INTEGER NOT NULL DEFAULT 7 CHECK (lookback_days > 0),
    ready_label            TEXT NOT NULL DEFAULT 'ready-for-human',
    skip_authors           TEXT NOT NULL DEFAULT '["dependabot","dependabot[bot]"]' CHECK (json_valid(skip_authors) AND json_type(skip_authors) = 'array'),
    include_todos          INTEGER NOT NULL DEFAULT 1 CHECK (include_todos IN (0, 1)),
    memcan_extra_projects  TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(memcan_extra_projects) AND json_type(memcan_extra_projects) = 'array'),
    schedule_enabled       INTEGER NOT NULL DEFAULT 1 CHECK (schedule_enabled IN (0, 1)),
    schedule_time          TEXT NOT NULL DEFAULT '06:00',
    -- IANA name; NULL until the browser sends one (the schedule does not run without it).
    time_zone              TEXT,
    -- Local date (YYYY-MM-DD) of the last scheduled run, its ScheduledOutcome and plain reason.
    last_scheduled_on      TEXT,
    last_scheduled_outcome TEXT CHECK (last_scheduled_outcome IN ('started', 'skipped', 'needs_sign_in', 'failed')),
    last_scheduled_reason  TEXT,
    updated_at             TEXT NOT NULL,
    CHECK ((last_scheduled_on IS NULL) = (last_scheduled_outcome IS NULL))
) STRICT;

-- One scan and its ranking per row; the snapshot is one validated JSON document (NULL until scanned).
CREATE TABLE dashboard_refreshes (
    id               TEXT NOT NULL PRIMARY KEY,
    tenant_id        TEXT NOT NULL REFERENCES tenants (id),
    user_id          TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    trigger          TEXT NOT NULL CHECK (trigger IN ('manual', 'scheduled')),
    state            TEXT NOT NULL CHECK (state IN ('scanning', 'ranking', 'finished', 'failed', 'cancelled')),
    -- {steps: ScanStep[], percent}
    progress         TEXT NOT NULL DEFAULT '{"steps":[],"percent":0}' CHECK (json_valid(progress)),
    -- string[], at most 200 lines
    log              TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(log) AND json_type(log) = 'array'),
    failure_code     TEXT CHECK (failure_code IN ('github_rate_limited', 'github_unavailable', 'sign_in_required', 'timed_out', 'interrupted', 'internal')),
    failure_message  TEXT,
    retry_at         TEXT,
    ranking_state    TEXT CHECK (ranking_state IN ('waiting_for_analysis', 'running', 'paused', 'failed', 'interrupted', 'finished')),
    judged           INTEGER NOT NULL DEFAULT 0 CHECK (judged >= 0),
    reused           INTEGER NOT NULL DEFAULT 0 CHECK (reused >= 0),
    left_provisional INTEGER NOT NULL DEFAULT 0 CHECK (left_provisional >= 0),
    snapshot         TEXT CHECK (snapshot IS NULL OR json_valid(snapshot)),
    started_at       TEXT NOT NULL,
    scanned_at       TEXT,
    finished_at      TEXT,
    CHECK ((failure_code IS NULL) = (failure_message IS NULL)),
    CHECK ((state = 'failed') = (failure_code IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX dashboard_refreshes_one_live_per_user ON dashboard_refreshes (user_id)
    WHERE state IN ('scanning', 'ranking');
CREATE INDEX dashboard_refreshes_by_user ON dashboard_refreshes (user_id, started_at);

-- Stored LLM judgement per (user, item): the inheritance source and the reuse rule of plan §B.4.
CREATE TABLE dashboard_judgements (
    tenant_id         TEXT NOT NULL REFERENCES tenants (id),
    user_id           TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_key          TEXT NOT NULL,
    -- SHA-256 hex of JUDGE_INPUT_VERSION and the canonical judged inputs
    input_fingerprint TEXT NOT NULL,
    input_version     INTEGER NOT NULL,
    prompt_version    INTEGER NOT NULL,
    body              TEXT NOT NULL CHECK (json_valid(body)),
    refresh_id        TEXT REFERENCES dashboard_refreshes (id) ON DELETE SET NULL,
    -- no foreign key: job rows follow their own retention
    job_id            TEXT,
    model             TEXT NOT NULL,
    judged_at         TEXT NOT NULL,
    last_seen_at      TEXT NOT NULL,
    PRIMARY KEY (user_id, item_key)
) STRICT;

CREATE INDEX dashboard_judgements_by_judged_at ON dashboard_judgements (user_id, judged_at);
CREATE INDEX dashboard_judgements_by_last_seen ON dashboard_judgements (last_seen_at);

-- Installation-wide reasoning effort of the dashboard ranking (OD13), next to the model choice.
ALTER TABLE llm_selection ADD COLUMN judge_effort TEXT NOT NULL DEFAULT 'low'
    CHECK (judge_effort IN ('low', 'medium', 'high'));
