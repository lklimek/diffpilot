ALTER TABLE users ADD COLUMN llm_job_slots INTEGER NOT NULL DEFAULT 1
    CHECK (llm_job_slots BETWEEN 1 AND 65535);

CREATE INDEX jobs_active_by_payer ON jobs (payer_id)
    WHERE state IN ('claimed', 'running');
