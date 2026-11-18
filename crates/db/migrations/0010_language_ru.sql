-- Russian LLM texts (`LlmLanguage::Ru`): widens the language CHECK of `users` and `analyses` in place. Widening a
-- CHECK accepts every stored row, so the table definitions are edited instead of rebuilding both tables and their
-- many children (SQLite's documented procedure for loosening constraints). `RESET` reloads the definitions on this
-- connection; the scratch table checks both edits and bumps the schema cookie, so every other open connection
-- reloads them too.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
SET sql = replace(sql, 'CHECK (language IN (''en'', ''pl''))', 'CHECK (language IN (''en'', ''pl'', ''ru''))')
WHERE type = 'table' AND name IN ('users', 'analyses');
PRAGMA writable_schema = RESET;
-- Fails (and rolls the migration back) unless both definitions now allow `ru`.
CREATE TABLE language_ru_schema_reload (widened INTEGER NOT NULL CHECK (widened = 2)) STRICT;
INSERT INTO language_ru_schema_reload
SELECT count(*) FROM sqlite_schema
WHERE type = 'table' AND name IN ('users', 'analyses')
  AND instr(sql, 'CHECK (language IN (''en'', ''pl'', ''ru''))') > 0;
DROP TABLE language_ru_schema_reload;
