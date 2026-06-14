-- 2026-06-11 · 审级模型(用户反馈:二审案号识别不到、首页不更新、劳动仲裁机关默认成法院)
--
-- 一个纠纷的审判程序生命线:[仲裁] → 一审 → 二审 → [再审]。
-- 此前 1 案件 = 1 案号(cases.agg_case_no 单值),多审级被压成一条记录。
-- 本表每个审级一条;cases.agg_* 语义改为「当前(最新)审级」的快照,首页卡照旧读它。
-- 执行是审判后的另一维度(已有执行模块/case_payments),不进本表。
--
-- 字段:
--   level          - '仲裁' / '一审' / '二审' / '再审'(边缘场景取最近枚举 + note 兜底)
--   seq            - 排序号,约定 仲裁=1 一审=2 二审=3 再审=4;发回重审等续排(5,6...);最大者=当前
--   case_no        - 该审级案号
--   authority      - 承办机关全称(法院 / 仲裁委员会)
--   authority_type - '法院' / '仲裁委' / '其他'
--   handlers       - JSON [{name,role,phone}](法官 / 仲裁员 / 书记员)
--   party_roles    - JSON [{name,role,is_our_side,note}] 该审级当事人称谓;
--                    二审 role=上诉人/被上诉人,note='原审被告' 之类对应关系(文书首部自带,LLM 直抽不推断)
--   filed_at       - 该审级立案/受理日(YYYY-MM-DD)
--   result         - 该审级结果(判决/裁决/调解,自由文本)
--   note           - 边缘场景备注(发回重审 / 管辖异议 / 分案等)
--   is_current     - 1=当前审级(首页/详情页置顶取它)
--   source         - 'llm' / 'user';重抽时只覆盖 llm 行,user 手加的不动

CREATE TABLE IF NOT EXISTS case_instances (
    id              TEXT PRIMARY KEY NOT NULL,
    case_id         TEXT NOT NULL,
    level           TEXT NOT NULL,
    seq             INTEGER NOT NULL,
    case_no         TEXT,
    authority       TEXT,
    authority_type  TEXT,
    handlers        TEXT,
    party_roles     TEXT,
    filed_at        TEXT,
    result          TEXT,
    note            TEXT,
    is_current      INTEGER NOT NULL DEFAULT 0,
    source          TEXT NOT NULL DEFAULT 'llm',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);

CREATE INDEX idx_case_instances_case ON case_instances(case_id, seq);

-- 当前承办机关类型('法院'/'仲裁委'/'其他'),驱动前端 label(承办法院 vs 仲裁委)
ALTER TABLE cases ADD COLUMN agg_court_type TEXT;
