-- Automatic retries of a failed flow: after `auto_retries` reaches the limit the flow stays failed until a user
-- requests it. Additive: existing rows keep their demand and failure and start at 0.
ALTER TABLE flow_work ADD COLUMN auto_retries INTEGER NOT NULL DEFAULT 0 CHECK (auto_retries >= 0);
