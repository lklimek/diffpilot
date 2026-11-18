-- Russian LLM texts (`LlmLanguage::Ru`): widens the language CHECK of `users` and `analyses` in place. Widening a
-- CHECK accepts every stored row, so the table definitions are edited instead of rebuilding both tables and their
-- many children (SQLite's documented procedure for loosening constraints). `RESET` reloads the definitions on this
-- connection; the scratch table bumps the schema cookie, so every other open connection reloads them too.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
SET sql = replace(sql, 'CHECK (language IN (''en'', ''pl''))', 'CHECK (language IN (''en'', ''pl'', ''ru''))')
WHERE type = 'table' AND name IN ('users', 'analyses');
PRAGMA writable_schema = RESET;
CREATE TABLE language_ru_schema_reload (x INTEGER) STRICT;
DROP TABLE language_ru_schema_reload;
