-- Latest known lifecycle of a stored GitHub pull request.
ALTER TABLE pull_requests ADD COLUMN lifecycle TEXT CHECK (lifecycle IN ('open', 'closed', 'merged'));
ALTER TABLE pull_requests ADD COLUMN lifecycle_updated_at TEXT;
ALTER TABLE pull_requests ADD COLUMN lifecycle_read_at TEXT;

-- A successful review-loop read is lifecycle evidence even in histories created before this migration.
WITH snapshots AS (
    SELECT c.pull_request_id, c.read_at,
           c.github_snapshot ->> '$.state' AS state,
           c.github_snapshot ->> '$.updated_at' AS updated_at
    FROM comment_prs c
    WHERE c.read_at IS NOT NULL
)
UPDATE pull_requests AS p
SET lifecycle = lower(c.state),
    lifecycle_updated_at = CASE length(c.updated_at)
        WHEN 20 THEN substr(c.updated_at, 1, 19) || '.000000Z'
        WHEN 24 THEN substr(c.updated_at, 1, 23) || '000Z'
        WHEN 27 THEN c.updated_at
        WHEN 30 THEN substr(c.updated_at, 1, 26) || 'Z'
    END,
    lifecycle_read_at = c.read_at
FROM snapshots c
WHERE c.pull_request_id = p.id AND c.state IN ('OPEN', 'CLOSED', 'MERGED');

-- Keep the latest signed version even if the first PR read has yet to create its row.
CREATE TABLE github_pr_lifecycle_signals (
    repository_id INTEGER NOT NULL CHECK (repository_id > 0),
    number INTEGER NOT NULL CHECK (number > 0),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('open', 'closed', 'merged')),
    updated_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    conflicted INTEGER NOT NULL DEFAULT 0 CHECK (conflicted IN (0, 1)),
    PRIMARY KEY (repository_id, number)
) STRICT;
CREATE INDEX github_pr_lifecycle_by_receipt ON github_pr_lifecycle_signals (received_at);

CREATE VIEW pull_request_lifecycle AS
SELECT p.id,
       CASE WHEN s.updated_at IS NOT NULL
                  AND (p.lifecycle_updated_at IS NULL OR p.lifecycle_updated_at < s.updated_at)
            THEN s.lifecycle ELSE p.lifecycle END AS lifecycle,
       s.updated_at IS NOT NULL
           AND (p.lifecycle_updated_at IS NULL OR p.lifecycle_updated_at < s.updated_at
                OR (p.lifecycle_updated_at = s.updated_at AND p.lifecycle_read_at <= s.received_at))
           AND (s.conflicted OR COALESCE(p.lifecycle_updated_at = s.updated_at AND p.lifecycle <> s.lifecycle, 0))
           AS needs_read
FROM pull_requests p
JOIN repos r ON r.id = p.repo_id
LEFT JOIN github_pr_lifecycle_signals s ON s.repository_id = r.github_id AND s.number = p.number;
