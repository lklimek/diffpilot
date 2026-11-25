-- Installation-wide GitHub App infrastructure (ADR-026).
CREATE TABLE github_deliveries (
    delivery_id     TEXT NOT NULL PRIMARY KEY CHECK (
        length(CAST(delivery_id AS BLOB)) = 36 AND length(replace(delivery_id, '-', '')) = 32
        AND substr(delivery_id, 9, 1) = '-' AND substr(delivery_id, 14, 1) = '-'
        AND substr(delivery_id, 19, 1) = '-' AND substr(delivery_id, 24, 1) = '-'
        AND delivery_id NOT GLOB '*[^0-9a-f-]*'
    ),
    body_sha256     TEXT NOT NULL CHECK (
        length(CAST(body_sha256 AS BLOB)) = 64 AND length(body_sha256) = 64
        AND body_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    event           TEXT NOT NULL CHECK (length(CAST(event AS BLOB)) BETWEEN 1 AND 64 AND event IN (
        'pull_request', 'pull_request_review', 'pull_request_review_comment', 'pull_request_review_thread',
        'issue_comment', 'issues', 'check_suite', 'check_run', 'status', 'ping', 'installation', 'installation_repositories'
    )),
    action          TEXT CHECK (action IS NULL OR (length(CAST(action AS BLOB)) BETWEEN 1 AND 64 AND CASE event
        WHEN 'pull_request' THEN action IN (
            'opened', 'closed', 'reopened', 'synchronize', 'edited', 'ready_for_review', 'converted_to_draft',
            'review_requested', 'review_request_removed', 'labeled', 'unlabeled', 'assigned', 'unassigned',
            'milestoned', 'demilestoned', 'auto_merge_enabled', 'auto_merge_disabled', 'enqueued', 'dequeued'
        )
        WHEN 'pull_request_review' THEN action IN ('submitted', 'edited', 'dismissed')
        WHEN 'pull_request_review_comment' THEN action IN ('created', 'edited', 'deleted')
        WHEN 'pull_request_review_thread' THEN action IN ('resolved', 'unresolved')
        WHEN 'issue_comment' THEN action IN ('created', 'edited', 'deleted')
        WHEN 'issues' THEN action IN (
            'opened', 'edited', 'closed', 'reopened', 'assigned', 'unassigned', 'labeled', 'unlabeled',
            'milestoned', 'demilestoned', 'transferred', 'deleted'
        )
        WHEN 'check_suite' THEN action = 'completed'
        WHEN 'check_run' THEN action = 'completed'
        WHEN 'installation' THEN action IN ('created', 'deleted', 'suspend', 'unsuspend', 'new_permissions_accepted')
        WHEN 'installation_repositories' THEN action IN ('added', 'removed')
        ELSE 0 END
    )),
    installation_id INTEGER,
    changes         TEXT NOT NULL CHECK (json_valid(changes) AND json_type(changes) = 'array'),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'done', 'ignored', 'not_allowed', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    received_at     TEXT NOT NULL,
    processed_at    TEXT,
    error           TEXT CHECK (error IN (
        'invalid_change', 'github_unavailable', 'dispatch_failed', 'storage_failed', 'timeout'
    ))
) STRICT;
CREATE INDEX github_deliveries_by_body ON github_deliveries (body_sha256, received_at);
CREATE INDEX github_deliveries_by_received ON github_deliveries (received_at);

-- Live coverage for tracked repositories (W13).
CREATE TABLE app_repositories (
    owner           TEXT NOT NULL COLLATE NOCASE,
    name            TEXT NOT NULL COLLATE NOCASE,
    repository_id   INTEGER NOT NULL UNIQUE,
    installation_id INTEGER NOT NULL,
    suspended       INTEGER NOT NULL DEFAULT 0 CHECK (suspended IN (0, 1)),
    checked_at      TEXT NOT NULL,
    PRIMARY KEY (owner, name)
) STRICT;
CREATE INDEX app_repositories_by_installation ON app_repositories (installation_id);
