ALTER TABLE comment_visits ADD COLUMN previous_visited_at TEXT;

CREATE TABLE comment_linked_account_pins (
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    login TEXT NOT NULL COLLATE NOCASE,
    github_id INTEGER NOT NULL CHECK (github_id > 0),
    PRIMARY KEY (tenant_id, user_id, login),
    FOREIGN KEY (tenant_id, user_id) REFERENCES memberships (tenant_id, user_id) ON DELETE CASCADE
) STRICT;

ALTER TABLE comment_effects ADD COLUMN after_primary_effect TEXT REFERENCES comment_effects (id) ON DELETE SET NULL;
