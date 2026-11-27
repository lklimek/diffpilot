CREATE TABLE agent_comment_submissions (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    pull_request_id INTEGER NOT NULL REFERENCES comment_prs (pull_request_id) ON DELETE CASCADE,
    token_id        TEXT NOT NULL,
    submission_key  TEXT NOT NULL,
    fingerprint     TEXT NOT NULL,
    agent           TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    item_id         TEXT UNIQUE REFERENCES comment_items (id) ON DELETE CASCADE,
    conversation    TEXT,
    target_url      TEXT,
    annotation      TEXT,
    UNIQUE (pull_request_id, token_id, submission_key),
    CHECK ((item_id IS NULL) = (conversation IS NOT NULL)),
    CHECK ((item_id IS NULL) = (target_url IS NOT NULL)),
    CHECK ((item_id IS NULL) = (annotation IS NOT NULL))
) STRICT;

CREATE INDEX agent_comment_annotations_by_pr ON agent_comment_submissions
    (pull_request_id, conversation, created_at, id) WHERE conversation IS NOT NULL;
