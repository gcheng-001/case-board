-- 2026-06-13 · 案件待办清单(case_todos · per-case todo list)
-- 反馈人「胡彬律师」:每个案件一个手动输入的待办清单,首页汇总,仿滴答清单(打钩完成即隐藏)。
--
-- done=1 表示已完成(前端默认隐藏,但**不删行**,可在「已完成」里查看/撤销)。
CREATE TABLE IF NOT EXISTS case_todos (
    id          TEXT PRIMARY KEY NOT NULL,
    case_id     TEXT NOT NULL,
    title       TEXT NOT NULL,
    done        INTEGER NOT NULL DEFAULT 0,   -- 0=未完成 1=已完成
    done_at     TEXT,                          -- 打钩完成时间
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_case_todos_case_done ON case_todos(case_id, done);
