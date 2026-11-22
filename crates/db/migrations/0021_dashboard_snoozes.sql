-- Snoozed dashboard items (owner SN1): one row per user and item key. The item is left out of the user's ranking
-- until `until`, or until a review request to the user or a mention of the user newer than `snoozed_at`; ended
-- snoozes are deleted. Number 0021: 0019 and 0020 are reserved by parallel work streams.
CREATE TABLE dashboard_snoozes (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    snoozed_at TEXT NOT NULL,
    until TEXT NOT NULL CHECK (until > snoozed_at),
    PRIMARY KEY (tenant_id, user_id, item_key)
) STRICT;
