-- Items sent to judge_work jobs per refresh, across its ranking attempts (plan §B.4, OD19): the per-refresh and
-- per-day ranking caps count what is sent to the LLM, including retried, rejected and superseded work.
ALTER TABLE dashboard_refreshes ADD COLUMN sent INTEGER NOT NULL DEFAULT 0 CHECK (sent >= 0);
