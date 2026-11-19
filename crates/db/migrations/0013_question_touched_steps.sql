-- The Tour steps that the lines of a lines-anchored question touch, as a JSON array of step ids: chosen when its job
-- is submitted and read when the job is claimed, so the claim does not rebuild the Tour. NULL: not chosen.
ALTER TABLE questions ADD COLUMN touched_steps TEXT CHECK (touched_steps IS NULL OR json_valid(touched_steps));
