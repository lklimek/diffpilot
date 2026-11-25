-- An App installation lookup identifies the installation but does not return the repository ID.
ALTER TABLE app_repositories RENAME TO app_repositories_with_ids;
DROP INDEX app_repositories_by_installation;

CREATE TABLE app_repositories (
    owner           TEXT NOT NULL COLLATE NOCASE,
    name            TEXT NOT NULL COLLATE NOCASE,
    repository_id   INTEGER UNIQUE,
    installation_id INTEGER NOT NULL,
    suspended       INTEGER NOT NULL DEFAULT 0 CHECK (suspended IN (0, 1)),
    checked_at      TEXT NOT NULL,
    PRIMARY KEY (owner, name)
) STRICT;

INSERT INTO app_repositories SELECT * FROM app_repositories_with_ids;
DROP TABLE app_repositories_with_ids;
CREATE INDEX app_repositories_by_installation ON app_repositories (installation_id);
