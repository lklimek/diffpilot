-- One most recent PR page visit per member and pull request.
CREATE TABLE pr_page_openings (
    tenant_id       TEXT NOT NULL,
    user_id         TEXT NOT NULL,
    pull_request_id INTEGER NOT NULL,
    last_opened_at  TEXT NOT NULL,
    PRIMARY KEY (tenant_id, user_id, pull_request_id),
    FOREIGN KEY (tenant_id, user_id) REFERENCES memberships (tenant_id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (pull_request_id, tenant_id) REFERENCES pull_requests (id, tenant_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX pr_page_openings_by_reader ON pr_page_openings (tenant_id, user_id, last_opened_at DESC, pull_request_id);
CREATE INDEX pr_page_openings_by_pr ON pr_page_openings (pull_request_id, tenant_id);
