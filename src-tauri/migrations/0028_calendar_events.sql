-- 2026-06-14 · 独立日历日程(calendar_events)
-- 作者反馈:日历某天右键「添加日程」,自由写一条提醒/事件,可删除。
-- 与 case_todos 区别:**不绑定案件**(纯个人提醒,像普通 To-Do),只挂在某个日期上。
-- 日历显示 = 案件自动抽取(开庭/续封等) + 带日期的待办 + 这里的独立日程。
CREATE TABLE IF NOT EXISTS calendar_events (
    id          TEXT PRIMARY KEY NOT NULL,
    date        TEXT NOT NULL,                 -- ISO 日期 "YYYY-MM-DD"
    title       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_calendar_events_date ON calendar_events(date);
