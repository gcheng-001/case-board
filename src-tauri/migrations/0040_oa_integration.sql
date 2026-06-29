-- ============================================================================
-- 案件看板 V0.4 · OA 系统对接
--
-- 三张表:
--   oa_configs     — OA 站点配置(登录地址、字段映射)
--   oa_credentials — 登录凭证(账号;密码存 macOS Keychain,这里只存 account)
--   oa_sessions    — 执行会话(立案/导入的进度和结果)
-- ============================================================================

-- OA 站点配置
CREATE TABLE IF NOT EXISTS oa_configs (
    id              TEXT PRIMARY KEY NOT NULL,
    site_name       TEXT NOT NULL UNIQUE,     -- 站点名称(如 "摩尚OA")
    login_url       TEXT NOT NULL,            -- 登录页 URL
    oa_type         TEXT NOT NULL DEFAULT 'nedev',  -- OA 平台类型(nedev / jtn / generic)
    is_enabled      INTEGER NOT NULL DEFAULT 1,
    field_mapping   TEXT NOT NULL DEFAULT '{}',  -- JSON:系统字段→OA表单字段映射
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 登录凭证(密码存 Keychain,SQLite 只存 account 用于索引)
CREATE TABLE IF NOT EXISTS oa_credentials (
    id              TEXT PRIMARY KEY NOT NULL,
    oa_config_id    TEXT NOT NULL,
    account         TEXT NOT NULL,            -- 登录账号(不存密码)
    display_name    TEXT,                     -- 显示名(如 "张三律师")
    cookies_path    TEXT,                     -- cookies JSON 文件路径
    last_login_at   TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (oa_config_id) REFERENCES oa_configs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_oa_credentials_config ON oa_credentials(oa_config_id);

-- OA 操作会话(立案/案件导入/客户导入)
CREATE TABLE IF NOT EXISTS oa_sessions (
    id              TEXT PRIMARY KEY NOT NULL,
    oa_config_id    TEXT NOT NULL,
    session_type    TEXT NOT NULL,            -- filing / case_import / client_import
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending / running / completed / failed
    case_id         TEXT,                     -- 关联的本机案件 ID(立案用)
    progress_pct    INTEGER NOT NULL DEFAULT 0,  -- 进度百分比 0-100
    progress_msg    TEXT NOT NULL DEFAULT '',  -- 进度描述
    result_json     TEXT,                     -- JSON:导入结果/立案结果
    error_message   TEXT NOT NULL DEFAULT '',
    started_at      TEXT,
    completed_at    TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (oa_config_id) REFERENCES oa_configs(id) ON DELETE CASCADE,
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_oa_sessions_config ON oa_sessions(oa_config_id);
CREATE INDEX IF NOT EXISTS idx_oa_sessions_status ON oa_sessions(status);
