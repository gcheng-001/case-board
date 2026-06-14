-- 团队版 Phase 2(编辑动作)· 2026-06-10
-- 设计:docs/提案-团队版-2026-06-10.md §3.2bis 编辑权
--
-- team_edits:跨成员编辑请求(签名留言式接力转交)。
--   有编辑权的成员改队友案件的登记字段 → 生成一条 pending 请求随 gossip 传播 →
--   案件所有人的 App 收到后验权限并应用(改 cases 表,回填 prev_value 供撤销)→
--   状态升级随 gossip 回传。状态只升不降:pending(0) → applied/rejected/reverted。
--   field 仅允许登记表层:'workflow_status'(改状态,落 cases 表)| 'note'(团队备注,
--   不动所有人案件数据,仅团队层展示)。

CREATE TABLE IF NOT EXISTS team_edits (
    id               TEXT PRIMARY KEY,
    team_id          TEXT NOT NULL,
    editor_id        TEXT NOT NULL,
    editor_name      TEXT NOT NULL,
    target_member_id TEXT NOT NULL,
    case_id          TEXT NOT NULL,
    case_name        TEXT NOT NULL,
    field            TEXT NOT NULL,
    value            TEXT NOT NULL,
    prev_value       TEXT,
    status           TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    applied_at       TEXT
);

CREATE INDEX IF NOT EXISTS team_edits_target_idx ON team_edits (target_member_id, status);
