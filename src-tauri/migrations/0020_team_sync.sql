-- 团队版 Phase 1(LAN 接力同步)· 2026-06-10
-- 设计:docs/提案-团队版-2026-06-10.md §6
--
-- team_snapshots:全队进度快照缓存(含自己的;member_id 主键,新者胜覆盖)。
--   payload 是登记表粒度的 JSON(案由/案号/当事人/阶段/重要日期/动态),
--   绝不含文档原文/报告/聊天。权限过滤在显示层(老板拍板:团队内信任模型)。
-- team_state:k/v(roster 清单 JSON、自己的快照 seq 计数等)。
-- 团队身份(team_id/secret/member_id/配对码)在 settings.json,不进库。

CREATE TABLE IF NOT EXISTS team_snapshots (
    member_id  TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    payload    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS team_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
