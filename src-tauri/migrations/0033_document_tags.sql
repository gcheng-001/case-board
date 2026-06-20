-- 源文件看板重构 Phase 3:文档标记层(2026-06-19)。
-- 只动元数据,挂 documents.id;documents 是软删除(deleted_at),重扫保留同行 id
-- → 标记刷新/失联都不丢(FK ON DELETE CASCADE 仅在硬删案件时级联清理)。
--
-- 两条独立标记轴(namespace):
--   importance:值 重要 / 忽略(普通 = 不打标 = 默认)。**单值**,由应用层保证
--              (set 前先删该 doc 同 namespace 旧值)—— 同一文件不能既重要又忽略。
--   party_side:值 原告 / 被告 / 第三人。**可多值**(一份材料可能同时涉原告与被告)。
-- source:'user'(人工)/ 'ai_suggest'(Phase 3b AI 自动分类建议,人确认后转 user)。
CREATE TABLE document_tags (
    id           TEXT PRIMARY KEY NOT NULL,
    document_id  TEXT NOT NULL,
    namespace    TEXT NOT NULL,                       -- 'importance' | 'party_side'
    value        TEXT NOT NULL,                        -- 重要|忽略 / 原告|被告|第三人
    source       TEXT NOT NULL DEFAULT 'user',         -- 'user' | 'ai_suggest'
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(document_id, namespace, value),
    FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX idx_document_tags_doc ON document_tags(document_id);
