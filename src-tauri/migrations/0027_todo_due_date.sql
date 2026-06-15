-- 2026-06-14 · 待办可选日期(case_todos.due_date)
-- 作者反馈:给待办加一个可选的"重要日期",带日期的待办既在首页待办汇总显示日期,
-- 又自动汇入首页日程日历(补上"日历无法手动加日程"的缺口 —— 手动日程 = 带日期的待办)。
-- 格式:ISO 日期字符串 "YYYY-MM-DD";NULL = 该待办无日期(普通待办,不进日历)。
ALTER TABLE case_todos ADD COLUMN due_date TEXT;

CREATE INDEX IF NOT EXISTS idx_case_todos_due ON case_todos(due_date) WHERE due_date IS NOT NULL;
