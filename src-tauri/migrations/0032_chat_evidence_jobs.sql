-- 2026-06-14 · 聊天录屏取证任务表(chat_evidence_jobs)
-- start_chat_evidence_extraction 往此表插 pending 记录,
-- 后台 spawn python3 wechat_evidence.py interval-pdf 处理,
-- 完成后更新 status/output_dir/pdf_path。

CREATE TABLE IF NOT EXISTS chat_evidence_jobs (
    id            TEXT PRIMARY KEY,
    case_id       TEXT NOT NULL REFERENCES cases(id),
    video_path    TEXT NOT NULL,
    preset        TEXT NOT NULL DEFAULT '少漏内容',  -- 少漏内容 / 平衡 / 更少页
    status        TEXT NOT NULL DEFAULT 'pending',   -- pending / running / completed / failed
    output_dir    TEXT,
    pdf_path      TEXT,
    frame_count   INTEGER,
    elapsed_ms    INTEGER,
    error         TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_chat_evidence_jobs_case_id ON chat_evidence_jobs(case_id);
