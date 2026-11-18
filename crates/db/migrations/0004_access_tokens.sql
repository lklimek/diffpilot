-- Personal access tokens (ADR-019, docs/v1.1-dashboard-plan.md §D.1 T-6). Only the SHA-256 of the whole token
-- `srp_<id>_<secret>` is stored; `id` is its public part. Additive.
CREATE TABLE access_tokens (
    id           TEXT NOT NULL PRIMARY KEY CHECK (length(id) = 8),
    tenant_id    TEXT NOT NULL REFERENCES tenants (id),
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
    token_hash   BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    -- TokenScope[]
    scopes       TEXT NOT NULL CHECK (json_valid(scopes) AND json_type(scopes) = 'array'),
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at   TEXT
) STRICT;

CREATE INDEX access_tokens_by_user ON access_tokens (user_id);
-- Names are unique among a user's tokens that are not revoked.
CREATE UNIQUE INDEX access_tokens_one_name_per_user ON access_tokens (user_id, name) WHERE revoked_at IS NULL;
