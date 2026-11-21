-- Dependencies view (ADR-021). Additive: widens the document kinds with `dependencies` in place (as 0010 widened the
-- language lists: widening a CHECK accepts every stored row) and adds the installation-wide controlled owners.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
SET sql = replace(sql, '''memcan_profile''))', '''memcan_profile'', ''dependencies''))')
WHERE type = 'table' AND name = 'documents';
PRAGMA writable_schema = RESET;
-- Fails (and rolls the migration back) unless the definition now allows `dependencies`.
CREATE TABLE dependencies_schema_reload (widened INTEGER NOT NULL CHECK (widened = 1)) STRICT;
INSERT INTO dependencies_schema_reload
SELECT count(*) FROM sqlite_schema
WHERE type = 'table' AND name = 'documents' AND instr(sql, '''memcan_profile'', ''dependencies''))') > 0;
DROP TABLE dependencies_schema_reload;

-- Controlled GitHub owners: their repositories are "yours" in the Dependencies view.
CREATE TABLE controlled_owners (
    tenant_id TEXT NOT NULL REFERENCES tenants (id),
    owner     TEXT NOT NULL COLLATE NOCASE CHECK (length(owner) BETWEEN 1 AND 39),
    PRIMARY KEY (tenant_id, owner)
) STRICT;

-- Who saved the controlled owners last and when; no row before the first save.
CREATE TABLE controlled_owners_changed (
    tenant_id  TEXT NOT NULL PRIMARY KEY REFERENCES tenants (id),
    changed_by TEXT NOT NULL,
    changed_at TEXT NOT NULL
) STRICT;
