-- Items a complete read no longer produces (docs/review-workflow-plan.md §2.1). Additive: one nullable column.
-- gone_at: when a complete read of the pull request first no longer produced the item (deleted on GitHub, or no
--          longer an item by the read rules); NULL while reads produce it. The row and its decisions stay.
ALTER TABLE comment_items ADD COLUMN gone_at TEXT;
