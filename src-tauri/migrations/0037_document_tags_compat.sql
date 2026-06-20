-- Compat migration for existing local installs.
--
-- Some customized databases already used migration versions 0033/0034 for OA
-- integration before upstream added 0033_document_tags.sql. Those databases
-- have _sqlx_migrations entries for version 33 but no document_tags table.
CREATE TABLE IF NOT EXISTS document_tags (
    id           TEXT PRIMARY KEY NOT NULL,
    document_id  TEXT NOT NULL,
    namespace    TEXT NOT NULL,
    value        TEXT NOT NULL,
    source       TEXT NOT NULL DEFAULT 'user',
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(document_id, namespace, value),
    FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_document_tags_doc ON document_tags(document_id);
