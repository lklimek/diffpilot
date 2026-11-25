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
    event           TEXT NOT NULL CHECK (
        length(CAST(event AS BLOB)) BETWEEN 1 AND 64 AND instr(event, char(0)) = 0
        AND event NOT GLOB '*[^a-z_]*'
    ),
    action          TEXT CHECK (
        length(CAST(action AS BLOB)) BETWEEN 1 AND 64 AND instr(action, char(0)) = 0
        AND action NOT GLOB '*[^a-z_]*'
    ),
    installation_id INTEGER,
    changes         TEXT NOT NULL CHECK (json_valid(changes) AND json_type(changes) = 'array'),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'done', 'ignored', 'not_allowed', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    received_at     TEXT NOT NULL,
    processed_at    TEXT,
    error           TEXT CHECK (
        length(CAST(error AS BLOB)) BETWEEN 1 AND 64 AND instr(error, char(0)) = 0
        AND error NOT GLOB '*[^a-z_]*'
    )
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
