//! 微信录屏取证工具集成。
//!
//! Rust 端只做任务编排、进度通知、停止控制与案件材料刷新；截图/PDF/OCR 的证据处理逻辑
//! 复用打包进 App 的 `sidecars/wechat_evidence/wechat_evidence.py`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

use crate::db::{cases as cases_db, documents as documents_db};
use crate::ingest::{pipeline, scanner::scan_folder};
use crate::llm::LlmConfig;

const SIDECAR_DIR: &str = "sidecars/wechat_evidence";
const SCRIPT_NAME: &str = "wechat_evidence.py";
const EVENT_NAME: &str = "wechat-evidence-progress";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WechatEvidenceJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WechatEvidenceStage {
    Export,
    Validate,
    Ocr,
    Ingest,
    Done,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WechatEvidenceStartInput {
    pub video_path: String,
    pub case_id: Option<String>,
    pub target_folder: Option<String>,
    /// `None` 或空字符串 = 自动抽帧；数字字符串 = 每 N 秒留一张。
    pub stride_seconds: Option<String>,
    pub preserve_head_sec: Option<f64>,
    pub run_ocr: Option<bool>,
    pub ocr_scope: Option<String>,
    pub cloud_text_summary: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WechatEvidenceJob {
    pub id: String,
    pub status: WechatEvidenceJobStatus,
    pub stage: WechatEvidenceStage,
    pub pct: u8,
    pub message: String,
    pub case_id: Option<String>,
    pub video_path: String,
    pub output_dir: Option<String>,
    pub pdf_path: Option<String>,
    pub report_path: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Default)]
pub struct WechatEvidenceState {
    jobs: Mutex<HashMap<String, WechatEvidenceJob>>,
    stop_senders: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl WechatEvidenceState {
    async fn upsert_job(&self, job: WechatEvidenceJob) {
        self.jobs.lock().await.insert(job.id.clone(), job);
    }

    async fn update_job<F>(&self, id: &str, f: F) -> Option<WechatEvidenceJob>
    where
        F: FnOnce(&mut WechatEvidenceJob),
    {
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(id)?;
        f(job);
        Some(job.clone())
    }

    async fn get_job(&self, id: &str) -> Option<WechatEvidenceJob> {
        self.jobs.lock().await.get(id).cloned()
    }
}

fn now_iso() -> String {
    Local::now().to_rfc3339()
}

fn sanitize_path_segment(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "录屏".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn script_path(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("获取应用资源目录失败: {e}"))?;
    let bundled = resource_dir.join(SIDECAR_DIR).join(SCRIPT_NAME);
    if bundled.exists() {
        return Ok(bundled);
    }

    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(SIDECAR_DIR)
        .join(SCRIPT_NAME);
    if dev.exists() {
        return Ok(dev);
    }

    Err(format!(
        "微信录屏取证脚本不存在: {} 或 {}",
        bundled.display(),
        dev.display()
    ))
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

fn emit_job(app: &AppHandle, job: &WechatEvidenceJob) {
    let _ = app.emit(EVENT_NAME, job);
}

async fn set_progress(
    app: &AppHandle,
    state: &WechatEvidenceState,
    job_id: &str,
    stage: WechatEvidenceStage,
    pct: u8,
    message: impl Into<String>,
) {
    if let Some(job) = state
        .update_job(job_id, |job| {
            job.status = WechatEvidenceJobStatus::Running;
            job.stage = stage;
            job.pct = pct;
            job.message = message.into();
        })
        .await
    {
        emit_job(app, &job);
    }
}

async fn set_failed(
    app: &AppHandle,
    state: &WechatEvidenceState,
    job_id: &str,
    stage: WechatEvidenceStage,
    error: String,
) {
    if let Some(job) = state
        .update_job(job_id, |job| {
            job.status = WechatEvidenceJobStatus::Failed;
            job.stage = stage;
            job.message = "处理失败".to_string();
            job.error = Some(error.clone());
            job.finished_at = Some(now_iso());
        })
        .await
    {
        emit_job(app, &job);
    }
}

async fn is_stopped(state: &WechatEvidenceState, job_id: &str) -> bool {
    state
        .get_job(job_id)
        .await
        .map(|j| j.status == WechatEvidenceJobStatus::Stopped)
        .unwrap_or(false)
}

fn command_output_to_error(label: &str, status: Option<i32>, stdout: &str, stderr: &str) -> String {
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else if !stdout.trim().is_empty() {
        stdout.trim()
    } else {
        "无详细输出"
    };
    format!("{label}失败(exit={:?}): {detail}", status)
}

async fn run_python_step(
    app: &AppHandle,
    state: &WechatEvidenceState,
    job_id: &str,
    label: &str,
    args: Vec<String>,
) -> Result<String, String> {
    run_python_step_with_env(app, state, job_id, label, args, vec![]).await
}

async fn run_python_step_with_env(
    app: &AppHandle,
    state: &WechatEvidenceState,
    job_id: &str,
    label: &str,
    args: Vec<String>,
    extra_env: Vec<(String, String)>,
) -> Result<String, String> {
    if is_stopped(state, job_id).await {
        return Err("任务已停止".to_string());
    }

    let python = python_path(app);
    let script = script_path(app)?;
    let mut child = Command::new(&python)
        .arg(script)
        .args(args)
        .envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动{label}失败: {e}"))?;

    let mut stdout = child.stdout.take().ok_or("无法捕获子进程 stdout")?;
    let mut stderr = child.stderr.take().ok_or("无法捕获子进程 stderr")?;
    let stdout_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf).await;
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    state
        .stop_senders
        .lock()
        .await
        .insert(job_id.to_string(), stop_tx);

    let mut stopped = false;
    let status = tokio::select! {
        res = child.wait() => {
            res.map_err(|e| format!("{label}等待失败: {e}"))?
        }
        _ = stop_rx => {
            stopped = true;
            let _ = child.start_kill();
            child.wait().await.map_err(|e| format!("{label}停止失败: {e}"))?
        }
    };

    state.stop_senders.lock().await.remove(job_id);

    let stdout_text = stdout_task.await.unwrap_or_default();
    let stderr_text = stderr_task.await.unwrap_or_default();

    if stopped || is_stopped(state, job_id).await {
        return Err("任务已停止".to_string());
    }
    if status.success() {
        Ok(stdout_text)
    } else {
        Err(command_output_to_error(
            label,
            status.code(),
            &stdout_text,
            &stderr_text,
        ))
    }
}

fn parse_export_stdout(stdout: &str) -> (Option<String>, Option<String>) {
    let mut paths = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| line.starts_with('/'));
    let _out_dir = paths.next();
    let pdf = paths.next().map(ToOwned::to_owned);
    let index = paths.next().map(ToOwned::to_owned);
    (pdf, index)
}

fn parse_ocr_stdout(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/') && line.ends_with("聊天记录分析报告.md"))
        .last()
        .map(ToOwned::to_owned)
        .or_else(|| {
            stdout
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with('/') && line.ends_with(".md"))
                .last()
                .map(ToOwned::to_owned)
        })
}

fn write_task_note(
    job: &WechatEvidenceJob,
    input: &WechatEvidenceStartInput,
) -> Result<(), String> {
    let Some(output_dir) = job.output_dir.as_deref() else {
        return Ok(());
    };
    let path = Path::new(output_dir).join("录屏取证任务说明.md");
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                files.push(
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.display().to_string()),
                );
            }
        }
    }
    files.sort();
    let content = format!(
        "# 录屏取证任务说明\n\n\
         > 原视频、截图和 PDF 是证据本体；OCR 文字、说话人身份、日期、金额和法律判断均需人工核实。\n\n\
         - 任务状态: {:?}\n\
         - 原视频: `{}`\n\
         - 目标案件 ID: `{}`\n\
         - 本地输出根目录: `{}`\n\
         - 输出目录: `{}`\n\
         - 开始时间: {}\n\
         - 完成时间: {}\n\
         - 抽帧间隔: {}\n\
         - 开头详情页保留秒数: {}\n\
         - OCR: {}\n\
         - OCR 范围: {}\n\
         - 云端文字增强: {}\n\
         - PDF: {}\n\
         - 分析报告: {}\n\n\
         ## 输出文件\n\n{}\n",
        job.status,
        job.video_path,
        job.case_id.as_deref().unwrap_or("未归档到案件"),
        input.target_folder.as_deref().unwrap_or("未单独指定"),
        output_dir,
        job.started_at,
        job.finished_at.as_deref().unwrap_or("未完成"),
        "固定间隔(默认1秒/帧)+保串联去重",
        input.preserve_head_sec.unwrap_or(0.0),
        if input.run_ocr.unwrap_or(true) { "是" } else { "否" },
        input.ocr_scope.as_deref().unwrap_or("selected"),
        if input.cloud_text_summary.unwrap_or(false) {
            "是，仅发送 OCR 文本和审计元数据"
        } else {
            "否"
        },
        job.pdf_path.as_deref().unwrap_or("未生成"),
        job.report_path.as_deref().unwrap_or("未生成"),
        if files.is_empty() {
            "- (未列出文件)\n".to_string()
        } else {
            files
                .into_iter()
                .map(|f| format!("- `{f}`"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
    );
    std::fs::write(&path, content).map_err(|e| format!("写入任务说明失败: {e}"))
}

async fn run_job(
    app: AppHandle,
    pool: SqlitePool,
    job_id: String,
    input: WechatEvidenceStartInput,
    output_dir: PathBuf,
) {
    let state = app.state::<WechatEvidenceState>();
    let state = state.inner();
    let stage = WechatEvidenceStage::Export;

    set_progress(
        &app,
        state,
        &job_id,
        WechatEvidenceStage::Export,
        8,
        "正在导出截图和证据 PDF",
    )
    .await;

    // 固定间隔抽帧(filter off, 默认1秒/帧) + 保串联去重: 替代 filter auto
    // (filter auto 的 stride 跳帧 + 激进去重会丢中间消息导致断片)
    // 抽帧间隔(秒): 前端选 1秒(不漏,推荐)/2秒(更快); filter off 固定间隔抽帧 + 保串联去重
    let interval = input
        .stride_seconds
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "auto")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(1.0);
    let export_args = vec![
        "interval-pdf".to_string(),
        input.video_path.clone(),
        "--out-dir".to_string(),
        output_dir.to_string_lossy().to_string(),
        "--filter".to_string(),
        "off".to_string(),
        "--interval".to_string(),
        interval.to_string(),
        "--preserve-head-sec".to_string(),
        input.preserve_head_sec.unwrap_or(0.0).to_string(),
        "--image-ext".to_string(),
        "png".to_string(),
    ];

    let export_stdout =
        match run_python_step(&app, state, &job_id, "导出录屏取证 PDF", export_args).await {
            Ok(out) => out,
            Err(e) if e == "任务已停止" => return,
            Err(e) => {
                set_failed(&app, state, &job_id, stage, e).await;
                return;
            }
        };
    let (pdf_path, _index_path) = parse_export_stdout(&export_stdout);
    if let Some(job) = state
        .update_job(&job_id, |job| {
            job.output_dir = Some(output_dir.to_string_lossy().to_string());
            job.pdf_path = pdf_path.clone();
        })
        .await
    {
        emit_job(&app, &job);
    }

    set_progress(
        &app,
        state,
        &job_id,
        WechatEvidenceStage::Validate,
        40,
        "正在校验证据输出",
    )
    .await;
    let validate_args = vec![
        "validate-export".to_string(),
        output_dir.to_string_lossy().to_string(),
    ];
    if let Err(e) = run_python_step(&app, state, &job_id, "校验证据输出", validate_args).await
    {
        if e != "任务已停止" {
            set_failed(&app, state, &job_id, WechatEvidenceStage::Validate, e).await;
        }
        return;
    }

    if input.run_ocr.unwrap_or(true) {
        set_progress(
            &app,
            state,
            &job_id,
            WechatEvidenceStage::Ocr,
            55,
            "正在生成 OCR 索引和分析报告",
        )
        .await;
        let scope = match input.ocr_scope.as_deref() {
            Some("raw") => "raw",
            _ => "selected",
        };
        let mut ocr_args = vec![
            "ocr-index".to_string(),
            output_dir.to_string_lossy().to_string(),
            "--scope".to_string(),
            scope.to_string(),
            "--jobs".to_string(),
            "2".to_string(),
        ];
        if input.cloud_text_summary.unwrap_or(false) {
            match crate::settings::read_settings() {
                Ok(settings) => {
                    if settings.effective_cloud_llm_backend() == "minimax" {
                        set_failed(
                            &app,
                            state,
                            &job_id,
                            WechatEvidenceStage::Ocr,
                            "云端文字增强需要 OpenAI 兼容接口；当前 MiniMax 配置暂不支持该脚本协议"
                                .to_string(),
                        )
                        .await;
                        return;
                    }
                    let llm = LlmConfig::from_settings(&settings);
                    let base_url = llm
                        .endpoint
                        .strip_suffix("/v1/chat/completions")
                        .unwrap_or(&llm.endpoint)
                        .to_string();
                    if let Some(api_key) = llm.api_key {
                        ocr_args.extend([
                            "--cloud".to_string(),
                            "text-summary".to_string(),
                            "--base-url".to_string(),
                            base_url,
                            "--model".to_string(),
                            llm.model,
                            "--api-key".to_string(),
                            api_key,
                        ]);
                    } else {
                        set_failed(
                            &app,
                            state,
                            &job_id,
                            WechatEvidenceStage::Ocr,
                            "已开启云端文字增强,但当前云端模型没有配置 API Key".to_string(),
                        )
                        .await;
                        return;
                    }
                }
                Err(e) => {
                    set_failed(
                        &app,
                        state,
                        &job_id,
                        WechatEvidenceStage::Ocr,
                        format!("读取云端模型配置失败: {e}"),
                    )
                    .await;
                    return;
                }
            }
        }
        // A 方案: 开了「云端文字增强」且 selected 时, 把 paddle token 传给 sidecar 走 VL 判发言人
        let mut ocr_env: Vec<(String, String)> = vec![];
        if input.cloud_text_summary.unwrap_or(false) && scope == "selected" {
            if let Ok(settings) = crate::settings::read_settings() {
                if let Some(key) = settings
                    .paddle_vl_api_key
                    .as_ref()
                    .map(|k| k.trim())
                    .filter(|k| !k.is_empty())
                {
                    ocr_env.push(("PADDLE_VL_API_KEY".to_string(), key.to_string()));
                }
            }
        }
        let ocr_stdout =
            match run_python_step_with_env(&app, state, &job_id, "OCR 索引", ocr_args, ocr_env).await {
            Ok(out) => out,
            Err(e) if e == "任务已停止" => return,
            Err(e) => {
                set_failed(&app, state, &job_id, WechatEvidenceStage::Ocr, e).await;
                return;
            }
        };
        let report_path = parse_ocr_stdout(&ocr_stdout).or_else(|| {
            Some(
                output_dir
                    .join("聊天记录分析报告.md")
                    .to_string_lossy()
                    .to_string(),
            )
        });
        if let Some(job) = state
            .update_job(&job_id, |job| {
                job.report_path = report_path.clone();
            })
            .await
        {
            emit_job(&app, &job);
        }

        let validate_ocr_args = vec![
            "validate-export".to_string(),
            output_dir.to_string_lossy().to_string(),
            "--require-ocr".to_string(),
        ];
        if let Err(e) =
            run_python_step(&app, state, &job_id, "校验 OCR 输出", validate_ocr_args).await
        {
            if e != "任务已停止" {
                set_failed(&app, state, &job_id, WechatEvidenceStage::Ocr, e).await;
            }
            return;
        }
    }

    let archive_to_case = input.case_id.as_deref().filter(|s| !s.trim().is_empty());
    set_progress(
        &app,
        state,
        &job_id,
        WechatEvidenceStage::Ingest,
        88,
        if archive_to_case.is_some() {
            "正在归档到案件材料"
        } else {
            "正在写入任务说明"
        },
    )
    .await;
    if let Some(job) = state.get_job(&job_id).await {
        let _ = write_task_note(&job, &input);
    }
    let Some(case_id) = archive_to_case else {
        if let Some(job) = state
            .update_job(&job_id, |job| {
                job.status = WechatEvidenceJobStatus::Completed;
                job.stage = WechatEvidenceStage::Done;
                job.pct = 100;
                job.message = "录屏取证已完成并保存到本地文件夹".to_string();
                job.finished_at = Some(now_iso());
            })
            .await
        {
            let _ = write_task_note(&job, &input);
            emit_job(&app, &job);
        }
        return;
    };

    let case = match cases_db::get_case(&pool, case_id).await {
        Ok(Some(case)) => case,
        Ok(None) => {
            set_failed(
                &app,
                state,
                &job_id,
                WechatEvidenceStage::Ingest,
                "案件不存在".to_string(),
            )
            .await;
            return;
        }
        Err(e) => {
            set_failed(
                &app,
                state,
                &job_id,
                WechatEvidenceStage::Ingest,
                format!("读取案件失败: {e}"),
            )
            .await;
            return;
        }
    };
    let scanned = scan_folder(Path::new(&case.source_folder));
    if let Err(e) = documents_db::sync_documents_for_case(&pool, case_id, &scanned).await {
        set_failed(
            &app,
            state,
            &job_id,
            WechatEvidenceStage::Ingest,
            format!("同步案件材料失败: {e}"),
        )
        .await;
        return;
    }
    match documents_db::list_documents_by_case(&pool, case_id).await {
        Ok(documents) => {
            pipeline::spawn_extraction(
                app.clone(),
                pool.clone(),
                case_id.to_string(),
                documents,
                true,
            );
        }
        Err(e) => {
            set_failed(
                &app,
                state,
                &job_id,
                WechatEvidenceStage::Ingest,
                format!("读取案件材料失败: {e}"),
            )
            .await;
            return;
        }
    }

    if let Some(job) = state
        .update_job(&job_id, |job| {
            job.status = WechatEvidenceJobStatus::Completed;
            job.stage = WechatEvidenceStage::Done;
            job.pct = 100;
            job.message = "录屏取证已完成并归档到案件材料".to_string();
            job.finished_at = Some(now_iso());
        })
        .await
    {
        let _ = write_task_note(&job, &input);
        emit_job(&app, &job);
    }
}

#[tauri::command]
pub async fn start_wechat_evidence_job(
    app: AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    state: tauri::State<'_, WechatEvidenceState>,
    input: WechatEvidenceStartInput,
) -> Result<WechatEvidenceJob, String> {
    script_path(&app)?;
    if which("ffmpeg").is_none() {
        return Err("未找到 ffmpeg,无法从录屏抽帧".to_string());
    }
    let video = Path::new(&input.video_path);
    if !video.is_file() {
        return Err(format!("录屏文件不存在: {}", input.video_path));
    }
    let ext = video
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "mp4" | "mov" | "m4v") {
        return Err("仅支持 .mp4/.mov/.m4v 录屏文件".to_string());
    }
    if input.run_ocr.unwrap_or(true) {
        let ocr_cli = directories::BaseDirs::new()
            .map(|base| base.home_dir().join(".local/bin/vision-ocr-pdf"))
            .ok_or("无法定位用户目录,不能检查 OCR 工具")?;
        if !ocr_cli.exists() {
            return Err(format!("未找到 OCR 工具: {}", ocr_cli.display()));
        }
    }
    let case_id = input.case_id.as_deref().filter(|s| !s.trim().is_empty());
    let target_folder = input
        .target_folder
        .as_deref()
        .filter(|s| !s.trim().is_empty());
    let (output_root, job_case_id) = if let Some(case_id) = case_id {
        let case = cases_db::get_case(pool.inner(), case_id)
            .await
            .map_err(|e| format!("读取案件失败: {e}"))?
            .ok_or_else(|| format!("案件不存在: {}", case_id))?;
        let case_folder = Path::new(&case.source_folder);
        if !case_folder.is_dir() {
            return Err(format!("案件源文件夹不可用: {}", case.source_folder));
        }
        (
            case_folder.join("证据").join("微信录屏取证"),
            Some(case_id.to_string()),
        )
    } else if let Some(target_folder) = target_folder {
        let folder = Path::new(target_folder);
        if !folder.is_dir() {
            return Err(format!("本地输出文件夹不可用: {}", target_folder));
        }
        (folder.join("微信录屏取证"), None)
    } else {
        return Err("请选择归档案件或本地输出文件夹".to_string());
    };

    let stem = video
        .file_stem()
        .and_then(|v| v.to_str())
        .map(sanitize_path_segment)
        .unwrap_or_else(|| "录屏".to_string());
    let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let output_dir = output_root.join(format!("{stem}_{stamp}"));
    std::fs::create_dir_all(&output_dir).map_err(|e| format!("创建输出目录失败: {e}"))?;

    let job = WechatEvidenceJob {
        id: Uuid::new_v4().to_string(),
        status: WechatEvidenceJobStatus::Queued,
        stage: WechatEvidenceStage::Export,
        pct: 0,
        message: "等待开始".to_string(),
        case_id: job_case_id,
        video_path: input.video_path.clone(),
        output_dir: Some(output_dir.to_string_lossy().to_string()),
        pdf_path: None,
        report_path: None,
        error: None,
        started_at: now_iso(),
        finished_at: None,
    };
    state.upsert_job(job.clone()).await;
    emit_job(&app, &job);

    let app_clone = app.clone();
    let pool_clone = pool.inner().clone();
    let job_id = job.id.clone();
    tauri::async_runtime::spawn(async move {
        run_job(app_clone, pool_clone, job_id, input, output_dir).await;
    });

    Ok(job)
}

#[tauri::command]
pub async fn stop_wechat_evidence_job(
    app: AppHandle,
    state: tauri::State<'_, WechatEvidenceState>,
    job_id: String,
) -> Result<bool, String> {
    let mut killed = false;
    if let Some(stop_tx) = state.stop_senders.lock().await.remove(&job_id) {
        let _ = stop_tx.send(());
        killed = true;
    }
    if let Some(job) = state
        .update_job(&job_id, |job| {
            job.status = WechatEvidenceJobStatus::Stopped;
            job.message = "任务已停止,已保留当前输出目录".to_string();
            job.finished_at = Some(now_iso());
        })
        .await
    {
        emit_job(&app, &job);
        Ok(killed)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn get_wechat_evidence_job(
    state: tauri::State<'_, WechatEvidenceState>,
    job_id: String,
) -> Result<Option<WechatEvidenceJob>, String> {
    Ok(state.get_job(&job_id).await)
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(bin))
            .find(|candidate| candidate.is_file())
    })
}
