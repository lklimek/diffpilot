-- The host-validated candidate from the latest accepted primary assessment.
ALTER TABLE comment_items ADD COLUMN import_candidate TEXT CHECK (json_valid(import_candidate));
ALTER TABLE comment_decision_origins ADD COLUMN author_withdrawal INTEGER NOT NULL DEFAULT 0
    CHECK (author_withdrawal IN (0, 1) AND (author_withdrawal = 0 OR origin = 'imported_github'));

CREATE TABLE comment_visits (
    tenant_id       TEXT NOT NULL,
    user_id         TEXT NOT NULL,
    pull_request_id INTEGER NOT NULL,
    visited_at      TEXT NOT NULL,
    PRIMARY KEY (tenant_id, user_id, pull_request_id),
    FOREIGN KEY (tenant_id, user_id) REFERENCES memberships (tenant_id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (pull_request_id, tenant_id) REFERENCES pull_requests (id, tenant_id) ON DELETE CASCADE
) STRICT;
