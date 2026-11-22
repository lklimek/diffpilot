-- Review loop owner actions (ADR-023, docs/review-workflow-plan.md §3.2): Undo and "Skip for now". Additive: new
-- tables only; decisions stay append-only (an undo is its own row).

-- An undone decision: its effects were cancelled before any was claimed and its item is asked again. At most one
-- undo per decision; a later decision supersedes the undone one as usual.
CREATE TABLE comment_decision_undos (
    decision_id TEXT NOT NULL PRIMARY KEY REFERENCES comment_decisions (id) ON DELETE CASCADE,
    tenant_id   TEXT NOT NULL REFERENCES tenants (id),
    user_id     TEXT NOT NULL REFERENCES users (id),
    at          TEXT NOT NULL
) STRICT;

-- "Skip for now" of one reader: the item comes last in that reader's queue while it stays at `revision`.
CREATE TABLE comment_item_skips (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    user_id   TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_id   TEXT NOT NULL REFERENCES comment_items (id) ON DELETE CASCADE,
    revision  INTEGER NOT NULL CHECK (revision >= 1),
    at        TEXT NOT NULL,
    PRIMARY KEY (user_id, item_id)
) STRICT;
