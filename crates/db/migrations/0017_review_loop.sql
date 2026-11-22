-- Review loop (ADR-023, docs/review-workflow-plan.md §4.3). Additive: new tables only. PR data is tenant-shared like
-- analyses (keyed by `pull_requests`); decisions are append-only; channel and push-target secrets are AES-GCM
-- ciphertext written by the server (`server::secrets`), never plain text. CHECK lists mirror the contract enums
-- (`contracts::api::review_loop`).

-- T-L1: one row per pull request processed in Comments: the last GitHub read and the item numbering.
CREATE TABLE comment_prs (
    pull_request_id INTEGER NOT NULL PRIMARY KEY REFERENCES pull_requests (id) ON DELETE CASCADE,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    read_at         TEXT, -- last successful read
    read_failure    TEXT CHECK (read_failure IN ('rate_limited', 'unavailable', 'sign_in_required')),
    read_failed_at  TEXT,
    github_snapshot TEXT CHECK (github_snapshot IS NULL OR json_valid(github_snapshot)),
    round_count     INTEGER NOT NULL DEFAULT 0 CHECK (round_count >= 0),
    next_item_no    INTEGER NOT NULL DEFAULT 1 CHECK (next_item_no >= 1), -- `C<no>` numbers are never reused
    purpose         TEXT, -- LLM: the PR's purpose of the latest pass
    created_at      TEXT NOT NULL,
    CHECK ((read_failure IS NULL) = (read_failed_at IS NULL))
) STRICT;

-- T-L2: items (threads, review-body items, PR-comment items, PR-level questions).
CREATE TABLE comment_items (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES comment_prs (pull_request_id) ON DELETE CASCADE,
    item_no         INTEGER NOT NULL CHECK (item_no >= 1),
    kind            TEXT NOT NULL CHECK (kind IN ('thread', 'review_body', 'pr_comment', 'pr_question')),
    stable_key      TEXT NOT NULL CHECK (length(stable_key) > 0),
    author          TEXT NOT NULL,
    author_kind     TEXT NOT NULL CHECK (author_kind IN ('human', 'bot', 'diffpilot')),
    url             TEXT,
    path            TEXT,
    start_line      INTEGER,
    end_line        INTEGER,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    body_hash       TEXT NOT NULL, -- SHA-256 hex of the normalised finding text (spike S13-1 K1)
    first_seen_head TEXT NOT NULL,
    first_round     INTEGER,
    bot_severity    TEXT,
    gate_marks      TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(gate_marks) AND json_type(gate_marks) = 'array'),
    github          TEXT NOT NULL CHECK (json_valid(github)), -- ItemGithubFacts
    state           TEXT NOT NULL CHECK (state IN ('assessing', 'undecided', 'asked_back', 'decided', 'executing',
                                                   'done', 'repeat', 'reopened', 'outside')),
    revision        INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    outside         TEXT CHECK (outside IS NULL OR json_valid(outside)), -- OutsideResolution
    repeat_of       TEXT REFERENCES comment_items (id),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE (pull_request_id, item_no),
    UNIQUE (pull_request_id, stable_key),
    CHECK ((start_line IS NULL) = (end_line IS NULL)),
    CHECK (start_line IS NULL OR (path IS NOT NULL AND 1 <= start_line AND start_line <= end_line)),
    CHECK ((state = 'outside') = (outside IS NOT NULL))
) STRICT;

-- T-L3: LLM assessments, one per pass that assessed the item; the newest is current. `fingerprint` (comment text,
-- code at its lines, PR purpose) lets an unchanged item reuse its assessment.
CREATE TABLE comment_assessments (
    id             INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    tenant_id      TEXT NOT NULL REFERENCES tenants (id),
    item_id        TEXT NOT NULL REFERENCES comment_items (id) ON DELETE CASCADE,
    job_id         TEXT NOT NULL, -- no foreign key: jobs are pruned independently
    head           TEXT NOT NULL,
    fingerprint    TEXT NOT NULL,
    body           TEXT NOT NULL CHECK (json_valid(body)), -- CommentAssessment
    prompt_version INTEGER NOT NULL,
    assessed_at    TEXT NOT NULL
) STRICT;
CREATE INDEX comment_assessments_by_item ON comment_assessments (item_id, id);

-- Assessment passes (`verify_comments`): one queued or running per pull request; charged to a user's daily cap.
CREATE TABLE comment_passes (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES comment_prs (pull_request_id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    cause           TEXT NOT NULL CHECK (cause IN ('opened', 'read', 'assess_again', 'question', 'watch')),
    head            TEXT NOT NULL,
    items           INTEGER NOT NULL CHECK (items >= 0),
    state           TEXT NOT NULL CHECK (state IN ('queued', 'running', 'finished', 'failed')),
    job_id          TEXT,
    created_at      TEXT NOT NULL,
    finished_at     TEXT,
    CHECK ((state IN ('finished', 'failed')) = (finished_at IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX comment_passes_one_live_per_pr ON comment_passes (pull_request_id)
    WHERE state IN ('queued', 'running');
CREATE INDEX comment_passes_by_user ON comment_passes (user_id, created_at);

-- T-L5: decisions, append-only (no UPDATE); the newest per item is current, `supersedes` names the one before.
CREATE TABLE comment_decisions (
    id             TEXT NOT NULL PRIMARY KEY,
    tenant_id      TEXT NOT NULL REFERENCES tenants (id),
    item_id        TEXT NOT NULL REFERENCES comment_items (id) ON DELETE CASCADE,
    user_id        TEXT NOT NULL REFERENCES users (id),
    kind           TEXT NOT NULL CHECK (kind IN ('resolve', 'fix_here', 'reject', 'handled_in_pr', 'follow_up_pr',
                                                 'upstream_issue', 'todo', 'merge_with_follow_ups', 'another_round',
                                                 'dismiss_stale_reviews', 'keep_reviews')),
    reason         TEXT,
    target         TEXT CHECK (target IS NULL OR json_valid(target)),     -- DecisionTarget
    override       TEXT CHECK (override IS NULL OR json_valid(override)), -- Override
    resolve_ticked INTEGER NOT NULL CHECK (resolve_ticked IN (0, 1)),
    reply_text     TEXT,
    comment        TEXT, -- the owner's comment: sent to the executor, never posted
    recommendation TEXT CHECK (recommendation IS NULL OR json_valid(recommendation)), -- shown at decision time
    via            TEXT NOT NULL CHECK (via IN ('question', 'group', 'bulk', 'table', 'carried')),
    carried_from   TEXT REFERENCES comment_decisions (id),
    supersedes     TEXT REFERENCES comment_decisions (id),
    head           TEXT NOT NULL,
    body_hash      TEXT NOT NULL, -- the item's normalised text hash at decision time (re-ask rule a)
    start_line     INTEGER,       -- the item's lines at decision time (re-ask rule b)
    end_line       INTEGER,
    decided_at     TEXT NOT NULL,
    CHECK ((via = 'carried') = (carried_from IS NOT NULL)),
    CHECK ((start_line IS NULL) = (end_line IS NULL))
) STRICT;
CREATE INDEX comment_decisions_by_item ON comment_decisions (item_id, decided_at);
CREATE UNIQUE INDEX comment_decisions_supersede_once ON comment_decisions (supersedes) WHERE supersedes IS NOT NULL;
CREATE TRIGGER comment_decisions_append_only BEFORE UPDATE ON comment_decisions
BEGIN SELECT RAISE(ABORT, 'decisions are append-only'); END;

-- T-L6: effects of a decision in `seq` order; claims and leases live on the claimed effects.
CREATE TABLE comment_effects (
    id                TEXT NOT NULL PRIMARY KEY,
    tenant_id         TEXT NOT NULL REFERENCES tenants (id),
    decision_id       TEXT NOT NULL REFERENCES comment_decisions (id) ON DELETE CASCADE,
    seq               INTEGER NOT NULL CHECK (seq >= 1),
    after_seq         INTEGER CHECK (after_seq IS NULL OR (after_seq >= 1 AND after_seq < seq)),
    kind              TEXT NOT NULL CHECK (kind IN ('fix', 'reply', 'resolve', 'pr_comment', 'follow_up_pr',
                                                    'upstream_issue', 'todo', 'dismiss_review')),
    executor          TEXT NOT NULL CHECK (executor IN ('agent', 'diffpilot_bot')),
    params            TEXT NOT NULL CHECK (json_valid(params)), -- EffectParams
    state             TEXT NOT NULL CHECK (state IN ('waiting', 'open', 'claimed', 'done', 'failed', 'cancelled')),
    claim_token_id    TEXT REFERENCES access_tokens (id),
    lease_until       TEXT,
    reference         TEXT,
    message           TEXT,
    seen_on_github_at TEXT,
    updated_at        TEXT NOT NULL,
    UNIQUE (decision_id, seq),
    CHECK ((state = 'claimed') = (claim_token_id IS NOT NULL)),
    CHECK ((claim_token_id IS NULL) = (lease_until IS NULL))
) STRICT;
CREATE INDEX comment_effects_by_state ON comment_effects (state, executor);

-- Agent reports per effect; the idempotency key makes a repeated report a no-op.
CREATE TABLE comment_effect_reports (
    id              INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    effect_id       TEXT NOT NULL REFERENCES comment_effects (id) ON DELETE CASCADE,
    token_id        TEXT NOT NULL REFERENCES access_tokens (id),
    outcome         TEXT NOT NULL CHECK (outcome IN ('done', 'failed', 'released')),
    reference       TEXT,
    message         TEXT,
    idempotency_key TEXT NOT NULL,
    at              TEXT NOT NULL,
    UNIQUE (token_id, idempotency_key)
) STRICT;
CREATE INDEX comment_effect_reports_by_effect ON comment_effect_reports (effect_id, id);

-- T-L7: carry-over log (carried decisions and repeats asked again) with the server-read evidence.
CREATE TABLE comment_carry_events (
    id                 TEXT NOT NULL PRIMARY KEY,
    tenant_id          TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id    INTEGER NOT NULL REFERENCES comment_prs (pull_request_id) ON DELETE CASCADE,
    item_id            TEXT NOT NULL REFERENCES comment_items (id) ON DELETE CASCADE,
    source_decision_id TEXT NOT NULL REFERENCES comment_decisions (id) ON DELETE CASCADE,
    kind               TEXT NOT NULL CHECK (kind IN ('carried', 'asked_again')),
    reason             TEXT CHECK (reason IN ('bot_edited', 'code_changed')),
    evidence           TEXT NOT NULL CHECK (json_valid(evidence)), -- CarryEvidence
    at                 TEXT NOT NULL,
    CHECK ((kind = 'asked_again') = (reason IS NOT NULL))
) STRICT;
CREATE INDEX comment_carry_events_by_pr ON comment_carry_events (pull_request_id, at);
CREATE INDEX comment_carry_events_by_time ON comment_carry_events (tenant_id, at);

-- T-L8: feed subscriptions of `review_loop_execute` tokens and their optional push target.
CREATE TABLE agent_subscriptions (
    token_id              TEXT NOT NULL PRIMARY KEY REFERENCES access_tokens (id) ON DELETE CASCADE,
    tenant_id             TEXT NOT NULL REFERENCES tenants (id),
    capabilities          TEXT NOT NULL CHECK (json_valid(capabilities) AND json_type(capabilities) = 'array'),
    last_seen_at          TEXT NOT NULL,
    push_kind             TEXT CHECK (push_kind IN ('ntfy', 'webhook')),
    push_url              TEXT,
    push_topic            TEXT,
    push_secret           BLOB, -- AES-GCM ciphertext
    last_delivery_at      TEXT,
    last_delivery_outcome TEXT CHECK (last_delivery_outcome IN ('delivered', 'failed')),
    last_error            TEXT,
    failing_since         TEXT,
    CHECK ((push_kind IS NULL) = (push_url IS NULL)),
    CHECK (push_kind IS NOT NULL OR (push_topic IS NULL AND push_secret IS NULL)),
    CHECK ((last_delivery_at IS NULL) = (last_delivery_outcome IS NULL))
) STRICT;

-- T-L9: watched pull requests per user.
CREATE TABLE pr_watches (
    user_id                 TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    pull_request_id         INTEGER NOT NULL REFERENCES pull_requests (id) ON DELETE CASCADE,
    tenant_id               TEXT NOT NULL REFERENCES tenants (id),
    since                   TEXT NOT NULL,
    probe                   TEXT CHECK (probe IS NULL OR json_valid(probe)), -- last probe values
    last_probe_at           TEXT,
    last_auto_pass_at       TEXT,
    last_auto_pass_questions INTEGER CHECK (last_auto_pass_questions >= 0),
    state                   TEXT NOT NULL CHECK (state IN ('active', 'waiting_for_bot_round', 'job_running',
                                                           'limit_reached', 'paused', 'ended')),
    ended_at                TEXT,
    PRIMARY KEY (user_id, pull_request_id),
    CHECK ((state = 'ended') = (ended_at IS NOT NULL)),
    CHECK ((last_auto_pass_at IS NULL) = (last_auto_pass_questions IS NULL))
) STRICT;
CREATE INDEX pr_watches_by_pr ON pr_watches (pull_request_id);

-- T-L10: per-user events (browser notifications, push channels).
CREATE TABLE user_events (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    user_id         TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN ('questions_ready', 'round_executed', 'test')),
    pull_request_id INTEGER REFERENCES pull_requests (id) ON DELETE CASCADE,
    count           INTEGER NOT NULL CHECK (count >= 0),
    text            TEXT NOT NULL,
    url             TEXT,
    at              TEXT NOT NULL,
    CHECK ((kind = 'test') = (pull_request_id IS NULL))
) STRICT;
CREATE INDEX user_events_by_user ON user_events (user_id, at);

-- T-L11: a user's push channels, at most one per kind.
CREATE TABLE notification_channels (
    id                    TEXT NOT NULL PRIMARY KEY,
    tenant_id             TEXT NOT NULL REFERENCES tenants (id),
    user_id               TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind                  TEXT NOT NULL CHECK (kind IN ('ntfy', 'webhook')),
    url                   TEXT NOT NULL,
    topic                 TEXT,
    secret                BLOB, -- AES-GCM ciphertext
    events                TEXT NOT NULL CHECK (json_valid(events) AND json_type(events) = 'array'),
    enabled               INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    last_delivery_at      TEXT,
    last_delivery_event   TEXT CHECK (last_delivery_event IN ('questions_ready', 'round_executed', 'test')),
    last_delivery_outcome TEXT CHECK (last_delivery_outcome IN ('delivered', 'failed')),
    last_error            TEXT,
    failing_since         TEXT,
    UNIQUE (user_id, kind),
    CHECK ((last_delivery_at IS NULL) = (last_delivery_outcome IS NULL))
) STRICT;

-- T-L12: deliveries of a user event to a channel, or of a decision to an agent push target; one row per attempt
-- schedule (kept; the planned system-wide retention applies later).
CREATE TABLE notification_deliveries (
    id              INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    user_event_id   TEXT REFERENCES user_events (id) ON DELETE CASCADE,
    decision_id     TEXT REFERENCES comment_decisions (id) ON DELETE CASCADE,
    channel_id      TEXT REFERENCES notification_channels (id) ON DELETE CASCADE,
    token_id        TEXT REFERENCES agent_subscriptions (token_id) ON DELETE CASCADE,
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'failed')),
    next_attempt_at TEXT,
    http_status     INTEGER,
    error           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    CHECK ((user_event_id IS NULL) = (channel_id IS NULL)),
    CHECK ((decision_id IS NULL) = (token_id IS NULL)),
    CHECK ((user_event_id IS NULL) <> (decision_id IS NULL)),
    CHECK ((state = 'pending') = (next_attempt_at IS NOT NULL))
) STRICT;
CREATE INDEX notification_deliveries_due ON notification_deliveries (next_attempt_at) WHERE state = 'pending';
CREATE UNIQUE INDEX notification_deliveries_once_per_channel ON notification_deliveries (user_event_id, channel_id)
    WHERE user_event_id IS NOT NULL;
CREATE UNIQUE INDEX notification_deliveries_once_per_target ON notification_deliveries (decision_id, token_id)
    WHERE decision_id IS NOT NULL;
