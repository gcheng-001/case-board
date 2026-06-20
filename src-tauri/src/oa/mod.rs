//! OA 系统对接(V0.4 · 2026-06-16)
//!
//! 把律所 OA 系统的能力搬到案件看板:
//!   1. OA 立案 — 把案件数据推送到 OA 表单
//!   2. 案件导入 — 从 OA 拉案件数据到本地
//!   3. 客户导入 — 从 OA 拉客户数据到本地
//!
//! 架构:
//!   - Rust 端负责配置/凭证管理、会话调度、进度追踪
//!   - Python sidecar 脚本做实际的浏览器自动化(Playwright)
//!   - 前端通过 Tauri Commands 操作

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::process::Command;

use crate::db::{
    cases,
    oa::{self, NewOAConfig, NewOACredential, OACredential, OASession, UpdateOAConfig},
};

// ─────────────────── 密码存取(Keychain) ───────────────────

const KEYCHAIN_SERVICE: &str = "CaseBoard-OA";

fn keyring_entry(account: &str) -> keyring::Entry {
    keyring::Entry::new(KEYCHAIN_SERVICE, account).unwrap_or_else(|_| {
        keyring::Entry::new_with_target(KEYCHAIN_SERVICE, "", account)
            .expect("keyring entry 创建失败")
    })
}

fn save_password(account: &str, password: &str) -> Result<(), String> {
    let entry = keyring_entry(account);
    entry
        .set_password(password)
        .map_err(|e| format!("保存密码失败: {e}"))
}

fn get_password(account: &str) -> Result<String, String> {
    let entry = keyring_entry(account);
    entry.get_password().or_else(|primary| {
        get_password_from_macos_keychain(account).map_err(|fallback| {
            format!("读取密码失败: {primary}; macOS 钥匙串兼容读取也失败: {fallback}")
        })
    })
}

#[cfg(target_os = "macos")]
fn get_password_from_macos_keychain(account: &str) -> Result<String, String> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
            "-w",
        ])
        .output()
        .map_err(|e| format!("无法调用 macOS 钥匙串: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("security 退出码 {:?}", output.status.code())
        } else {
            stderr
        });
    }

    let password = String::from_utf8(output.stdout)
        .map_err(|e| format!("钥匙串内容不是 UTF-8: {e}"))?
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if password.is_empty() {
        Err("钥匙串条目为空".to_string())
    } else {
        Ok(password)
    }
}

#[cfg(not(target_os = "macos"))]
fn get_password_from_macos_keychain(_account: &str) -> Result<String, String> {
    Err("当前系统不支持 macOS 钥匙串兼容读取".to_string())
}

fn delete_password(account: &str) -> Result<(), String> {
    let entry = keyring_entry(account);
    entry
        .delete_credential()
        .map_err(|e| format!("删除密码失败: {e}"))
}

// ─────────────────── Sidecar 进程管理 ───────────────────

const SIDECAR_DIR: &str = "sidecars/oa";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SidecarProgress {
    event: String,
    pct: Option<i64>,
    message: Option<String>,
    data: Option<serde_json::Value>,
}

async fn fail_session(app: &AppHandle, pool: &SqlitePool, session_id: &str, error: String) {
    let _ = oa::update_session_status(pool, session_id, "failed", Some(&error)).await;
    let _ = app.emit(
        "oa-session-error",
        serde_json::json!({"session_id": session_id, "error": error}),
    );
}

fn sidecar_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("获取资源目录失败: {e}"))?;
    let path = resource_dir.join(SIDECAR_DIR).join("cli.py");
    if !path.exists() {
        return Err(format!("OA 脚本不存在: {}", path.display()));
    }
    Ok(path)
}

fn python_path(app: &AppHandle) -> String {
    let resource_dir = app.path().resource_dir().unwrap_or_default();
    let venv_python = resource_dir
        .join(SIDECAR_DIR)
        .join(".venv")
        .join("bin")
        .join("python3");
    if venv_python.exists() {
        return venv_python.to_string_lossy().to_string();
    }
    "python3".to_string()
}

async fn run_sidecar(
    app: &AppHandle,
    session_id: &str,
    action: &str,
    args: Vec<String>,
    pool: &SqlitePool,
) -> Result<serde_json::Value, String> {
    let script = sidecar_path(app)?;
    let python = python_path(app);

    let mut cmd_args: Vec<String> = vec![
        script.to_string_lossy().to_string(),
        "--action".to_string(),
        action.to_string(),
        "--session-id".to_string(),
        session_id.to_string(),
    ];
    cmd_args.extend(args);

    let mut child = Command::new(&python)
        .args(&cmd_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 OA 脚本失败: {e}"))?;

    let _ = oa::update_session_status(pool, session_id, "running", None).await;

    let stdout = child.stdout.take().expect("stdout 未捕获");
    let mut stderr = child.stderr.take().expect("stderr 未捕获");
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();
    let mut final_result: Option<serde_json::Value> = None;

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(prog) = serde_json::from_str::<SidecarProgress>(trimmed) {
            let pct = prog.pct.unwrap_or(0);
            let msg = prog.message.as_deref().unwrap_or("");
            let _ = oa::update_session_progress(pool, session_id, pct, msg).await;
            let _ = app.emit(
                "oa-session-progress",
                serde_json::json!({
                    "session_id": session_id,
                    "pct": pct,
                    "msg": msg,
                }),
            );
            if prog.event == "completed" || prog.event == "done" {
                final_result = prog.data;
            }
            if prog.event == "failed" {
                let err = msg.to_string();
                let _ = oa::update_session_status(pool, session_id, "failed", Some(&err)).await;
                return Err(err);
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("等待 OA 脚本结束失败: {e}"))?;

    let stderr_text = stderr_task.await.unwrap_or_default();

    if !status.success() {
        let stderr_tail = stderr_text
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let detail = if stderr_tail.trim().is_empty() {
            format!("OA 脚本退出码: {:?}", status.code())
        } else {
            format!("OA 脚本退出码: {:?}\n{}", status.code(), stderr_tail)
        };
        let _ = oa::update_session_status(pool, session_id, "failed", Some(&detail)).await;
        return Err(detail);
    }

    if let Some(ref result) = final_result {
        if let Ok(s) = serde_json::to_string(result) {
            let _ = oa::update_session_result(pool, session_id, &s).await;
        }
    }

    let result = final_result.unwrap_or(serde_json::Value::Null);
    let _ = oa::update_session_status(pool, session_id, "completed", None).await;
    Ok(result)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OAApprovalOptions {
    pub risk_fee_amount: Option<f64>,
    pub conflict_reviewed: Option<bool>,
    pub conflict_memo: Option<String>,
    pub risk_contract_confirmed: Option<bool>,
    pub risk_notice_confirmed: Option<bool>,
    pub fee_reviewed: Option<bool>,
    pub fee_memo: Option<String>,
    pub min_fee: Option<f64>,
    pub low_ratio: Option<f64>,
    pub high_ratio: Option<f64>,
    pub risk_base_fee_min: Option<f64>,
}

impl OAApprovalOptions {
    fn json_arg(self) -> Result<String, String> {
        serde_json::to_string(&self).map_err(|e| format!("审批参数序列化失败: {e}"))
    }
}

async fn sidecar_args_for_credential(
    pool: &SqlitePool,
    config_id: &str,
    credential_id: &str,
) -> Result<Vec<String>, String> {
    let creds = oa::list_credentials(pool, config_id)
        .await
        .map_err(|e| e.to_string())?;
    let cred = creds
        .iter()
        .find(|c| c.id == credential_id)
        .ok_or_else(|| "凭证不存在".to_string())?;
    let password = get_password(&cred.account)?;
    let config = oa::get_config(pool, config_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "OA 配置不存在".to_string())?;
    Ok(vec![
        "--site-url".to_string(),
        config.login_url,
        "--account".to_string(),
        cred.account.clone(),
        "--password".to_string(),
        password,
        "--oa-type".to_string(),
        config.oa_type,
    ])
}

async fn run_oa_action_once(
    app: &AppHandle,
    pool: &SqlitePool,
    config_id: &str,
    credential_id: &str,
    action: &str,
    mut extra_args: Vec<String>,
) -> Result<serde_json::Value, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut args = sidecar_args_for_credential(pool, config_id, credential_id).await?;
    args.append(&mut extra_args);
    run_sidecar(app, &session_id, action, args, pool).await
}

// ─────────────────── Tauri Commands ───────────────────

#[tauri::command]
pub async fn oa_list_configs(
    pool: tauri::State<'_, SqlitePool>,
) -> Result<Vec<oa::OAConfig>, String> {
    oa::list_configs(&pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_create_config(
    pool: tauri::State<'_, SqlitePool>,
    config: NewOAConfig,
) -> Result<oa::OAConfig, String> {
    oa::create_config(&pool, config)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_update_config(
    pool: tauri::State<'_, SqlitePool>,
    id: String,
    patch: UpdateOAConfig,
) -> Result<oa::OAConfig, String> {
    oa::update_config(&pool, &id, patch)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_delete_config(
    pool: tauri::State<'_, SqlitePool>,
    id: String,
) -> Result<(), String> {
    oa::delete_config(&pool, &id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_list_credentials(
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
) -> Result<Vec<OACredential>, String> {
    oa::list_credentials(&pool, &config_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_create_credential(
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    account: String,
    password: String,
    display_name: Option<String>,
) -> Result<OACredential, String> {
    save_password(&account, &password)?;
    let cred = NewOACredential {
        oa_config_id: config_id,
        account,
        display_name,
    };
    oa::create_credential(&pool, cred)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_delete_credential(
    pool: tauri::State<'_, SqlitePool>,
    id: String,
) -> Result<(), String> {
    let creds = oa::list_credentials(&pool, "")
        .await
        .map_err(|e| e.to_string())?;
    if let Some(cred) = creds.iter().find(|c| c.id == id) {
        let _ = delete_password(&cred.account);
    }
    oa::delete_credential(&pool, &id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_list_sessions(
    pool: tauri::State<'_, SqlitePool>,
    config_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<OASession>, String> {
    oa::list_sessions(&pool, config_id.as_deref(), limit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn oa_get_session(
    pool: tauri::State<'_, SqlitePool>,
    id: String,
) -> Result<Option<OASession>, String> {
    oa::get_session(&pool, &id).await.map_err(|e| e.to_string())
}

fn spawn_oa_task(
    app: AppHandle,
    pool: SqlitePool,
    session_id: String,
    action: String,
    config_id: String,
    credential_id: String,
    case_id: Option<String>,
    filing_options: Option<serde_json::Value>,
) {
    tokio::spawn(async move {
        let creds = match oa::list_credentials(&pool, &config_id).await {
            Ok(c) => c,
            Err(e) => {
                fail_session(&app, &pool, &session_id, e.to_string()).await;
                return;
            }
        };
        let cred = match creds.iter().find(|c| c.id == credential_id) {
            Some(c) => c,
            None => {
                fail_session(&app, &pool, &session_id, "凭证不存在".to_string()).await;
                return;
            }
        };
        let password = match get_password(&cred.account) {
            Ok(p) => p,
            Err(e) => {
                fail_session(&app, &pool, &session_id, e).await;
                return;
            }
        };
        let config = match oa::get_config(&pool, &config_id).await {
            Ok(Some(c)) => c,
            _ => {
                fail_session(&app, &pool, &session_id, "OA 配置不存在".to_string()).await;
                return;
            }
        };

        let mut args = vec![
            "--site-url".to_string(),
            config.login_url.clone(),
            "--account".to_string(),
            cred.account.clone(),
            "--password".to_string(),
            password,
            "--oa-type".to_string(),
            config.oa_type.clone(),
        ];
        if let Some(cid) = case_id.clone() {
            args.push("--case-id".to_string());
            args.push(cid.clone());

            if action == "filing" {
                match cases::get_case(&pool, &cid).await {
                    Ok(Some(case_data)) => match serde_json::to_value(&case_data) {
                        Ok(mut value) => {
                            if let Some(options) = filing_options.clone() {
                                if let (Some(case_obj), Some(options_obj)) =
                                    (value.as_object_mut(), options.as_object())
                                {
                                    for (key, option_value) in options_obj {
                                        case_obj.insert(key.clone(), option_value.clone());
                                    }
                                }
                            }
                            match serde_json::to_string(&value) {
                                Ok(json) => {
                                    args.push("--case-data".to_string());
                                    args.push(json);
                                }
                                Err(e) => {
                                    fail_session(
                                        &app,
                                        &pool,
                                        &session_id,
                                        format!("案件数据序列化失败: {e}"),
                                    )
                                    .await;
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            fail_session(
                                &app,
                                &pool,
                                &session_id,
                                format!("案件数据序列化失败: {e}"),
                            )
                            .await;
                            return;
                        }
                    },
                    Ok(None) => {
                        fail_session(&app, &pool, &session_id, "本机案件不存在".to_string()).await;
                        return;
                    }
                    Err(e) => {
                        fail_session(&app, &pool, &session_id, format!("读取案件失败: {e}")).await;
                        return;
                    }
                }
            }
        }

        let result = run_sidecar(&app, &session_id, &action, args, &pool).await;
        match result {
            Ok(mut value) => {
                if action == "case_import" {
                    let cases = value
                        .get("cases")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    match oa::import_cases_from_agent_api(&pool, &config.login_url, &cases).await {
                        Ok(report) => {
                            let report_value = serde_json::to_value(&report).unwrap_or_default();
                            if let Some(obj) = value.as_object_mut() {
                                obj.insert("import_report".to_string(), report_value);
                            }
                            if let Ok(s) = serde_json::to_string(&value) {
                                let _ = oa::update_session_result(&pool, &session_id, &s).await;
                            }
                            let msg = format!(
                                "已导入 OA 案件: 新增 {} 条, 更新 {} 条, 跳过 {} 条",
                                report.inserted, report.updated, report.skipped
                            );
                            let _ =
                                oa::update_session_progress(&pool, &session_id, 100, &msg).await;
                            let _ = app.emit(
                                "oa-session-progress",
                                serde_json::json!({"session_id": session_id, "pct": 100, "msg": msg}),
                            );
                        }
                        Err(e) => {
                            let err = format!("OA 案件已拉取,但写入本地失败: {e}");
                            let _ =
                                oa::update_session_status(&pool, &session_id, "failed", Some(&err))
                                    .await;
                            let _ = app.emit(
                                "oa-session-error",
                                serde_json::json!({"session_id": session_id, "error": err}),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                fail_session(&app, &pool, &session_id, e).await;
            }
        }
    });
}

#[tauri::command]
pub async fn oa_execute_filing(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    case_id: String,
    credential_id: String,
    filing_options: Option<serde_json::Value>,
) -> Result<OASession, String> {
    let session = oa::create_session(&pool, &config_id, "filing", Some(&case_id))
        .await
        .map_err(|e| e.to_string())?;
    let sid = session.id.clone();
    let pool_clone = pool.inner().clone();
    spawn_oa_task(
        app,
        pool_clone,
        sid,
        "filing".into(),
        config_id,
        credential_id,
        Some(case_id),
        filing_options,
    );
    Ok(session)
}

#[tauri::command]
pub async fn oa_start_case_import(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
) -> Result<OASession, String> {
    let session = oa::create_session(&pool, &config_id, "case_import", None)
        .await
        .map_err(|e| e.to_string())?;
    let sid = session.id.clone();
    let pool_clone = pool.inner().clone();
    spawn_oa_task(
        app,
        pool_clone,
        sid,
        "case_import".into(),
        config_id,
        credential_id,
        None,
        None,
    );
    Ok(session)
}

#[tauri::command]
pub async fn oa_start_client_import(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
) -> Result<OASession, String> {
    let session = oa::create_session(&pool, &config_id, "client_import", None)
        .await
        .map_err(|e| e.to_string())?;
    let sid = session.id.clone();
    let pool_clone = pool.inner().clone();
    spawn_oa_task(
        app,
        pool_clone,
        sid,
        "client_import".into(),
        config_id,
        credential_id,
        None,
        None,
    );
    Ok(session)
}

#[tauri::command]
pub async fn oa_pending_approvals(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
) -> Result<serde_json::Value, String> {
    run_oa_action_once(
        &app,
        pool.inner(),
        &config_id,
        &credential_id,
        "pending_approvals",
        vec![],
    )
    .await
}

#[tauri::command]
pub async fn oa_approval_check(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
    lawcase_id: i64,
    options: Option<OAApprovalOptions>,
) -> Result<serde_json::Value, String> {
    let options_json = options
        .unwrap_or(OAApprovalOptions {
            risk_fee_amount: None,
            conflict_reviewed: None,
            conflict_memo: None,
            risk_contract_confirmed: None,
            risk_notice_confirmed: None,
            fee_reviewed: None,
            fee_memo: None,
            min_fee: None,
            low_ratio: None,
            high_ratio: None,
            risk_base_fee_min: None,
        })
        .json_arg()?;
    run_oa_action_once(
        &app,
        pool.inner(),
        &config_id,
        &credential_id,
        "approval_check",
        vec![
            "--lawcase-id".to_string(),
            lawcase_id.to_string(),
            "--approval-options".to_string(),
            options_json,
        ],
    )
    .await
}

#[tauri::command]
pub async fn oa_approval_monitor_snapshot(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
    options: Option<OAApprovalOptions>,
) -> Result<serde_json::Value, String> {
    let options_json = options
        .unwrap_or(OAApprovalOptions {
            risk_fee_amount: None,
            conflict_reviewed: None,
            conflict_memo: None,
            risk_contract_confirmed: None,
            risk_notice_confirmed: None,
            fee_reviewed: None,
            fee_memo: None,
            min_fee: None,
            low_ratio: None,
            high_ratio: None,
            risk_base_fee_min: None,
        })
        .json_arg()?;
    let state_path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?
        .join("oa-approval-monitor.json");
    run_oa_action_once(
        &app,
        pool.inner(),
        &config_id,
        &credential_id,
        "approval_monitor_snapshot",
        vec![
            "--approval-options".to_string(),
            options_json,
            "--monitor-state-path".to_string(),
            state_path.to_string_lossy().to_string(),
        ],
    )
    .await
}

async fn run_approval_session(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
    lawcase_id: i64,
    memo: String,
    options: Option<OAApprovalOptions>,
    approved: bool,
) -> Result<OASession, String> {
    let session_type = if approved {
        "approval_approve"
    } else {
        "approval_reject"
    };
    let session = oa::create_session(&pool, &config_id, session_type, None)
        .await
        .map_err(|e| e.to_string())?;
    let sid = session.id.clone();
    let pool_clone = pool.inner().clone();
    let options_json = options
        .unwrap_or(OAApprovalOptions {
            risk_fee_amount: None,
            conflict_reviewed: None,
            conflict_memo: None,
            risk_contract_confirmed: None,
            risk_notice_confirmed: None,
            fee_reviewed: None,
            fee_memo: None,
            min_fee: None,
            low_ratio: None,
            high_ratio: None,
            risk_base_fee_min: None,
        })
        .json_arg()?;

    tokio::spawn(async move {
        let mut args =
            match sidecar_args_for_credential(&pool_clone, &config_id, &credential_id).await {
                Ok(args) => args,
                Err(e) => {
                    fail_session(&app, &pool_clone, &sid, e).await;
                    return;
                }
            };
        args.extend([
            "--lawcase-id".to_string(),
            lawcase_id.to_string(),
            "--memo".to_string(),
            memo,
            "--confirm".to_string(),
            "--approval-options".to_string(),
            options_json,
        ]);
        let action = if approved {
            "approval_approve"
        } else {
            "approval_reject"
        };
        match run_sidecar(&app, &sid, action, args, &pool_clone).await {
            Ok(_) => {}
            Err(e) => fail_session(&app, &pool_clone, &sid, e).await,
        }
    });

    Ok(session)
}

#[tauri::command]
pub async fn oa_approve_case(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
    lawcase_id: i64,
    memo: String,
    options: Option<OAApprovalOptions>,
) -> Result<OASession, String> {
    run_approval_session(
        app,
        pool,
        config_id,
        credential_id,
        lawcase_id,
        memo,
        options,
        true,
    )
    .await
}

#[tauri::command]
pub async fn oa_reject_case(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    config_id: String,
    credential_id: String,
    lawcase_id: i64,
    memo: String,
    options: Option<OAApprovalOptions>,
) -> Result<OASession, String> {
    run_approval_session(
        app,
        pool,
        config_id,
        credential_id,
        lawcase_id,
        memo,
        options,
        false,
    )
    .await
}
