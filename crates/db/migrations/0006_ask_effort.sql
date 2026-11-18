-- Reasoning effort of Ask jobs (docs/contracts/db-schema.md), separate from the installation-wide effort of the
-- analysis jobs. Additive: a stored selection gains the default `medium`.
ALTER TABLE llm_selection ADD COLUMN ask_effort TEXT NOT NULL DEFAULT 'medium'
    CHECK (ask_effort IN ('low', 'medium', 'high'));
