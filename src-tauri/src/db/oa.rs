//! OA 系统对接 — 数据库层(2026-06-16)
//!
//! 三张表:oa_configs / oa_credentials / oa_sessions
//! 密码存 macOS Keychain(由 Rust `oa` 模块处理),SQLite 只存 account 用于索引。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

// ─────────────────── OA 站点配置 ───────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OAConfig {
    pub id: String,
    pub site_name: String,
    pub login_url: String,
    pub oa_type: String,
    pub is_enabled: i64,
    pub field_mapping: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewOAConfig {
    pub site_name: String,
    pub login_url: String,
    #[serde(default = "default_oa_type")]
    pub oa_type: String,
    #[serde(default)]
    pub field_mapping: Option<String>,
}

fn default_oa_type() -> String {
    "nedev".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateOAConfig {
    pub site_name: Option<String>,
    pub login_url: Option<String>,
    pub oa_type: Option<String>,
    pub is_enabled: Option<i64>,
    pub field_mapping: Option<String>,
}

// ─────────────────── 登录凭证 ───────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OACredential {
    pub id: String,
    pub oa_config_id: String,
    pub account: String,
    pub display_name: Option<String>,
    pub cookies_path: Option<String>,
    pub last_login_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewOACredential {
    pub oa_config_id: String,
    pub account: String,
    pub display_name: Option<String>,
}

// ─────────────────── OA 会话 ───────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OASession {
    pub id: String,
    pub oa_config_id: String,
    pub session_type: String,
    pub status: String,
    pub case_id: Option<String>,
    pub progress_pct: i64,
    pub progress_msg: String,
    pub result_json: Option<String>,
    pub error_message: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// ─────────────────── OA Config CRUD ───────────────────

pub async fn list_configs(pool: &SqlitePool) -> Result<Vec<OAConfig>, sqlx::Error> {
    sqlx::query_as::<_, OAConfig>("SELECT * FROM oa_configs ORDER BY created_at DESC")
        .fetch_all(pool)
        .await
}

pub async fn get_config(pool: &SqlitePool, id: &str) -> Result<Option<OAConfig>, sqlx::Error> {
    sqlx::query_as::<_, OAConfig>("SELECT * FROM oa_configs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn create_config(pool: &SqlitePool, cfg: NewOAConfig) -> Result<OAConfig, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let mapping = cfg.field_mapping.as_deref().unwrap_or("{}");
    sqlx::query(
        "INSERT INTO oa_configs (id, site_name, login_url, oa_type, field_mapping) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&cfg.site_name)
    .bind(&cfg.login_url)
    .bind(&cfg.oa_type)
    .bind(mapping)
    .execute(pool)
    .await?;

    get_config(pool, &id).await?.ok_or(sqlx::Error::RowNotFound)
}

pub async fn update_config(
    pool: &SqlitePool,
    id: &str,
    patch: UpdateOAConfig,
) -> Result<OAConfig, sqlx::Error> {
    // 动态拼 UPDATE 只改传了值的字段
    if let Some(ref name) = patch.site_name {
        sqlx::query(
            "UPDATE oa_configs SET site_name = ?1, updated_at = datetime('now') WHERE id = ?2",
        )
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    }
    if let Some(ref url) = patch.login_url {
        sqlx::query(
            "UPDATE oa_configs SET login_url = ?1, updated_at = datetime('now') WHERE id = ?2",
        )
        .bind(url)
        .bind(id)
        .execute(pool)
        .await?;
    }
    if let Some(ref t) = patch.oa_type {
        sqlx::query(
            "UPDATE oa_configs SET oa_type = ?1, updated_at = datetime('now') WHERE id = ?2",
        )
        .bind(t)
        .bind(id)
        .execute(pool)
        .await?;
    }
    if let Some(enabled) = patch.is_enabled {
        sqlx::query(
            "UPDATE oa_configs SET is_enabled = ?1, updated_at = datetime('now') WHERE id = ?2",
        )
        .bind(enabled)
        .bind(id)
        .execute(pool)
        .await?;
    }
    if let Some(ref mapping) = patch.field_mapping {
        sqlx::query(
            "UPDATE oa_configs SET field_mapping = ?1, updated_at = datetime('now') WHERE id = ?2",
        )
        .bind(mapping)
        .bind(id)
        .execute(pool)
        .await?;
    }

    get_config(pool, &id).await?.ok_or(sqlx::Error::RowNotFound)
}

pub async fn delete_config(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM oa_configs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ─────────────────── Credential CRUD ───────────────────

pub async fn list_credentials(
    pool: &SqlitePool,
    config_id: &str,
) -> Result<Vec<OACredential>, sqlx::Error> {
    sqlx::query_as::<_, OACredential>(
        "SELECT * FROM oa_credentials WHERE oa_config_id = ? ORDER BY created_at DESC",
    )
    .bind(config_id)
    .fetch_all(pool)
    .await
}

pub async fn create_credential(
    pool: &SqlitePool,
    cred: NewOACredential,
) -> Result<OACredential, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO oa_credentials (id, oa_config_id, account, display_name) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&cred.oa_config_id)
    .bind(&cred.account)
    .bind(&cred.display_name)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, OACredential>("SELECT * FROM oa_credentials WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await
}

pub async fn delete_credential(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM oa_credentials WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_credential_login(
    pool: &SqlitePool,
    id: &str,
    cookies_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE oa_credentials SET cookies_path = ?1, last_login_at = datetime('now'), updated_at = datetime('now') WHERE id = ?2",
    )
    .bind(cookies_path)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

// ─────────────────── Session CRUD ───────────────────

pub async fn create_session(
    pool: &SqlitePool,
    config_id: &str,
    session_type: &str,
    case_id: Option<&str>,
) -> Result<OASession, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO oa_sessions (id, oa_config_id, session_type, case_id, status) VALUES (?, ?, ?, ?, 'pending')",
    )
    .bind(&id)
    .bind(config_id)
    .bind(session_type)
    .bind(case_id)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, OASession>("SELECT * FROM oa_sessions WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await
}

pub async fn get_session(pool: &SqlitePool, id: &str) -> Result<Option<OASession>, sqlx::Error> {
    sqlx::query_as::<_, OASession>("SELECT * FROM oa_sessions WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn list_sessions(
    pool: &SqlitePool,
    config_id: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<OASession>, sqlx::Error> {
    let lim = limit.unwrap_or(50);
    match config_id {
        Some(cid) => {
            sqlx::query_as::<_, OASession>(
                "SELECT * FROM oa_sessions WHERE oa_config_id = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(cid)
            .bind(lim)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as::<_, OASession>(
                "SELECT * FROM oa_sessions ORDER BY created_at DESC LIMIT ?",
            )
            .bind(lim)
            .fetch_all(pool)
            .await
        }
    }
}

pub async fn update_session_progress(
    pool: &SqlitePool,
    id: &str,
    pct: i64,
    msg: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE oa_sessions SET progress_pct = ?1, progress_msg = ?2, updated_at = datetime('now') WHERE id = ?3",
    )
    .bind(pct)
    .bind(msg)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_session_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    match status {
        "running" => {
            sqlx::query(
                "UPDATE oa_sessions SET status = 'running', started_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
        "completed" => {
            sqlx::query(
                "UPDATE oa_sessions SET status = 'completed', progress_pct = 100, completed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
        "failed" => {
            sqlx::query(
                "UPDATE oa_sessions SET status = 'failed', error_message = ?1, completed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?2",
            )
            .bind(error.unwrap_or(""))
            .bind(id)
            .execute(pool)
            .await?;
        }
        "conflict" => {
            sqlx::query(
                "UPDATE oa_sessions SET status = 'conflict', error_message = ?1, updated_at = datetime('now') WHERE id = ?2",
            )
            .bind(error.unwrap_or(""))
            .bind(id)
            .execute(pool)
            .await?;
        }
        _ => {}
    }
    Ok(())
}

pub async fn update_session_result(
    pool: &SqlitePool,
    id: &str,
    result_json: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE oa_sessions SET result_json = ?1, updated_at = datetime('now') WHERE id = ?2",
    )
    .bind(result_json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

// ─────────────────── OA → 本地案件导入 ───────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAImportReport {
    pub total: usize,
    pub inserted: usize,
    pub updated: usize,
    pub skipped: usize,
}

pub async fn import_cases_from_agent_api(
    pool: &SqlitePool,
    site_url: &str,
    cases: &[Value],
) -> Result<OAImportReport, sqlx::Error> {
    let host = oa_host(site_url);
    let mut report = OAImportReport {
        total: cases.len(),
        inserted: 0,
        updated: 0,
        skipped: 0,
    };

    for item in cases {
        let Some(oa_id) = item.get("id").and_then(Value::as_i64) else {
            report.skipped += 1;
            continue;
        };
        let source_folder = format!("oa://{host}/lawcase/{oa_id}");
        let name = case_name(item).unwrap_or_else(|| format!("OA 案件 {oa_id}"));
        let case_type = str_field(item, "baseTypeName").unwrap_or_else(|| "诉讼".to_string());
        let cause = str_field(item, "causeAction");
        let case_no = str_field(item, "no").or_else(|| str_field(item, "preNo"));
        let filed_at = str_field(item, "shouliDate");
        let plaintiffs = names_json(str_field(item, "wtrNames"));
        let defendants = names_json(str_field(item, "tosNames"));
        let fees = fees_json(item);
        let charge_amount = number_field(item, "chargeAmount");
        let status_name = str_field(item, "statusName");
        let summary = summary_text(item);

        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM cases WHERE source_folder = ?")
                .bind(&source_folder)
                .fetch_optional(pool)
                .await?;

        if let Some(case_id) = exists {
            sqlx::query(
                r#"
                UPDATE cases SET
                    name = ?,
                    case_type = ?,
                    cause = COALESCE(?, cause),
                    case_no = COALESCE(?, case_no),
                    agg_case_no = COALESCE(?, agg_case_no),
                    agg_cause = COALESCE(?, agg_cause),
                    agg_filed_at = COALESCE(?, agg_filed_at),
                    agg_plaintiffs = COALESCE(?, agg_plaintiffs),
                    agg_defendants = COALESCE(?, agg_defendants),
                    agg_fees = COALESCE(?, agg_fees),
                    agg_claim_amount = COALESCE(?, agg_claim_amount),
                    agg_status_text = COALESCE(?, agg_status_text),
                    case_summary = COALESCE(?, case_summary),
                    last_scanned_at = datetime('now'),
                    updated_at = datetime('now')
                WHERE id = ?
                "#,
            )
            .bind(&name)
            .bind(&case_type)
            .bind(&cause)
            .bind(&case_no)
            .bind(&case_no)
            .bind(&cause)
            .bind(&filed_at)
            .bind(&plaintiffs)
            .bind(&defendants)
            .bind(&fees)
            .bind(charge_amount)
            .bind(&status_name)
            .bind(&summary)
            .bind(&case_id)
            .execute(pool)
            .await?;
            report.updated += 1;
        } else {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                r#"
                INSERT INTO cases (
                    id, name, case_type, cause, case_no, source_folder,
                    agg_case_no, agg_cause, agg_filed_at, agg_plaintiffs,
                    agg_defendants, agg_fees, agg_claim_amount, agg_status_text,
                    case_summary, last_scanned_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
                "#,
            )
            .bind(&id)
            .bind(&name)
            .bind(&case_type)
            .bind(&cause)
            .bind(&case_no)
            .bind(&source_folder)
            .bind(&case_no)
            .bind(&cause)
            .bind(&filed_at)
            .bind(&plaintiffs)
            .bind(&defendants)
            .bind(&fees)
            .bind(charge_amount)
            .bind(&status_name)
            .bind(&summary)
            .execute(pool)
            .await?;
            report.inserted += 1;
        }
    }

    Ok(report)
}

fn oa_host(site_url: &str) -> String {
    site_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

fn str_field(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number_field(item: &Value, key: &str) -> Option<f64> {
    item.get(key).and_then(Value::as_f64)
}

fn case_name(item: &Value) -> Option<String> {
    let cause = str_field(item, "causeAction");
    let wtr = str_field(item, "wtrNames");
    let tos = str_field(item, "tosNames");
    match (wtr, tos, cause) {
        (Some(a), Some(b), Some(c)) => Some(format!("{a} 与 {b} {c}")),
        (Some(a), None, Some(c)) => Some(format!("{a} {c}")),
        (Some(a), Some(b), None) => Some(format!("{a} 与 {b}")),
        (None, Some(b), Some(c)) => Some(format!("{b} {c}")),
        (Some(a), None, None) => Some(a),
        (None, None, Some(c)) => Some(c),
        _ => str_field(item, "no").or_else(|| str_field(item, "preNo")),
    }
}

fn names_json(value: Option<String>) -> Option<String> {
    let value = value?;
    let parts: Vec<String> = value
        .split(['、', ',', '，', ';', '；'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        None
    } else {
        serde_json::to_string(&parts).ok()
    }
}

fn fees_json(item: &Value) -> Option<String> {
    let mut fees = Vec::new();
    if let Some(amount) = number_field(item, "chargeAmount") {
        fees.push(serde_json::json!({"item": "委托费用", "amount": amount, "note": null}));
    }
    if let Some(amount) = number_field(item, "otherCharge") {
        if amount > 0.0 {
            fees.push(serde_json::json!({"item": "其他收费", "amount": amount, "note": null}));
        }
    }
    if let Some(amount) = number_field(item, "yishou") {
        fees.push(serde_json::json!({"item": "已收", "amount": amount, "note": null}));
    }
    if let Some(amount) = number_field(item, "weishou") {
        fees.push(serde_json::json!({"item": "未收", "amount": amount, "note": null}));
    }
    if fees.is_empty() {
        None
    } else {
        serde_json::to_string(&fees).ok()
    }
}

fn summary_text(item: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for (label, key) in [
        ("OA案号", "no"),
        ("待审号", "preNo"),
        ("状态", "statusName"),
        ("案件分类", "caseCategoryName"),
        ("经办律师", "empNames"),
    ] {
        if let Some(v) = str_field(item, key) {
            parts.push(format!("{label}: {v}"));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("；"))
    }
}
