-- Verified GitHub signals invalidate the last Comments read, even when nobody is viewing the PR.
ALTER TABLE comment_prs ADD COLUMN github_dirty_at TEXT;
ALTER TABLE comment_prs ADD COLUMN github_head TEXT;
ALTER TABLE comment_prs ADD COLUMN github_head_at TEXT;
