-- Installation-wide GitHub App infrastructure (ADR-026).
CREATE TABLE github_deliveries (
    delivery_id     TEXT NOT NULL PRIMARY KEY,
    body_sha256     TEXT NOT NULL,
    event           TEXT NOT NULL,
    action          TEXT,
    installation_id INTEGER,
    changes         TEXT NOT NULL CHECK (json_valid(changes) AND json_type(changes) = 'array'),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'done', 'ignored', 'not_allowed', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    received_at     TEXT NOT NULL,
    processed_at    TEXT,
    error           TEXT -- typed processing code; never a payload or error message
) STRICT;
CREATE INDEX github_deliveries_by_body ON github_deliveries (body_sha256, received_at);
CREATE INDEX github_deliveries_by_received ON github_deliveries (received_at);

CREATE TABLE app_installations_allowed (
    installation_id      INTEGER NOT NULL PRIMARY KEY,
    account_id           INTEGER NOT NULL,
    account_login        TEXT NOT NULL COLLATE NOCASE,
    repository_selection TEXT NOT NULL CHECK (repository_selection IN ('all', 'selected')),
    resolved_at          TEXT NOT NULL
) STRICT;

CREATE TABLE app_repositories (
    owner           TEXT NOT NULL COLLATE NOCASE,
    name            TEXT NOT NULL COLLATE NOCASE,
    installation_id INTEGER NOT NULL REFERENCES app_installations_allowed (installation_id) ON DELETE CASCADE,
    suspended       INTEGER NOT NULL DEFAULT 0 CHECK (suspended IN (0, 1)),
    checked_at      TEXT NOT NULL,
    PRIMARY KEY (owner, name)
) STRICT;
CREATE INDEX app_repositories_by_installation ON app_repositories (installation_id);
