CREATE TABLE comment_item_marks (
    id              TEXT NOT NULL PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants (id),
    item_id         TEXT NOT NULL REFERENCES comment_items (id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL REFERENCES users (id),
    mark            TEXT NOT NULL CHECK (mark = 'not_a_review_point'),
    reason          TEXT,
    follows_mark_id TEXT REFERENCES comment_item_marks (id),
    marked_at       TEXT NOT NULL,
    undone_at       TEXT,
    undone_by       TEXT REFERENCES users (id),
    CHECK ((undone_at IS NULL) = (undone_by IS NULL))
) STRICT;

CREATE UNIQUE INDEX comment_item_marks_one_active ON comment_item_marks (item_id)
    WHERE undone_at IS NULL;
CREATE INDEX comment_item_marks_page ON comment_item_marks (tenant_id, marked_at DESC, id DESC)
    WHERE undone_at IS NULL;
CREATE INDEX comment_item_marks_followers ON comment_item_marks (follows_mark_id, item_id);

CREATE TRIGGER comment_item_marks_append_only BEFORE UPDATE ON comment_item_marks
WHEN OLD.undone_at IS NOT NULL OR NEW.undone_at IS NULL OR NEW.undone_by IS NULL
    OR NEW.id IS NOT OLD.id OR NEW.tenant_id IS NOT OLD.tenant_id
    OR NEW.item_id IS NOT OLD.item_id OR NEW.user_id IS NOT OLD.user_id
    OR NEW.mark IS NOT OLD.mark OR NEW.reason IS NOT OLD.reason
    OR NEW.follows_mark_id IS NOT OLD.follows_mark_id OR NEW.marked_at IS NOT OLD.marked_at
BEGIN SELECT RAISE(ABORT, 'marks can only be undone once'); END;
