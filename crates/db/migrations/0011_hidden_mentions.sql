-- A hide belongs to a post, not an item; a later post remains visible.
CREATE TABLE dashboard_hidden_mentions (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    mention_id TEXT NOT NULL,
    hidden_at TEXT NOT NULL,
    PRIMARY KEY (tenant_id, user_id, mention_id)
);
