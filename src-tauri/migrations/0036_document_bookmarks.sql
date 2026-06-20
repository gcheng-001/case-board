-- 源文件看板:PDF 页码书签(2026-06-20)。
CREATE TABLE IF NOT EXISTS document_bookmarks (
    id           TEXT PRIMARY KEY NOT NULL,
    document_id  TEXT NOT NULL,
    page         INTEGER NOT NULL,
    label        TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_document_bookmarks_doc ON document_bookmarks(document_id);
