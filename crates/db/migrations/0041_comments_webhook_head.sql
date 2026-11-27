-- Verified GitHub signals invalidate the last Comments read, even when nobody is viewing the PR.
ALTER TABLE comment_prs ADD COLUMN github_dirty_at TEXT;
ALTER TABLE comment_prs ADD COLUMN github_head TEXT;
ALTER TABLE comment_prs ADD COLUMN github_head_at TEXT;
ALTER TABLE comment_prs ADD COLUMN github_head_before TEXT;

-- A follow-up issue's creation reservation survives lease expiry and reports.
ALTER TABLE comment_effects ADD COLUMN issue_attempt_id TEXT;

-- Widen the CHECK in place, as in 0010, preserving the decisions table and its foreign keys and triggers.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
SET sql = replace(sql, '''upstream_issue'', ''todo'', ''merge_with_follow_ups''',
                       '''follow_up_issue'', ''upstream_issue'', ''todo'', ''merge_with_follow_ups''')
WHERE type = 'table' AND name = 'comment_decisions';
PRAGMA writable_schema = RESET;

-- Reload the definition on every connection and roll back if the expected CHECK was not widened.
CREATE TABLE follow_up_issue_schema_reload (widened INTEGER NOT NULL CHECK (widened = 1)) STRICT;
INSERT INTO follow_up_issue_schema_reload
SELECT count(*) FROM sqlite_schema
WHERE type = 'table' AND name = 'comment_decisions'
  AND instr(sql, '''follow_up_issue'', ''upstream_issue'', ''todo'', ''merge_with_follow_ups''') > 0;
DROP TABLE follow_up_issue_schema_reload;
