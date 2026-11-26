-- Personal overrides inherit unset fields from the legacy installation selection.
CREATE TABLE user_llm_selection (
    user_id TEXT NOT NULL PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    model TEXT,
    reasoning_effort TEXT CHECK (reasoning_effort IN ('low', 'medium', 'high')),
    judge_effort TEXT CHECK (judge_effort IN ('low', 'medium', 'high')),
    ask_effort TEXT CHECK (ask_effort IN ('low', 'medium', 'high')),
    changed_by TEXT NOT NULL,
    changed_at TEXT NOT NULL,
    CHECK ((model IS NULL) = (reasoning_effort IS NULL))
) STRICT;
