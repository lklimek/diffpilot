-- Known bots per user (docs/contracts/db-schema.md): logins treated as bots besides GitHub's Bot accounts and
-- `[bot]` logins. Additive: existing rows gain the default list, the review bots that were built in before.
ALTER TABLE dashboard_settings ADD COLUMN known_bots TEXT NOT NULL
    DEFAULT '["thepastaclaw","Claudius-Maginificent","DCG-Claude"]'
    CHECK (json_valid(known_bots) AND json_type(known_bots) = 'array');
