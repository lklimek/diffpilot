-- Watch detection state (ADR-023, docs/review-workflow-plan.md §2.8). Additive: two nullable columns.
-- pass_due_at:  since when an automatic assessment pass is due (a probe found a change); NULL when none is due. The
--               90-minute wait for the bot's round counts from it.
-- notified_for: the round the last "questions ready" event of this watch was for (`round:<n>`, or `head:<sha>` while
--               the pull request has no bot round), so a round notifies once.
ALTER TABLE pr_watches ADD COLUMN pass_due_at TEXT;
ALTER TABLE pr_watches ADD COLUMN notified_for TEXT;
