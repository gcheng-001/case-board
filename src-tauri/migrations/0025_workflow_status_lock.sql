-- 2026-06-13 · 工作流状态锁(workflow_status_locked)
-- 反馈人「胡彬律师」:结案后状态被刷新掉。根因 = 全局抽 write_table_to_cases 用
-- `workflow_status = COALESCE(LLM值, 现值)`,重新扫描/分析时 LLM 重推状态把用户手设的覆盖掉。
--
-- locked=1 表示用户在卡片右上角手动选过状态 → 全局抽不再覆盖(用 CASE WHEN 跳过);
-- locked=0 表示走自动推断(用户把状态设回"自动/未设"时清回 0)。
-- 契合既有约定「workflow_status 非 null = 用户手工选过,优先取用户值」。
ALTER TABLE cases ADD COLUMN workflow_status_locked INTEGER NOT NULL DEFAULT 0;
