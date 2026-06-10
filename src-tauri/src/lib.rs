pub mod chat;
pub mod court_sms;
pub mod db;
pub mod deepseek;
pub mod diagnostic_log;
pub mod docx_filing;
pub mod embedding;
pub mod export;
pub mod express;
pub mod feedback;
pub mod feishu;
pub mod ingest;
pub mod lifecycle;
pub mod llm;
pub mod local_kb;
pub mod settings;
pub mod telemetry;
pub mod update;
pub mod verify;
pub mod yuandian;

use std::path::Path;

use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{Emitter, Manager};

use crate::db::cases::{self as cases_db, Case};
use crate::db::documents::{self as documents_db, Document};
use crate::ingest::case_split;
use crate::ingest::pipeline;
use crate::ingest::scanner::{scan_folder, ScannedDoc};

// ============================================================================
// 公共类型
// ============================================================================

#[derive(Debug, Serialize)]
pub struct DbHealth {
    pub ok: bool,
    pub table_count: i64,
    pub case_count: i64,
    pub db_path: String,
}

/// 导入一个案件文件夹后返回的完整结果:案件 + 扫描出的文档清单。
#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub case: Case,
    pub docs: Vec<ScannedDoc>,
    /// 是否是 upsert 命中已存在的案件(true = 之前导入过,这次只刷新)
    pub is_existing: bool,
}

/// 案件 + 它的文档列表(用于详情页)。
#[derive(Debug, Serialize)]
pub struct CaseWithDocs {
    pub case: Case,
    pub documents: Vec<Document>,
}

#[derive(Debug, Serialize)]
pub struct CaseOsInputExport {
    pub manifest_path: String,
    pub memo_path: String,
    pub case_id: String,
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// 读一个文本文件的内容(用于 AI 产物 markdown 渲染)。
///
/// 安全限制:
///   - 只读 UTF-8 文本(.md / .txt / .html / .htm)
///   - 大小上限 5MB(超过的可能是误识别,前端展示不动)
const TEXT_FILE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// 用系统默认应用打开一个文件(PDF → Preview,docx → Word,图片 → Preview,etc.)。
#[tauri::command]
fn open_in_default_app(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("文件不存在: {}", path));
    }
    // 用 std::process::Command 调 macOS 的 `open` 命令
    // 简单可靠,不依赖额外 plugin(虽然有 tauri-plugin-opener,但它的 API 限制更多)
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("无法打开: {}", e))?;
    Ok(())
}

/// 用系统默认浏览器打开一个 URL(Settings 里的"申请 token"链接用)。
///
/// 2026-05-24 k:Tauri WebView 里 `<a target="_blank">` 不会跳系统浏览器,必须走原生 open。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("不是合法 http(s) URL: {}", url));
    }
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("无法打开浏览器: {}", e))?;
    Ok(())
}

/// 在 Finder 中显示该路径(选中并打开父目录)。macOS 用 `open -R`。
#[tauri::command]
fn reveal_in_finder(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("路径不存在: {}", path));
    }
    std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("无法在 Finder 中显示: {}", e))?;
    Ok(())
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("文件不存在: {}", path));
    }
    if !p.is_file() {
        return Err(format!("不是文件: {}", path));
    }
    let size = std::fs::metadata(p)
        .map(|m| m.len())
        .map_err(|e| format!("读不到文件元信息: {}", e))?;
    if size > TEXT_FILE_MAX_BYTES {
        return Err(format!(
            "文件太大({} 字节),超过 {} MB 上限,渲染会卡",
            size,
            TEXT_FILE_MAX_BYTES / 1024 / 1024
        ));
    }
    std::fs::read_to_string(p).map_err(|e| format!("读文件失败: {}", e))
}

/// 抽 Word/RTF/ODT 等 office 文档的纯文本(用 macOS 自带 textutil)。
///
/// V0.1 阶段最简单可靠的方案:`textutil -convert txt -stdout <path>` 把
/// `.docx / .doc / .rtf / .odt / .html / .webarchive` 都能转纯文本。
/// 不依赖 Rust office crate(它们很多在中文场景上有坑),不用 Word 启动开销。
///
/// 后续 Layer 2 完整版做 .pdf(走 MinerU/pdfium)。
#[tauri::command]
fn extract_doc_text(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("文件不存在: {}", path));
    }
    if !p.is_file() {
        return Err(format!("不是文件: {}", path));
    }
    let size = std::fs::metadata(p)
        .map(|m| m.len())
        .map_err(|e| format!("读不到文件元信息: {}", e))?;
    // 50MB 上限——比纯文本宽,因为 docx 含图片/字体会大
    const DOC_MAX_BYTES: u64 = 50 * 1024 * 1024;
    if size > DOC_MAX_BYTES {
        return Err(format!(
            "文件太大({:.1} MB),超过 {} MB 上限",
            size as f64 / 1024.0 / 1024.0,
            DOC_MAX_BYTES / 1024 / 1024
        ));
    }
    let output = std::process::Command::new("textutil")
        .arg("-convert")
        .arg("txt")
        .arg("-stdout")
        .arg(&path)
        .output()
        .map_err(|e| format!("调 textutil 失败: {}(macOS 自带,正常情况不会出错)", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "textutil 转换失败: {}",
            if stderr.is_empty() {
                "未知错误"
            } else {
                stderr.trim()
            }
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("textutil 输出不是 UTF-8: {}", e))
}

/// 纯扫描(不入库),给前端"先看看"用。保留兼容 task #5 时的接口。
#[tauri::command]
fn scan_case_folder(path: String) -> Result<Vec<ScannedDoc>, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("路径不存在: {}", path));
    }
    if !p.is_dir() {
        return Err(format!("不是文件夹: {}", path));
    }
    Ok(scan_folder(p))
}

/// 导入一个案件文件夹:扫描 + upsert 案件 + 替换文档列表 + 后台 spawn 字段抽取。
///
/// 入库后重启 App,这个案件依然在,这是 V0.1 端到端的核心动作。
/// 字段抽取在后台 tokio task 跑,前端订阅 "extraction_progress" 事件看进度。
#[tauri::command]
async fn import_case_folder(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    path: String,
) -> Result<ImportResult, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("路径不存在: {}", path));
    }
    if !p.is_dir() {
        return Err(format!("不是文件夹: {}", path));
    }

    // 1) 用文件夹最后一段做默认案件名
    let default_name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "未命名案件".to_string());

    // 2) 先看是不是已经导入过(判断 is_existing 标记)
    let pre_existing = cases_db::find_case_by_folder(pool.inner(), &path)
        .await
        .map_err(db_err)?;
    let is_existing = pre_existing.is_some();

    // 3) Upsert 案件
    let case = cases_db::upsert_case_for_folder(pool.inner(), &path, &default_name, "诉讼")
        .await
        .map_err(db_err)?;

    // 4) 扫描文件夹(这里是同步,scanner 很快)
    let scanned = scan_folder(p);

    // 5) 替换文档列表
    documents_db::replace_documents_for_case(pool.inner(), &case.id, &scanned)
        .await
        .map_err(db_err)?;

    // 6) 后台启动字段抽取(立即返回,前端通过事件订阅进度)
    let docs_for_extraction = documents_db::list_documents_by_case(pool.inner(), &case.id)
        .await
        .map_err(db_err)?;
    pipeline::spawn_extraction(
        app.clone(),
        pool.inner().clone(),
        case.id.clone(),
        docs_for_extraction,
    );

    Ok(ImportResult {
        case,
        docs: scanned,
        is_existing,
    })
}

/// 多案件检测:对一个文件夹做「拆分预案」(只读,不写库)。前端据此决定是否弹拆分预览。
/// `multi=false` 时按现状走 `import_case_folder` 单案导入即可。
/// 详见 `docs/提案-多案件文件夹识别-2026-06-04.md`。
#[tauri::command]
async fn plan_import_folder(
    pool: tauri::State<'_, SqlitePool>,
    path: String,
) -> Result<case_split::ImportPlan, String> {
    let p = Path::new(&path);
    if !p.is_dir() {
        return Err(format!("不是文件夹: {}", path));
    }
    let mut plan = case_split::plan_folder(p);
    // 根文件夹此前是否已作为「单个案件」导入过(拆分会与旧案重复 → 前端告警)
    plan.root_already_imported = cases_db::find_case_by_folder(pool.inner(), &path)
        .await
        .map_err(db_err)?
        .is_some();
    Ok(plan)
}

/// 确认后的一个待建案件(前端可改名)。
#[derive(Debug, serde::Deserialize)]
pub struct CommitCase {
    pub dir: String,
    pub name: String,
}

/// 拆分批量建案的**写库部分**(不含后台抽取,便于真库集成测试)。
///
/// - `root`:被拖入的上层文件夹。若它此前已作为**单个案件**导入过(且不在本次要建的子案件里),
///   先把那个旧的整体案件删掉 —— 否则它的文档行占着 `source_path`,子案件 INSERT 会撞唯一约束。
///   删旧案 = 用拆分结果替换它。
/// - 每个案件 = upsert(子目录) + 扫描 + 替换文档。
/// - 共用材料(Phase 2,migration 0019 后)挂到**每个**案件:`(case_id, source_path)` 复合唯一,
///   同一文件在各案各一行。各案件子目录互不相交、共用目录是独立兄弟,故同一案内不会出现重复 source_path。
async fn build_split_cases(
    pool: &SqlitePool,
    root: &str,
    cases: &[CommitCase],
    shared_dirs: &[String],
) -> Result<Vec<ImportResult>, String> {
    if cases.is_empty() {
        return Err("没有要导入的案件".to_string());
    }
    // 旧的「整体作单案」记录 → 删掉,释放其文档占用的 source_path
    if !root.is_empty() && !cases.iter().any(|c| c.dir == root) {
        if let Some(old) = cases_db::find_case_by_folder(pool, root)
            .await
            .map_err(db_err)?
        {
            cases_db::delete_case(pool, &old.id).await.map_err(db_err)?;
        }
    }

    let mut results = Vec::with_capacity(cases.len());
    for c in cases.iter() {
        let dir = Path::new(&c.dir);
        if !dir.is_dir() {
            return Err(format!("案件目录不存在: {}", c.dir));
        }
        let name = if c.name.trim().is_empty() {
            dir.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "未命名案件".to_string())
        } else {
            c.name.trim().to_string()
        };
        let is_existing = cases_db::find_case_by_folder(pool, &c.dir)
            .await
            .map_err(db_err)?
            .is_some();
        let case = cases_db::upsert_case_for_folder(pool, &c.dir, &name, "诉讼")
            .await
            .map_err(db_err)?;
        let mut scanned = scan_folder(dir);
        // 共用材料挂到**每个**案件(migration 0019 后 (case_id, source_path) 复合唯一,可多挂)
        for sd in shared_dirs {
            let sp = Path::new(sd);
            if sp.is_dir() {
                scanned.extend(scan_folder(sp));
            }
        }
        documents_db::replace_documents_for_case(pool, &case.id, &scanned)
            .await
            .map_err(db_err)?;
        results.push(ImportResult {
            case,
            docs: scanned,
            is_existing,
        });
    }
    Ok(results)
}

/// 按确认后的拆分预案**批量建案**(写库 + 每案后台抽取)。前端拆分弹窗点确认后调用。
#[tauri::command]
async fn commit_import_folder(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    root: String,
    cases: Vec<CommitCase>,
    shared_dirs: Vec<String>,
) -> Result<Vec<ImportResult>, String> {
    let results = build_split_cases(pool.inner(), &root, &cases, &shared_dirs).await?;
    // 写库成功后,逐案启动后台抽取(年份大文件夹会并发 N 个,Phase 1 限制,后续做节流)
    for r in &results {
        let docs = documents_db::list_documents_by_case(pool.inner(), &r.case.id)
            .await
            .map_err(db_err)?;
        pipeline::spawn_extraction(app.clone(), pool.inner().clone(), r.case.id.clone(), docs);
    }
    Ok(results)
}

/// 列出所有已导入的案件(用于"最近案件" / 案件列表页)。
#[tauri::command]
async fn list_cases(pool: tauri::State<'_, SqlitePool>) -> Result<Vec<Case>, String> {
    cases_db::list_cases(pool.inner()).await.map_err(db_err)
}

/// 删除一个案件(级联删除所有关联文档/事件/联系人)。
///
/// 不动原始文件夹,只删 CaseBoard 数据库里这个案件的记录。
#[tauri::command]
async fn delete_case(pool: tauri::State<'_, SqlitePool>, id: String) -> Result<(), String> {
    cases_db::delete_case(pool.inner(), &id)
        .await
        .map_err(db_err)
}

/// 取案件详情 + 文档列表(详情页用)。
#[tauri::command]
async fn get_case_with_docs(
    pool: tauri::State<'_, SqlitePool>,
    id: String,
) -> Result<CaseWithDocs, String> {
    let case = cases_db::get_case(pool.inner(), &id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", id))?;
    let documents = documents_db::list_documents_by_case(pool.inner(), &id)
        .await
        .map_err(db_err)?;
    Ok(CaseWithDocs { case, documents })
}

#[tauri::command]
async fn list_case_logs(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<Vec<db::logs::CaseLog>, String> {
    cases_db::get_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", case_id))?;
    db::logs::list_by_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)
}

#[tauri::command]
async fn add_case_log(
    pool: tauri::State<'_, SqlitePool>,
    input: db::logs::NewCaseLog,
) -> Result<db::logs::CaseLog, String> {
    let content = input.content.trim();
    if content.is_empty() {
        return Err("日志内容不能为空".into());
    }
    if content.chars().count() > 5000 {
        return Err("日志内容过长，最多 5000 字".into());
    }
    cases_db::get_case(pool.inner(), &input.case_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", input.case_id))?;
    db::logs::add(pool.inner(), input).await.map_err(db_err)
}

#[tauri::command]
async fn delete_case_log(pool: tauri::State<'_, SqlitePool>, id: String) -> Result<u64, String> {
    db::logs::delete(pool.inner(), &id).await.map_err(db_err)
}

#[tauri::command]
async fn export_case_os_input(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<CaseOsInputExport, String> {
    let case = cases_db::get_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", case_id))?;
    if case.source_folder == "__DEMO__" {
        return Err("示例案件没有真实案件目录，不能导出案件 OS 输入源".into());
    }
    let folder = Path::new(&case.source_folder);
    if !folder.is_dir() {
        return Err(format!("案件源文件夹不可用: {}", case.source_folder));
    }
    let documents = documents_db::list_documents_by_case(pool.inner(), &case.id)
        .await
        .map_err(db_err)?;

    let handoff_dir = folder.join("_caseboard");
    std::fs::create_dir_all(&handoff_dir).map_err(|e| format!("创建交接目录失败: {}", e))?;

    let generated_at = chrono::Utc::now().to_rfc3339();
    let docs_json: Vec<serde_json::Value> = documents
        .iter()
        .map(|d| {
            let relative_path = Path::new(&d.source_path)
                .strip_prefix(folder)
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            serde_json::json!({
                "id": d.id,
                "filename": d.filename,
                "source_path": d.source_path,
                "relative_path": relative_path,
                "stage": d.stage,
                "category": d.category,
                "source": d.source,
                "is_ai_artifact": d.is_ai_artifact,
                "extraction_status": d.extraction_status,
                "extracted_text_path": d.extracted_text_path,
                "missing": d.missing,
            })
        })
        .collect();

    let manifest = serde_json::json!({
        "schema": "caseboard.case_os_input.v1",
        "generated_at": generated_at,
        "source": "CaseBoard",
        "case": {
            "id": &case.id,
            "name": &case.name,
            "case_type": &case.case_type,
            "source_folder": &case.source_folder,
            "case_no": case.agg_case_no.as_deref().or(case.case_no.as_deref()),
            "court": case.agg_court.as_deref().or(case.court.as_deref()),
            "cause": case.agg_cause.as_deref().or(case.cause.as_deref()),
            "summary": case.case_summary.as_deref(),
            "workflow_status": case.workflow_status.as_deref(),
            "case_status": &case.case_status,
            "party_contacts_json": case.agg_party_contacts.as_deref(),
            "court_contacts_json": case.agg_court_contacts.as_deref(),
            "key_dates_json": case.agg_key_dates.as_deref(),
            "fees_json": case.agg_fees.as_deref(),
            "report_path": case.case_report_path.as_deref(),
            "risk_assessment_path": case.risk_assessment_path.as_deref(),
            "deep_dive_report_path": case.deep_dive_report_path.as_deref(),
            "full_report_path": case.full_report_path.as_deref(),
        },
        "documents": docs_json,
        "case_os_entry": {
            "case_directory": folder.to_string_lossy(),
            "trigger": "在该案件目录触发：案件OS / 继续",
            "resume_check_order": [
                "CLAUDE.md",
                "LOG.md",
                "_archive/case-os-state.json",
                "intermediate/_index.json"
            ],
            "note": "CaseBoard 仅导出本地输入清单；案件OS执行仍以案件目录、LOG.md 与 _archive/case-os-state.json 为权威。"
        }
    });

    let manifest_path = handoff_dir.join("case_os_input.json");
    let manifest_text =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("生成 JSON 失败: {}", e))?;
    std::fs::write(&manifest_path, manifest_text).map_err(|e| format!("写输入清单失败: {}", e))?;

    let docs_md = documents
        .iter()
        .take(200)
        .map(|d| {
            format!(
                "- {} | {} | {} | {}",
                d.filename,
                d.category.as_deref().unwrap_or("未分类"),
                d.extraction_status,
                d.source_path
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let memo = format!(
        "# CaseBoard 案件OS入口\n\n\
         - 生成时间：{}\n\
         - 案件名称：{}\n\
         - 案件目录：{}\n\
         - 输入清单：{}\n\n\
         ## 执行入口\n\n\
         在本案件目录触发 `案件OS` 或 `继续`。若目录已有 `CLAUDE.md`、`LOG.md`、`_archive/case-os-state.json`、`intermediate/_index.json`，按案件OS断点恢复规则继续，不重新通读全卷。\n\n\
         ## 看板文档清单\n\n{}\n",
        generated_at,
        case.name,
        case.source_folder,
        manifest_path.to_string_lossy(),
        if docs_md.is_empty() { "- 暂无文档".to_string() } else { docs_md }
    );
    let memo_path = handoff_dir.join("CASE_OS入口.md");
    std::fs::write(&memo_path, memo).map_err(|e| format!("写入口说明失败: {}", e))?;

    Ok(CaseOsInputExport {
        manifest_path: manifest_path.to_string_lossy().to_string(),
        memo_path: memo_path.to_string_lossy().to_string(),
        case_id: case.id,
    })
}

/// 读取用户设置(给前端 SettingsModal 用)。
///
/// 自动补上默认 endpoint(MinerU / Ollama),但 api_key 不补默认值。
#[tauri::command]
fn get_settings() -> Result<settings::Settings, String> {
    settings::read_settings().map(|s| s.with_defaults_for_display())
}

/// 2026-05-25 V0.1.6 · 若 cases 表为空,seed 一个示例案件「张三 诉 李四 民间借贷」。
/// onboarding 完成时(开始使用 / 稍后再配置都触发)调一次。
#[tauri::command]
async fn seed_demo_case_if_empty(pool: tauri::State<'_, SqlitePool>) -> Result<bool, String> {
    db::seed::seed_demo_case_if_empty(pool.inner())
        .await
        .map_err(db_err)
}

/// 2026-05-25 V0.1.6 · 验证 MinerU API token,前端「验证」按钮触发。
#[tauri::command]
async fn verify_mineru_key(token: String) -> verify::VerifyResult {
    verify::verify_mineru_key(&token).await
}

/// 2026-05-25 V0.1.6 · 验证 DeepSeek API key,前端「验证」按钮触发。
#[tauri::command]
async fn verify_deepseek_key(api_key: String, endpoint: Option<String>) -> verify::VerifyResult {
    verify::verify_deepseek_key(&api_key, endpoint.as_deref()).await
}

/// 通用云端 LLM key 验证（按提供商分流）。deepseek→/user/balance，mimo/custom→/v1/models。
#[tauri::command]
async fn verify_cloud_llm_key(
    provider: String,
    api_key: String,
    endpoint: Option<String>,
) -> verify::VerifyResult {
    verify::verify_cloud_llm_key(&provider, &api_key, endpoint.as_deref()).await
}

/// 2026-05-25 V0.1.8 · 验证元典(open.chineselaw.com)API key,前端「验证」按钮触发。
#[tauri::command]
async fn verify_yuandian_key(api_key: String) -> verify::VerifyResult {
    verify::verify_yuandian_key(&api_key).await
}

/// 2026-05-25 V0.1.8 · 检测版本更新。
///
/// 前端启动时调一次(静默,失败不报错),设置页「检查更新」按钮也调。
/// 数据源:lawtools.top 仓库的 version.json。返回 UpdateInfo 给前端判断是否弹提示。
#[tauri::command]
async fn check_for_update() -> update::UpdateInfo {
    update::check_for_update().await
}

/// 2026-05-25 V0.1.8 · 拿当前 App 版本(等同于 Tauri 的 getVersion,但走 Cargo.toml)。
///
/// 前端有 @tauri-apps/api 的 getVersion 也行,这里提供同步包装方便偶尔单文件用。
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 写入用户设置(全量覆盖,前端发来什么就存什么)。
#[tauri::command]
fn save_settings(payload: settings::Settings) -> Result<(), String> {
    settings::write_settings(&payload)
}

/// 2026-05-26 V0.1.13 · 单独写"首页在办案件"用户拖动后的顺序。
///
/// 不让前端走 save_settings 全量覆盖 — 那会跟 SettingsModal 同时写的话有覆盖竞态。
/// 这里 read-modify-write:只动 home_case_order 字段,其他不动。
#[tauri::command]
fn update_home_case_order(case_ids: Vec<String>) -> Result<(), String> {
    let mut s = settings::read_settings()?;
    s.home_case_order = if case_ids.is_empty() {
        None
    } else {
        Some(case_ids)
    };
    settings::write_settings(&s)
}

/// 检测本机模型 + llama-server 状态(给 onboarding 和 Settings 用)。
#[tauri::command]
fn detect_local_readiness(model_dir: Option<String>) -> lifecycle::LocalReadiness {
    lifecycle::detect_local_readiness(model_dir.as_deref())
}

/// 后台启动 llama-server。如果已经在跑 / 模型缺失 / 二进制缺失就报错。
#[tauri::command]
async fn ensure_local_ready() -> Result<(), String> {
    let s = settings::read_settings().unwrap_or_default();
    lifecycle::ensure_local_ready(s.local_model_dir.as_deref()).await
}

/// 从一段诉讼文书纯文本里抽取结构化字段(案号/法院/原告/被告/案由/金额/起诉日期)。
///
/// 2026-05-25:修 bug — 之前硬编码 `local_llamacpp_default()`,导致用户即使
/// 选了云端 LLM,在 MarkdownModal 打开单份文档预览时也走本机,本机服务没起
/// 来就报"LLM 提取失败"。现改成 `from_settings`,跟主 pipeline 保持一致。
#[tauri::command]
async fn extract_fields_from_text(text: String) -> Result<llm::ExtractedFields, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let config = llm::LlmConfig::from_settings(&settings);
    llm::extract_case_fields(&config, &text)
        .await
        .map_err(|e| e.to_string())
}

/// 对所有案件重跑一次 LLM 全局抽(2026-05-24 h · 替代旧规则 aggregator)。
///
/// 用途:升级 prompt(或新增字段)后,把存量案件的 cases.agg_* + 案件分析报告**全部刷新**。
/// 用户在详情页"↻ 重新计算画像"按钮触发。串行跑,每个案件约 10-30 秒(取决于文档数 + 网络)。
///
/// 注意:**会重新调 LLM**(不像旧 aggregator 不调 LLM),所以耗时更长但更准。
/// 前端按钮文案要提示"重抽中,可能需要几分钟"。
#[tauri::command]
async fn reaggregate_all_cases(
    pool: tauri::State<'_, SqlitePool>,
) -> Result<ingest::global_pipeline::ReaggregateReport, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let llm_config = llm::LlmConfig::from_settings(&settings);
    ingest::global_pipeline::rerun_all_cases(pool.inner(), &llm_config)
        .await
        .map_err(db_err)
}

/// 2026-05-24 k · 元典 P1 主动触发:查被执行人 + LLM 风险提示报告。
///
/// 流程:
///   1. 从 cases.agg_party_contacts 拿被执行人 → 跑 yuandian_basic_query → raw JSON 落盘
///   2. 喂 DeepSeek 出风险报告 + 深挖建议 JSON
///   3. 写 cases.risk_assessment_path / risk_assessment_at
///   4. 返回报告路径 + 深挖建议(给前端)
#[derive(serde::Serialize)]
struct YuandianP1Response {
    orchestrator: yuandian::orchestrator::OrchestratorReport,
    assessment: yuandian::risk_assessment::AssessmentReport,
}

/* ============================================================
 * 2026-05-25 · 还款记录 (case_payments) commands
 * ============================================================ */

#[tauri::command]
async fn add_payment(
    pool: tauri::State<'_, SqlitePool>,
    new: db::payments::NewPayment,
) -> Result<db::payments::Payment, String> {
    db::payments::add(pool.inner(), new).await.map_err(db_err)
}

#[tauri::command]
async fn list_payments(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<Vec<db::payments::Payment>, String> {
    db::payments::list_by_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)
}

#[tauri::command]
async fn delete_payment(pool: tauri::State<'_, SqlitePool>, id: String) -> Result<u64, String> {
    db::payments::delete(pool.inner(), &id)
        .await
        .map_err(db_err)
}

/// V0.2.2 · 软删一个文档(用户从材料列表手动移除,主要给 AI artifact 用)。只标 deleted_at,不动磁盘。
#[tauri::command]
async fn delete_document(pool: tauri::State<'_, SqlitePool>, id: String) -> Result<u64, String> {
    let now = chrono::Local::now().to_rfc3339();
    db::documents::soft_delete_document(pool.inner(), &id, &now)
        .await
        .map_err(db_err)
}

/// 2026-05-31 V0.3 · 强制重抽单个文档(源文件列表「重新抽取」按钮)。
///
/// 重置该文档 extraction_status='pending' + 清 last_error → spawn 后台抽取(单文档,
/// 走现有 `extraction_progress` 事件通道,前端订阅看进度 + 完成自动刷新)。
/// ⚠️ 会重跑 OCR/LLM(PDF 走云端 OCR 会再烧 MinerU 积分,用户主动选择)。
#[tauri::command]
async fn reextract_document(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    doc_id: String,
) -> Result<(), String> {
    // 复用共享入口(与 chat 工具 reextract_document 同一逻辑,防漂移)。
    pipeline::trigger_reextract(app, pool.inner(), &doc_id)
        .await
        .map(|_| ())
}

/// 2026-05-25 V0.1.7 · 完整报告:合并风险报告 + 深挖报告 → DeepSeek 总结出第三份
#[tauri::command]
async fn yuandian_full_report(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<yuandian::full_report::FullReportResult, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let llm_config = llm::LlmConfig::from_settings(&settings);
    Ok(yuandian::full_report::run_full_report(pool.inner(), &case_id, &llm_config).await)
}

/// 2026-05-24 k-9 · P2 深挖:用 P1 LLM 给的 dig_hints 拉关联公司 / 案号 / 第三方主体,
/// 出深查报告(参考股权转让案件 yuandian_深查 格式)
#[tauri::command]
async fn yuandian_deep_dive(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<yuandian::deep_dive::DeepDiveReport, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let api_key = settings
        .yuandian_api_key
        .as_deref()
        .ok_or_else(|| "元典 API key 未配置 — 请到 Settings 里填".to_string())?;
    let llm_config = llm::LlmConfig::from_settings(&settings);
    Ok(yuandian::deep_dive::run_deep_dive(pool.inner(), &case_id, api_key, &llm_config).await)
}

#[tauri::command]
async fn yuandian_basic_query(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<YuandianP1Response, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let api_key = settings
        .yuandian_api_key
        .as_deref()
        .ok_or_else(|| "元典 API key 未配置 — 请到 Settings 里填".to_string())?;

    // P1.1 跑元典 16 端点
    let orch = yuandian::orchestrator::basic_query(pool.inner(), &case_id, api_key).await?;

    // P1.2 LLM 写风险报告
    let llm_config = llm::LlmConfig::from_settings(&settings);
    let assess = yuandian::risk_assessment::run_assessment(
        pool.inner(),
        &case_id,
        &llm_config,
        &orch.raw_files,
    )
    .await;

    Ok(YuandianP1Response {
        orchestrator: orch,
        assessment: assess,
    })
}

/// 2026-05-24 j · 导出案件分析报告为 HTML(陶土红 × 羊皮纸专业风格)。
/// 前端先用 dialog/save 拿到 save_path,然后调本 command。
#[tauri::command]
async fn export_report_html(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
    save_path: String,
) -> Result<String, String> {
    let p = export::export_report_html_to(pool.inner(), &case_id, std::path::Path::new(&save_path))
        .await?;
    Ok(p.to_string_lossy().to_string())
}

/// 2026-05-24 j · 导出案件分析报告为 Word(.docx)。2026-06-04 改走 `docx_filing` base 档
/// (原生 OOXML,零外部依赖),替代旧的 macOS textutil 路径。
#[tauri::command]
async fn export_report_docx(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
    save_path: String,
) -> Result<String, String> {
    let p = export::export_report_docx_to(pool.inner(), &case_id, std::path::Path::new(&save_path))
        .await?;
    Ok(p.to_string_lossy().to_string())
}

/// 2026-05-25 V0.1.7 · 通用 MD → HTML 导出。
/// 用于风险报告 / 深挖报告 / 完整报告(任何 MD 文件 + 标题)。
#[tauri::command]
async fn export_md_html(
    md_path: String,
    title: String,
    save_path: String,
) -> Result<String, String> {
    let p = export::export_md_html_to(
        std::path::Path::new(&md_path),
        &title,
        std::path::Path::new(&save_path),
    )
    .await?;
    Ok(p.to_string_lossy().to_string())
}

/// 2026-05-25 V0.1.7 · 通用 MD → Word 导出。
#[tauri::command]
async fn export_md_docx(
    md_path: String,
    title: String,
    save_path: String,
) -> Result<String, String> {
    let p = export::export_md_docx_to(
        std::path::Path::new(&md_path),
        &title,
        std::path::Path::new(&save_path),
    )
    .await?;
    Ok(p.to_string_lossy().to_string())
}

/// 2026-05-31 V0.3 M1 · 把 save_artifact 生成的文书导出为 **Word(法律格式)**。
///
/// 走 `docx_filing` **filing 档**(MD→原生 OOXML),复刻 quote.law 样本排版(方正小标宋标题 /
/// 黑体小标题 / 仿宋正文 / 两端对齐 / 首行缩进2字 / 1.5倍行距);与 `export_md_docx` 的 base 档
/// 共享同一引擎,仅多几条法律叠加(列表去圆点 / 软换行并段 / 丢分隔线)。
/// `doc_id` 是 documents 行 id;标题优先取文书元信息头,缺则用文件名兜底。
#[tauri::command]
async fn export_filing_docx(
    pool: tauri::State<'_, SqlitePool>,
    doc_id: String,
    save_path: String,
) -> Result<String, String> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source_path, filename FROM documents WHERE id = ?")
            .bind(&doc_id)
            .fetch_optional(pool.inner())
            .await
            .map_err(|e| format!("查文书失败:{}", e))?;
    let (md_path, filename) = row.ok_or_else(|| "文书不存在(doc_id 无效)".to_string())?;
    let md = std::fs::read_to_string(&md_path).map_err(|e| format!("读文书 MD 失败:{}", e))?;
    let title = docx_filing::extract_filing_title(&md)
        .unwrap_or_else(|| filename.trim_end_matches(".md").to_string());
    let bytes = docx_filing::build_filing_docx_bytes(&title, &md)?;
    std::fs::write(&save_path, &bytes).map_err(|e| format!("写 docx 失败:{}", e))?;
    Ok(save_path)
}

/// save_editor_doc 返回:被写回的 document id(本批永远 = 传入 doc_id,原地覆盖)。
#[derive(serde::Serialize)]
struct EditorSaveResult {
    doc_id: String,
}

/// 2026-05-31 V0.3 D1+D2 · Milkdown 编辑器保存:把编辑后的正文写回该文书 .md 文件。
///
/// 前端只传 (title, 正文 content_md,**不含注释头**);后端按 doc_id 查 category(=doc_type)
/// 重建 filing 注释头(对齐 `chat::tools::artifact::persist_filing` 格式)→ 原地覆盖 source_path
/// → 更新 size_bytes/modified_at。**只允许编辑 AI 产物文书**(is_ai_artifact=1),绝不覆盖
/// 导入的原始文件。导出/解析头依赖此格式,见 docs/V0.3-Milkdown编辑器-实施落地.md §1.4/§1.6。
#[tauri::command]
async fn save_editor_doc(
    pool: tauri::State<'_, SqlitePool>,
    doc_id: String,
    title: String,
    content_md: String,
) -> Result<EditorSaveResult, String> {
    let id = write_editor_doc(pool.inner(), &doc_id, &title, &content_md).await?;
    Ok(EditorSaveResult { doc_id: id })
}

/// `save_editor_doc` 的可测内核(不依赖 Tauri State,单测直接调)。
async fn write_editor_doc(
    pool: &SqlitePool,
    doc_id: &str,
    title: &str,
    content_md: &str,
) -> Result<String, String> {
    if content_md.len() as u64 > TEXT_FILE_MAX_BYTES {
        return Err(format!(
            "文书内容太大({} 字节),超过 {} MB 上限",
            content_md.len(),
            TEXT_FILE_MAX_BYTES / 1024 / 1024
        ));
    }
    let row: Option<(String, Option<String>, bool, String)> = sqlx::query_as(
        "SELECT source_path, category, is_ai_artifact, source FROM documents \
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(doc_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("查文书失败:{}", e))?;
    let (source_path, category, is_ai_artifact, source) =
        row.ok_or_else(|| "文书不存在或已删除(doc_id 无效)".to_string())?;
    // 安全闸 1:绝不让编辑器覆盖导入的原始文件(诉状/合同/判决书等原始证据)。
    if !is_ai_artifact {
        return Err("只能编辑 AI 生成的文书,不能覆盖导入的原始文件".to_string());
    }
    // 安全闸 2(V0.3):只允许编辑 app 自有的 chat 文档(source='chat' 分析产物 / 'chat_artifact'
    // 起草文书,均落在 app data 的 extracts/ 内)。编辑器现在对更多 AI 文档开放,但 scanner 会把
    // 用户拖进来的 AI 名 .md/.html 也标 is_ai_artifact(source='scan',source_path 在用户案件
    // 文件夹)—— **原地覆写会改用户原文件(数据丢失)**。用 source 死锁覆写范围,与前端编辑闸同源。
    if source != "chat" && source != "chat_artifact" {
        return Err("只能编辑 AI 助手生成的文书,不能覆盖导入/扫描的原始文件".to_string());
    }

    // 标题写进 HTML 注释,安全化防破坏注释结构(去换行 / 去 `-->`)。
    let safe_title = title.replace(['\n', '\r'], " ").replace("-->", "—>");
    let safe_title = safe_title.trim();
    let doc_type = category.unwrap_or_default();
    let now_iso = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // 重建 filing 头 + 正文(格式对齐 artifact.rs::persist_filing)。
    let body = format!(
        "<!-- filing · doc_type={} · title={} · ts={} -->\n\n{}",
        doc_type, safe_title, now_iso, content_md
    );

    tokio::fs::write(&source_path, &body)
        .await
        .map_err(|e| format!("写文书失败:{}", e))?;

    sqlx::query("UPDATE documents SET size_bytes = ?, modified_at = ? WHERE id = ?")
        .bind(body.len() as i64)
        .bind(&now_iso)
        .bind(doc_id)
        .execute(pool)
        .await
        .map_err(|e| format!("更新文书元信息失败:{}", e))?;

    Ok(doc_id.to_string())
}

/// 2026-05-24 i · 单个案件主动跑一次 LLM 全局抽(用户点「📖 案件报告」按钮触发)。
///
/// 用途:用户进入详情页,如果案件还没生成报告(case_report_path 为 null),
/// 点报告按钮即可立刻触发抽取 + 等结果。也用于强制刷新已有报告。
///
/// 阻塞返回(前端 await 完才弹报告 Modal),时间通常 10-30 秒。
#[tauri::command]
async fn global_extract_case(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<ingest::global_pipeline::GlobalExtractReport, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let llm_config = llm::LlmConfig::from_settings(&settings);
    Ok(ingest::global_pipeline::run_global_extract(pool.inner(), &case_id, &llm_config).await)
}

/// 2026-05-24 e:收集反馈用的诊断信息(给前端弹窗预填用)。
///
/// 收集内容:版本 / OS / provider / 案件数 / 文档统计 / 最近失败 / 匿名 client_id /
/// **2026-05-26 V0.1.11 新加**:Settings 脱敏快照 + 系统级(磁盘/DB) + App stderr ring buffer +
/// 前端 console 错误(由前端打开弹窗时累积传入)。
/// **不含**案件名 / 当事人 / 文档内容(隐私铁律)。
#[tauri::command]
async fn collect_feedback_diagnostic(
    pool: tauri::State<'_, SqlitePool>,
    console_errors: Option<Vec<feedback::ConsoleError>>,
) -> Result<feedback::DiagnosticInfo, String> {
    feedback::collect(pool.inner(), console_errors.unwrap_or_default()).await
}

/// 2026-05-24 e:把诊断信息 + 用户描述拼成 MD,写到 ~/Desktop/。
/// 返回最终文件的绝对路径(前端用来 reveal_in_finder)。
#[tauri::command]
async fn save_feedback_md(
    info: feedback::DiagnosticInfo,
    description: String,
) -> Result<String, String> {
    let path = feedback::save_to_desktop(&info, &description)?;
    Ok(path.to_string_lossy().to_string())
}

/// 2026-05-27 V0.1.13+:打开默认邮件客户端发反馈给作者。
///
/// 实现策略(macOS 主路径):
///   1. 先 osascript 调 Mail.app:自动填收件人 / 主题 / 正文 +「带附件」
///   2. AppleScript 失败(用户没装 Mail.app / 没授权 AppleScript)→ fallback
///      `open mailto:` 链接(默认邮件客户端打开新邮件,但**不带附件**,需要用户手动拖入)
///
/// 返回 `("applescript"|"mailto", warning_message_or_empty)`,
/// 让前端 toast 提示用户:走的哪条路径 + 是否需要手动拖附件。
#[tauri::command]
async fn send_feedback_email(
    md_path: String,
    to: String,
    subject: String,
) -> Result<(String, String), String> {
    feedback::send_via_default_mail(&md_path, &to, &subject).await
}

/// 2026-05-24 e:取 DeepSeek 当前余额 + 今日消费。
///
/// `refresh = true`:发请求拉新数据,落 DB 快照;失败返回错误
/// `refresh = false`:只读 DB 缓存(立即返回,不发请求);无缓存时返回 None
///
/// 仅在 settings.effective_llm_provider() == "cloud" + 有 api_key 时有意义,
/// 前端调用前自己判断(本 command 不做 provider 校验,api_key 缺失时返回 NoApiKey 错误)。
#[tauri::command]
async fn get_deepseek_balance(
    pool: tauri::State<'_, SqlitePool>,
    refresh: bool,
) -> Result<Option<deepseek::DeepSeekBalance>, String> {
    if refresh {
        let settings = settings::read_settings().unwrap_or_default();
        let Some(api_key) = settings.cloud_llm_api_key.as_deref() else {
            return Err("尚未配置云端模型 API Key".into());
        };
        let bal = deepseek::fetch_balance_and_persist(pool.inner(), api_key)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(bal))
    } else {
        deepseek::cached_balance(pool.inner()).await.map_err(db_err)
    }
}

/// 2026-05-24 e:手工覆盖案件工作流状态(看板卡片右上角的 chip)。
///
/// `status = None` → 清空,前端走自动推断;
/// `status = Some(...)` → 用户手工选过,优先取用户值。
///
/// 不校验 status 字面值(枚举约束在前端 TS),DB 层透传。
#[tauri::command]
async fn update_workflow_status(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
    status: Option<String>,
) -> Result<(), String> {
    cases_db::update_workflow_status(pool.inner(), &case_id, status.as_deref())
        .await
        .map_err(db_err)?;

    let settings = settings::read_settings().unwrap_or_default();
    if settings.feishu_enabled.unwrap_or(false) {
        match cases_db::get_case(pool.inner(), &case_id).await {
            Ok(Some(case_data)) => match feishu::sync_case(&settings, &case_data).await {
                Ok(result) if result.synced => {
                    crate::dlog!(
                        "[feishu] case={} status sync {} record={:?}",
                        case_id,
                        result.action,
                        result.record_id
                    );
                }
                Ok(result) => {
                    crate::dlog!(
                        "[feishu] case={} status sync skipped: {}",
                        case_id,
                        result.message
                    );
                }
                Err(e) => {
                    crate::dlog!("[feishu] case={} status sync failed: {}", case_id, e);
                }
            },
            Ok(None) => crate::dlog!("[feishu] case={} not found after status update", case_id),
            Err(e) => crate::dlog!("[feishu] case={} reload failed: {}", case_id, e),
        }
    }

    Ok(())
}

/// 手动把当前案件写入飞书案件池。自动状态同步失败时,详情页可用这个按钮补写/排查。
#[tauri::command]
async fn sync_case_to_feishu(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<feishu::FeishuSyncResult, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let case_data = cases_db::get_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", case_id))?;
    feishu::sync_case(&settings, &case_data).await
}

/// 2026-06-10 V0.3.7 · 同步首页日历事件到飞书日历表。
#[tauri::command]
async fn sync_feishu_calendar(
    pool: tauri::State<'_, SqlitePool>,
) -> Result<feishu::FeishuSyncResult, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let cases = cases_db::list_cases(pool.inner())
        .await
        .map_err(db_err)?;
    feishu::sync_calendar_table(&settings, &cases).await
}

/// 2026-06-10 V0.3.7 · 手动触发一次到期事项推送（设置页"测试推送"按钮用）。
#[tauri::command]
async fn test_feishu_notify(
    pool: tauri::State<'_, SqlitePool>,
) -> Result<usize, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let cases = cases_db::list_cases(pool.inner())
        .await
        .map_err(db_err)?;
    feishu::check_and_notify_expiries(&settings, &cases).await
}

/// 2026-06-10 V0.3.7 · 从飞书日历获取指定日期范围内的事件。
#[tauri::command]
async fn fetch_feishu_calendar(
    start: String,
    end: String,
) -> Result<Vec<feishu::FeishuCalendarEvent>, String> {
    feishu::fetch_calendar_events(&start, &end).await
}

/// 2026-06-10 V0.3.7 · 根据飞书日历事件标题在案件池中查找本地路径。
#[tauri::command]
async fn find_feishu_case_path(
    event_summary: String,
) -> Result<Option<String>, String> {
    let settings = settings::read_settings().unwrap_or_default();
    feishu::find_case_local_path(&settings, &event_summary).await
}

/// 2026-05-26 V0.1.13 · 写入案件 user_overrides JSON(编辑模式手改 overlay)。
///
/// `overrides_json = None` → 清空所有用户改动;
/// `overrides_json = Some(...)` → 整段覆盖。
///
/// 结构由前端 `userOverrides.ts` 定义(fields / hidden_sections / deleted_rows /
/// section_order),后端不解析,sqlite 列定义见 migration 0016。
#[tauri::command]
async fn update_case_overrides(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
    overrides_json: Option<String>,
) -> Result<(), String> {
    cases_db::update_user_overrides(pool.inner(), &case_id, overrides_json.as_deref())
        .await
        .map_err(db_err)
}

/// 2026-05-24 (T3) 重抽该案件的所有 LLM 抽取。
///
/// 用途:升级 prompt(扩字段 / 反诉视角 / is_our_side 等)后,存量
/// `documents.extracted_fields` 是旧 prompt 出的,需要让 LLM 按新 prompt 重抽一遍。
///
/// 做法:
///   1. 把该案件下所有 `extraction_status='done'` 的文档重置为 `'pending'`
///      并清掉 `extracted_fields` / `extracted_text_path`(留 cache_key)
///   2. 触发 pipeline(spawn_extraction)在后台跑,前端订阅 `extraction_progress` 看进度
///   3. 立即返回被重置的文档数,UI 用来 toast 提示"重抽中 · N 份文档"
///
/// 注意:`skipped` / `failed` 的文档**不重置**(用户可能手工跳过了某些噪音文档;
/// failed 的可能是 LLM 暂时不可用,下次重新导入会重试)。
#[tauri::command]
async fn recompute_case_extraction(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<usize, String> {
    // 1) 重置 done → pending,清抽取产物
    let res = sqlx::query(
        "UPDATE documents \
         SET extraction_status = 'pending', \
             extracted_fields = NULL, \
             extracted_text_path = NULL \
         WHERE case_id = ? AND extraction_status = 'done'",
    )
    .bind(&case_id)
    .execute(pool.inner())
    .await
    .map_err(|e| format!("重置失败: {}", e))?;
    let reset_count = res.rows_affected() as usize;

    if reset_count == 0 {
        return Ok(0); // 没什么可重抽的,直接返回
    }

    // 2) 触发 pipeline 后台跑(立即返回,前端通过 extraction_progress 事件看进度)
    let documents = documents_db::list_documents_by_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?;
    pipeline::spawn_extraction(
        app.clone(),
        pool.inner().clone(),
        case_id.clone(),
        documents,
    );

    Ok(reset_count)
}

/// 2026-05-25 V0.1.5 「🔄 刷新源文件」按钮触发。
///
/// 增量逻辑:
///   1. 找到案件,定位 `source_folder`
///   2. `scan_folder` 重扫一遍
///   3. `sync_documents_for_case` 做 diff:
///      - 全新文件 → INSERT,status=pending
///      - mtime+size 变了 → UPDATE,清抽取产物,status=pending
///      - 完全没变 → 不动
///      - 磁盘消失 → 标 `deleted_at`(软删,LLM corpus 不再带它,但 DB 留痕)
///   4. 如果有任何变化(added/updated/deleted),后台 spawn_extraction 跑 pending +
///      自动重跑 global_extract 生成新画像 + 新报告
///   5. 返回 `SyncStats` 给前端 toast 显示
///
/// 用户体验:点按钮 → 立即弹 toast「新增 X / 更新 Y / 移除 Z」→ 后台慢慢跑抽取,
/// 完成后前端通过 `extraction_progress` 事件自动刷新卡片和报告。
#[tauri::command]
async fn refresh_case_files(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<documents_db::SyncStats, String> {
    // 1) 拿案件 + source_folder
    let case = cases_db::get_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", case_id))?;

    let folder = Path::new(&case.source_folder);
    if !folder.exists() {
        return Err(format!("案件源文件夹已不存在: {}", case.source_folder));
    }
    if !folder.is_dir() {
        return Err(format!("案件源路径不是文件夹: {}", case.source_folder));
    }

    // 2) 扫文件夹(scanner 很快,同步即可)
    let scanned = scan_folder(folder);

    // 3) diff sync,拿统计
    let stats = documents_db::sync_documents_for_case(pool.inner(), &case_id, &scanned)
        .await
        .map_err(db_err)?;

    // 4) 有任何变化 或 DB 里还有 pending 文档 → 后台跑抽取(pipeline 自带重跑 global_extract)
    //
    // 2026-05-25 V0.1.8 加 pending 检测:这样老板手工把 failed 重置成 pending 后,
    // 点一下「刷新源文件」就能触发重抽,不用加新按钮。
    let pending_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents \
         WHERE case_id = ? AND deleted_at IS NULL AND extraction_status = 'pending'",
    )
    .bind(&case_id)
    .fetch_one(pool.inner())
    .await
    .unwrap_or(0);
    if stats.added > 0 || stats.updated > 0 || stats.deleted > 0 || pending_count > 0 {
        let documents = documents_db::list_documents_by_case(pool.inner(), &case_id)
            .await
            .map_err(db_err)?;
        pipeline::spawn_extraction(
            app.clone(),
            pool.inner().clone(),
            case_id.clone(),
            documents,
        );
    }

    Ok(stats)
}

// ─────────────────────────── 法院短信处理(V0.3) ───────────────────────────
// 一张网(zxfw.court.gov.cn)送达短信 → 解析 → 拉文书 → 匹配案件 → 下载进 source_folder
// → 复用刷新管线抽取上看板。纯逻辑在 court_sms 模块,这里做命令编排。

#[derive(serde::Serialize)]
struct CourtSmsDocBrief {
    name: String,
    ext: String,
}

#[derive(serde::Serialize)]
struct CourtSmsPreview {
    court: Option<String>,
    case_no: Option<String>,
    has_link: bool,
    /// 透传给 ingest(只传链接参数,**不传时效性的 wjlj**)
    link: Option<court_sms::ZxfwLink>,
    docs: Vec<CourtSmsDocBrief>,
    matched_case_id: Option<String>,
    matched_case_name: Option<String>,
    note: Option<String>,
}

#[derive(serde::Serialize)]
struct CourtSmsIngestResult {
    downloaded: Vec<String>,
    skipped: Vec<String>,
    sync: documents_db::SyncStats,
}

/// 案号归一化后比对 `agg_case_no`,返回首个匹配案件 (id, 展示名)。
async fn find_case_by_case_no(
    pool: &SqlitePool,
    case_no: &str,
) -> (Option<String>, Option<String>) {
    let target = court_sms::normalize_case_no(case_no);
    if target.is_empty() {
        return (None, None);
    }
    let cases = match cases_db::list_cases(pool).await {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    for c in cases {
        if let Some(no) = &c.agg_case_no {
            if court_sms::normalize_case_no(no) == target {
                let name = c.agg_cause.clone().unwrap_or_else(|| c.name.clone());
                return (Some(c.id), Some(name));
            }
        }
    }
    (None, None)
}

fn sanitize_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "court_document".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// 在 `folder` 下取一个不与现有文件冲突的路径:`base.ext`,占用则 `base (2).ext`…
fn unique_path(folder: &Path, base: &str, ext: &str) -> std::path::PathBuf {
    let first = folder.join(format!("{}.{}", base, ext));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let p = folder.join(format!("{} ({}).{}", base, n, ext));
        if !p.exists() {
            return p;
        }
    }
    first
}

/// 预览:解析短信 → 拉文书名列表 → 匹配在办案件。**不下载、不落盘**(无副作用)。
#[tauri::command]
async fn preview_court_sms(
    pool: tauri::State<'_, SqlitePool>,
    sms_text: String,
) -> Result<CourtSmsPreview, String> {
    let parsed = court_sms::parse_sms(&sms_text);
    let Some(link) = parsed.link.clone() else {
        return Ok(CourtSmsPreview {
            court: parsed.court,
            case_no: parsed.case_no,
            has_link: false,
            link: None,
            docs: vec![],
            matched_case_id: None,
            matched_case_name: None,
            note: Some(
                "没识别到「人民法院在线服务/一张网」(zxfw.court.gov.cn)送达链接。\
                 目前只支持一张网;其它平台(江苏微解纷等)暂不支持自动下载。"
                    .into(),
            ),
        });
    };
    let docs = court_sms::fetch_zxfw_doc_list(&link).await?;
    let (matched_id, matched_name) = match &parsed.case_no {
        Some(cn) => find_case_by_case_no(pool.inner(), cn).await,
        None => (None, None),
    };
    let docs_out: Vec<CourtSmsDocBrief> = docs
        .iter()
        .map(|d| CourtSmsDocBrief {
            name: d.name.clone(),
            ext: d.ext.clone(),
        })
        .collect();
    let note = if matched_id.is_none() {
        Some("没自动匹配到在办案件,请手动选择要归档到哪个案件。".into())
    } else {
        None
    };
    Ok(CourtSmsPreview {
        court: parsed
            .court
            .or_else(|| docs.first().and_then(|d| d.court.clone())),
        case_no: parsed.case_no,
        has_link: true,
        link: Some(link),
        docs: docs_out,
        matched_case_id: matched_id,
        matched_case_name: matched_name,
        note,
    })
}

/// 导入:重新拉新鲜文书列表(wjlj 有时效)→ 下载进案件 source_folder → 复用刷新管线抽取。
#[tauri::command]
async fn ingest_court_sms(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
    link: court_sms::ZxfwLink,
) -> Result<CourtSmsIngestResult, String> {
    let case = cases_db::get_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("案件不存在: {}", case_id))?;
    let folder = Path::new(&case.source_folder);
    if !folder.is_dir() {
        return Err(format!("案件源文件夹不可用: {}", case.source_folder));
    }
    // wjlj 有时效,这里重新拉一次拿新鲜下载地址
    let docs = court_sms::fetch_zxfw_doc_list(&link).await?;
    if docs.is_empty() {
        return Err("一张网未返回任何文书(链接可能已失效,请重新粘贴最新短信)".into());
    }
    let mut downloaded = vec![];
    let mut skipped = vec![];
    for d in &docs {
        let dest = unique_path(folder, &sanitize_filename(&d.name), &d.ext);
        match court_sms::download_doc(&d.wjlj, &dest).await {
            Ok(_) => downloaded.push(
                dest.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| d.name.clone()),
            ),
            Err(e) => {
                crate::dlog!("court_sms 下载失败 {}: {}", d.name, e);
                skipped.push(format!("{}({})", d.name, e));
            }
        }
    }
    // 复用「刷新源文件」:扫描 → diff → 新 PDF 进 documents(pending)→ 后台抽取 + 重跑画像/看板
    let scanned = scan_folder(folder);
    let sync = documents_db::sync_documents_for_case(pool.inner(), &case_id, &scanned)
        .await
        .map_err(db_err)?;
    let documents = documents_db::list_documents_by_case(pool.inner(), &case_id)
        .await
        .map_err(db_err)?;
    pipeline::spawn_extraction(
        app.clone(),
        pool.inner().clone(),
        case_id.clone(),
        documents,
    );
    Ok(CourtSmsIngestResult {
        downloaded,
        skipped,
        sync,
    })
}

/// 快递100 凭证(每次读不缓存,改了实时生效)。
fn kuaidi100_creds() -> (String, String) {
    let s = settings::read_settings().unwrap_or_default();
    (
        s.kuaidi100_customer.unwrap_or_default(),
        s.kuaidi100_key.unwrap_or_default(),
    )
}

/// 查询并跟踪一个单号:实时查 → 落本地 express_tracks.json → 返回最新全列表。
/// com=快递公司编码(ems/shunfeng…),com_name=中文名(展示),num=运单号。
#[tauri::command]
async fn query_express(
    com: String,
    com_name: String,
    num: String,
    phone: String,
) -> Result<Vec<express::TrackRecord>, String> {
    let (customer, key) = kuaidi100_creds();
    express::query_and_track(&customer, &key, &com, &com_name, &num, &phone).await
}

/// 列出本地所有跟踪记录(不联网)。
#[tauri::command]
fn list_express_tracks() -> Vec<express::TrackRecord> {
    express::load_tracks()
}

/// 刷新在跟踪的单号(未签收 + 30 天内 + 距上次轮询≥6 小时)。同单号 40 天内重查免费。
#[tauri::command]
async fn refresh_express_tracks() -> Result<Vec<express::TrackRecord>, String> {
    let (customer, key) = kuaidi100_creds();
    if customer.is_empty() || key.is_empty() {
        return Ok(express::load_tracks());
    }
    express::refresh_active(&customer, &key, 6).await
}

/// 删除一个跟踪记录。
#[tauri::command]
fn delete_express_track(num: String) -> Result<Vec<express::TrackRecord>, String> {
    express::delete_track(&num)
}

/// 数据库健康检查:返回表数量 + 数据库文件路径。
#[tauri::command]
async fn db_health(pool: tauri::State<'_, SqlitePool>) -> Result<DbHealth, String> {
    let (table_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'")
            .fetch_one(pool.inner())
            .await
            .map_err(|e| format!("查询失败: {}", e))?;

    let (case_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cases")
        .fetch_one(pool.inner())
        .await
        .map_err(|e| format!("查询案件数失败: {}", e))?;

    Ok(DbHealth {
        ok: true,
        table_count,
        case_count,
        db_path: db::default_db_path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string()),
    })
}

/// sqlx::Error → 前端友好字符串
fn db_err(e: sqlx::Error) -> String {
    format!("数据库错误: {}", e)
}

// ============================================================================
// 案件 AI 助手(case-aware chat)— 2026-05-27 V0.1.13+
// ============================================================================

/// 启动一次案件聊天:边接 SSE 边把 delta `emit` 到 `chat-stream-{message_id}` 频道。
/// 流式完成后入库一对 user/assistant 消息,长输出自动落 artifact。
#[tauri::command]
async fn case_chat(
    app: tauri::AppHandle,
    pool: tauri::State<'_, SqlitePool>,
    registry: tauri::State<'_, chat::ChatCancelRegistry>,
    input: chat::CaseChatInput,
) -> Result<chat::CaseChatResult, String> {
    chat::case_chat_impl(app, pool.inner(), registry.inner(), input).await
}

/// 取案件聊天历史(升序,前端直接渲染)。
#[tauri::command]
async fn list_chat_history(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
    limit: Option<i64>,
) -> Result<Vec<crate::db::chat::ChatMessage>, String> {
    chat::list_chat_history_impl(pool.inner(), &case_id, limit).await
}

/// 取消进行中的 chat。message_id 必须跟 case_chat 入参的 message_id 相同。
#[tauri::command]
fn cancel_chat(registry: tauri::State<'_, chat::ChatCancelRegistry>, message_id: String) -> bool {
    chat::cancel_chat_impl(registry.inner(), &message_id)
}

/// 清空某案件下全部聊天记录(用户主动)。
#[tauri::command]
async fn clear_chat_history(
    pool: tauri::State<'_, SqlitePool>,
    case_id: String,
) -> Result<u64, String> {
    chat::clear_chat_history_impl(pool.inner(), &case_id).await
}

// ============================================================================
// V0.2 D7 · 本地知识库 + 元典积分 Settings 卡片用的 5 个 commands
// ============================================================================

/// 检测本地 KB 状态:Bound / Unbound / PermissionDenied,带统计字段。
/// 前端 Settings 卡片按 status.state 切换三态 UI(7.5.A/B/C)。
#[tauri::command]
fn detect_kb_status() -> local_kb::status::KbStatus {
    let settings = settings::read_settings().unwrap_or_default();
    local_kb::status::detect_kb_status(&settings)
}

/// 在指定路径创建空 KB(已存在则只补缺失子目录,不覆盖任何已有文件)。
/// 创建成功后会自动写 settings.local_kb_root = path,local_kb_enabled = true。
#[tauri::command]
async fn create_local_kb(path: String) -> Result<local_kb::init::KbInitResult, String> {
    // tilde 展开,允许 "~/Documents/知识库" 这种
    let expanded = shellexpand::tilde(&path).into_owned();
    let target = std::path::PathBuf::from(&expanded);
    let result = local_kb::init::create_empty_kb(&target).map_err(|e| e.to_string())?;
    // 自动写回 settings,让下次 auto_detect 拿到这个路径
    let mut s = settings::read_settings().unwrap_or_default();
    s.local_kb_root = Some(path);
    s.local_kb_enabled = Some(true);
    settings::write_settings(&s).map_err(|e| format!("写 settings 失败: {}", e))?;
    Ok(result)
}

/// 启动兜底:老版本(1.x,无本地 KB 功能)升级用户的 settings 里没有 `local_kb_root` →
/// `LocalKb::auto_detect` 返回 None → 找到的法规/案例不写回 KB、本地命中省积分全失效。
/// 这里在默认路径 `~/Documents/知识库` 创建(已存在则只补目录不覆盖)+ 写回 settings,
/// 让所有用户(含老用户、新装用户)开箱即用「越用越省钱」。
/// 幂等:只在 `local_kb_root` 为空且用户没显式禁用(`local_kb_enabled != Some(false)`)时动作;
/// 返回 `Some(展示路径)` 表示本次新配置了 KB(用于前端提示),`None` = 已配置过 / 已禁用 / 失败。
/// 失败(权限 / macOS Documents TCC 拒绝等)非致命,只 dlog,不阻断启动 —— 下次启动会重试,
/// 用户也可在设置里手动新建。
fn ensure_default_local_kb() -> Option<String> {
    let mut s = settings::read_settings().ok()?;
    // 用户显式停用 → 尊重选择
    if s.local_kb_enabled == Some(false) {
        return None;
    }
    // 已配置过路径(新装走 onboarding 或老用户已手动建过)→ 不动
    let has_root = s
        .local_kb_root
        .as_deref()
        .map(str::trim)
        .map(|r| !r.is_empty())
        .unwrap_or(false);
    if has_root {
        return None;
    }
    const DEFAULT_KB: &str = "~/Documents/知识库";
    let expanded = shellexpand::tilde(DEFAULT_KB).into_owned();
    let target = std::path::PathBuf::from(&expanded);
    match local_kb::init::create_empty_kb(&target) {
        Ok(r) => {
            s.local_kb_root = Some(DEFAULT_KB.to_string());
            s.local_kb_enabled = Some(true);
            if let Err(e) = settings::write_settings(&s) {
                crate::dlog!("[startup] 写 KB settings 失败(非致命): {}", e);
                return None;
            }
            crate::dlog!(
                "[startup] 自动配置本地知识库 at {:?} (reused={})",
                r.path,
                r.reused_existing
            );
            Some(DEFAULT_KB.to_string())
        }
        Err(e) => {
            crate::dlog!("[startup] 自动创建本地知识库失败(非致命): {}", e);
            None
        }
    }
}

/// 从 zip 导入资料包合并进当前 KB。`on_conflict` 是 "skip" / "overwrite_older" / "always_overwrite"。
#[tauri::command]
async fn import_kb_from_zip(
    zip_path: String,
    on_conflict: String,
) -> Result<local_kb::share::ImportResult, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let kb = local_kb::cache::LocalKb::auto_detect(&settings)
        .ok_or_else(|| "本地知识库未启用,无法导入".to_string())?;
    let strategy = match on_conflict.as_str() {
        "skip" => local_kb::share::ConflictStrategy::Skip,
        "overwrite_older" => local_kb::share::ConflictStrategy::OverwriteOlder,
        "always_overwrite" => local_kb::share::ConflictStrategy::AlwaysOverwrite,
        _ => return Err(format!("未知冲突策略: {}", on_conflict)),
    };
    local_kb::share::import_from_zip(
        &kb,
        local_kb::share::ImportOptions {
            zip_path: std::path::PathBuf::from(zip_path),
            on_conflict: strategy,
        },
    )
    .map_err(|e| e.to_string())
}

/// 把当前 KB 的元典缓存打包成 zip 导出。
#[tauri::command]
async fn export_kb_to_zip(output_path: String) -> Result<local_kb::share::ExportResult, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let kb = local_kb::cache::LocalKb::auto_detect(&settings)
        .ok_or_else(|| "本地知识库未启用,无法导出".to_string())?;
    local_kb::share::export_to_zip(
        &kb,
        local_kb::share::ExportOptions {
            yuandian_cache_only: true,
            output_path: std::path::PathBuf::from(output_path),
            include_readme: true,
            exporter_version: env!("CARGO_PKG_VERSION").to_string(),
        },
    )
    .map_err(|e| e.to_string())
}

/// P2 · 清理「搜索 / 向量类、且超 `max_age_days` 天」的元典缓存(.md + .raw.json + index 条目)。
/// **只清检索列表**,法规 / 法条 / 案例**全文详情**与企业类一律不动(是复用资产)。
/// 显式触发(Settings 维护按钮),绝不自动跑(§7 删除需确认)。
#[tauri::command]
async fn prune_yuandian_cache(max_age_days: u32) -> Result<local_kb::cache::PruneStats, String> {
    let settings = settings::read_settings().unwrap_or_default();
    let kb = local_kb::cache::LocalKb::auto_detect(&settings)
        .ok_or_else(|| "本地知识库未启用,无法清理".to_string())?;
    kb.prune_stale(max_age_days).map_err(|e| e.to_string())
}

/// 取当前月份元典积分账。给 Settings 元典积分卡显示「本月已用 / 上限 / KB 节省」。
#[tauri::command]
async fn get_yuandian_monthly_stats(
    pool: tauri::State<'_, SqlitePool>,
) -> Result<db::credits::MonthlyCredits, String> {
    let ym = db::credits::current_year_month();
    db::credits::get_monthly_stats(pool.inner(), &ym)
        .await
        .map_err(db_err)
}

/// 取元典积分账总览(当月 + 上月 + 累计)。当月跨月归 0 时,前端用上月/累计补显示,避免误以为数据丢了。
#[tauri::command]
async fn get_yuandian_credits_overview(
    pool: tauri::State<'_, SqlitePool>,
) -> Result<db::credits::CreditsOverview, String> {
    let ym = db::credits::current_year_month();
    db::credits::get_overview(pool.inner(), &ym)
        .await
        .map_err(db_err)
}

/// 验证 embedding 配置:embed 一个探针词,成功返回向量维度。给设置页「验证」按钮。
#[tauri::command]
async fn verify_embedding_key(
    endpoint: String,
    model: String,
    api_key: String,
) -> Result<usize, String> {
    embedding::verify(&endpoint, &model, &api_key).await
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // run() 是应用入口,按惯例放文件末尾
mod tests {
    use super::*;
    use std::io::Write;

    /// 回归:一个文件夹**先作为单案导入过**,再拆分导入,必须先删旧整体案、不撞 source_path 唯一索引。
    /// (单元测试用合成目录树 + :memory: 真库,覆盖 commit 的写库层 —— advisor 抓的阻断点。)
    #[tokio::test]
    async fn split_reimport_replaces_old_root_no_unique_conflict() {
        use crate::db::init_pool;

        let pool = init_pool(":memory:").await.unwrap();
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        for rel in [
            "01_原告与共用证据/身份证.pdf",
            "02_案件A/01_诉讼文书/起诉状.pdf",
            "03_案件B/01_诉讼文书/起诉状.pdf",
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }
        let root_str = root.to_string_lossy().to_string();

        // 1) 模拟「整体作为单个案件导入过」:旧案占用了所有 source_path
        let old = cases_db::upsert_case_for_folder(&pool, &root_str, "张三", "诉讼")
            .await
            .unwrap();
        documents_db::replace_documents_for_case(&pool, &old.id, &scan_folder(root))
            .await
            .unwrap();

        // 2) 检测 → 拆分导入(root_already_imported 场景)
        let plan = case_split::plan_folder(root);
        assert!(plan.multi && plan.cases.len() == 2, "应检测为 2 案");
        let cases: Vec<CommitCase> = plan
            .cases
            .iter()
            .map(|c| CommitCase {
                dir: c.dir.clone(),
                name: c.suggested_name.clone(),
            })
            .collect();
        let results = build_split_cases(&pool, &root_str, &cases, &plan.shared_dirs)
            .await
            .expect("拆分导入不应失败(应先删旧 root 案释放 source_path)");

        assert_eq!(results.len(), 2, "应建 2 个案件");
        // 旧整体案件已被替换删除
        assert!(
            cases_db::get_case(&pool, &old.id).await.unwrap().is_none(),
            "旧整体案件应被删除替换"
        );
        // 每个新案件都有文档
        for r in &results {
            assert!(!r.docs.is_empty(), "新案件应有文档");
        }
        // Phase 2(migration 0019):共用材料(身份证)挂到**每个**案件
        for r in &results {
            assert!(
                r.docs.iter().any(|d| d.filename.contains("身份证")),
                "每个案件都应含共用材料(Phase 2 一文件多案)"
            );
        }
    }

    /// 端到端测试:写一个临时 .txt 文件,用 textutil 抽取应能读到内容。
    /// (textutil 是 macOS 自带的,CI 跑 macos-latest 没问题)
    #[test]
    fn extract_doc_text_handles_txt() {
        let dir = std::env::temp_dir().join("caseboard_extract_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_note.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "民事诉状").unwrap();
            writeln!(f, "原告:张三").unwrap();
            writeln!(f, "被告:李四").unwrap();
            writeln!(f, "诉讼请求:返还借款 10000 元").unwrap();
        }
        let result = extract_doc_text(path.to_string_lossy().to_string());
        assert!(result.is_ok(), "textutil 应该能读 .txt: {:?}", result.err());
        let text = result.unwrap();
        assert!(text.contains("民事诉状"));
        assert!(text.contains("张三"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_doc_text_rejects_missing_file() {
        let result = extract_doc_text("/nonexistent/path.docx".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不存在"));
    }

    /// 插一个 documents 行 + 落一个真实 .md 文件,返回 doc_id。
    async fn seed_doc(
        pool: &SqlitePool,
        case_id: &str,
        md_path: &std::path::Path,
        is_ai_artifact: i64,
        source: &str,
        initial: &str,
    ) -> String {
        std::fs::write(md_path, initial).unwrap();
        let doc_id = uuid::Uuid::new_v4().to_string();
        let path_str = md_path.to_string_lossy().to_string();
        sqlx::query(
            "INSERT INTO documents \
             (id, case_id, source_path, filename, category, is_ai_artifact, mime_type, \
              size_bytes, extraction_status, source, created_at) \
             VALUES (?, ?, ?, ?, '民事起诉状', ?, 'text/markdown', ?, 'done', ?, datetime('now'))",
        )
        .bind(&doc_id)
        .bind(case_id)
        .bind(&path_str)
        .bind("起诉状_test.md")
        .bind(is_ai_artifact)
        .bind(initial.len() as i64)
        .bind(source)
        .execute(pool)
        .await
        .unwrap();
        doc_id
    }

    #[tokio::test]
    async fn write_editor_doc_rewrites_in_place_and_rebuilds_header() {
        use crate::db::cases::{create_case, NewCase};
        use crate::db::init_pool;

        let tmp = tempfile::tempdir().unwrap();
        let pool = init_pool(":memory:").await.unwrap();
        let case = create_case(
            &pool,
            NewCase {
                name: "editor test".into(),
                case_type: "诉讼".into(),
                source_folder: "/tmp/editor".into(),
            },
        )
        .await
        .unwrap();

        let md_path = tmp.path().join("起诉状_test.md");
        let doc_id = seed_doc(
            &pool,
            &case.id,
            &md_path,
            1,
            "chat_artifact",
            "<!-- filing · doc_type=民事起诉状 · title=旧标题 · ts=2026-05-31T00:00:00Z -->\n\n# 一、诉讼请求\n\n旧正文",
        )
        .await;

        let new_body = "# 一、诉讼请求\n\n新正文,内容已改。";
        let returned = write_editor_doc(&pool, &doc_id, "新标题", new_body)
            .await
            .unwrap();
        assert_eq!(returned, doc_id, "返回同一 doc_id(原地覆盖)");

        // 文件原地覆盖:新正文 + 重建的头(新标题 + doc_type 来自 category)
        let written = std::fs::read_to_string(&md_path).unwrap();
        assert!(written.contains("新正文,内容已改。"), "应写入新正文");
        assert!(written.contains("title=新标题"), "头应重建成新标题");
        assert!(
            written.contains("doc_type=民事起诉状"),
            "doc_type 从 category 重建"
        );
        assert!(!written.contains("旧正文"), "旧正文应被覆盖");
        // 头在最前,正文在后(filing 格式)
        assert!(written.starts_with("<!-- filing"), "头必须在文件最前");

        // size_bytes 更新为新文件字节数
        let size: i64 = sqlx::query_scalar("SELECT size_bytes FROM documents WHERE id = ?")
            .bind(&doc_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            size as usize,
            written.len(),
            "size_bytes 应等于新文件字节数"
        );
    }

    #[tokio::test]
    async fn write_editor_doc_refuses_original_file() {
        use crate::db::cases::{create_case, NewCase};
        use crate::db::init_pool;

        let tmp = tempfile::tempdir().unwrap();
        let pool = init_pool(":memory:").await.unwrap();
        let case = create_case(
            &pool,
            NewCase {
                name: "editor test".into(),
                case_type: "诉讼".into(),
                source_folder: "/tmp/editor".into(),
            },
        )
        .await
        .unwrap();

        // is_ai_artifact=0 的导入原始文件:绝不允许被编辑覆盖
        let md_path = tmp.path().join("原始诉状.md");
        let doc_id = seed_doc(&pool, &case.id, &md_path, 0, "scan", "原始内容").await;

        let err = write_editor_doc(&pool, &doc_id, "x", "篡改内容")
            .await
            .unwrap_err();
        assert!(err.contains("原始文件"), "应拒绝编辑原始文件: {}", err);
        // 原文件不被改动
        let after = std::fs::read_to_string(&md_path).unwrap();
        assert_eq!(after, "原始内容", "原始文件内容不能被覆盖");
    }

    #[tokio::test]
    async fn write_editor_doc_sanitizes_title_for_comment() {
        use crate::db::cases::{create_case, NewCase};
        use crate::db::init_pool;

        let tmp = tempfile::tempdir().unwrap();
        let pool = init_pool(":memory:").await.unwrap();
        let case = create_case(
            &pool,
            NewCase {
                name: "editor test".into(),
                case_type: "诉讼".into(),
                source_folder: "/tmp/editor".into(),
            },
        )
        .await
        .unwrap();
        let md_path = tmp.path().join("起诉状_test.md");
        let doc_id = seed_doc(&pool, &case.id, &md_path, 1, "chat_artifact", "x").await;

        // 标题含 `-->` 和换行,应被安全化,不破坏注释结构
        write_editor_doc(&pool, &doc_id, "标题-->注入\n第二行", "正文")
            .await
            .unwrap();
        let written = std::fs::read_to_string(&md_path).unwrap();
        // 注释头闭合只有一处(`-->` 被替换 + 换行被替换成空格)
        assert_eq!(
            written.matches("-->").count(),
            1,
            "注释头不能被标题里的 --> 破坏"
        );
        assert!(written.starts_with("<!-- filing"));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 2026-05-26 V0.1.11:启动早期装 panic hook,把 panic 信息落到 diagnostic_log
    // ring buffer,反馈通道带出来。
    diagnostic_log::install_panic_hook();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // 启动时同步初始化数据库连接池 + 跑 migrations
            // 用 tauri::async_runtime::block_on 避免前端在 pool 就绪前发命令
            let db_path = db::default_db_path()
                .map_err(|e| Box::<dyn std::error::Error>::from(format!("找不到数据目录: {}", e)))?
                .to_string_lossy()
                .to_string();
            let pool = tauri::async_runtime::block_on(db::init_pool(&db_path)).map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("初始化数据库失败: {}", e))
            })?;

            // V0.2 D5.5 · 启动时把上次崩溃前没收尾的 chat_tasks 标 failed,
            // 让前端展示「重试」按钮。阈值 5 分钟,跟实施计划 § 6.9 对齐。
            // 用 spawn 异步跑(不阻塞 setup),失败也不阻止启动 — 内部已经走 dlog。
            {
                let pool_for_resume = pool.clone();
                tauri::async_runtime::spawn(async move {
                    let n =
                        db::chat_tasks::resume_orphaned_chat_tasks(&pool_for_resume, 5 * 60).await;
                    if n > 0 {
                        crate::dlog!("[startup] resume_orphaned_chat_tasks 标记 {} 个 orphan", n);
                    }
                });
            }

            app.manage(pool);
            // chat 模块全局 cancel 注册表(V0.1.13+)
            app.manage(chat::ChatCancelRegistry::default());
            // 匿名使用遥测(只在编译期注入了 key 的 release 构建启用;dev/test 静默)。
            // fire-and-forget,失败不影响启动。
            telemetry::start();

            // 2026-06-10 V0.3.7 · 飞书到期事项定时推送。
            // 每 6 小时检查一次，首次启动延迟 30 秒后执行一次。
            // fire-and-forget，失败不影响主功能。
            {
                let pool = app.state::<SqlitePool>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    // 首次延迟 30 秒
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    loop {
                        let settings = settings::read_settings().unwrap_or_default();
                        if settings.feishu_notify_enabled.unwrap_or(false) {
                            match cases_db::list_cases(&pool).await {
                                Ok(cases) => {
                                    match feishu::check_and_notify_expiries(&settings, &cases).await {
                                        Ok(n) if n > 0 => crate::dlog!("[notify] sent {} expiry notifications", n),
                                        Err(e) => crate::dlog!("[notify] check failed: {}", e),
                                        _ => {}
                                    }
                                }
                                Err(e) => crate::dlog!("[notify] list_cases failed: {}", e),
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                    }
                });
            }

            // V0.3:老版本(1.x)升级 / 新装用户兜底自动创建本地知识库,让「越用越省钱」
            // (法规/案例自动写回 + 本地命中)开箱即用。独立线程跑(含 FS IO + 可能的 macOS
            // Documents TCC 提示),非阻塞、非致命;新配置成功后 emit 事件让前端弹一次提示。
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    if let Some(path) = ensure_default_local_kb() {
                        // 等前端挂好事件监听(启动期 webview 仍在加载,emit 太早会丢)再提示。
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        let _ = app_handle.emit("local-kb-auto-created", path);
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_case_folder,
            import_case_folder,
            plan_import_folder,
            commit_import_folder,
            list_cases,
            get_case_with_docs,
            list_case_logs,
            add_case_log,
            delete_case_log,
            export_case_os_input,
            delete_case,
            read_text_file,
            extract_doc_text,
            extract_fields_from_text,
            open_in_default_app,
            open_url,
            reveal_in_finder,
            get_settings,
            save_settings,
            update_home_case_order,
            detect_local_readiness,
            ensure_local_ready,
            db_health,
            reaggregate_all_cases,
            global_extract_case,
            yuandian_basic_query,
            yuandian_deep_dive,
            add_payment,
            list_payments,
            delete_payment,
            delete_document,
            reextract_document,
            export_report_html,
            export_report_docx,
            recompute_case_extraction,
            refresh_case_files,
            preview_court_sms,
            ingest_court_sms,
            query_express,
            list_express_tracks,
            refresh_express_tracks,
            delete_express_track,
            update_workflow_status,
            sync_case_to_feishu,
            sync_feishu_calendar,
            test_feishu_notify,
            fetch_feishu_calendar,
            find_feishu_case_path,
            update_case_overrides,
            get_deepseek_balance,
            collect_feedback_diagnostic,
            save_feedback_md,
            send_feedback_email,
            verify_mineru_key,
            verify_deepseek_key,
            verify_cloud_llm_key,
            verify_yuandian_key,
            check_for_update,
            app_version,
            seed_demo_case_if_empty,
            yuandian_full_report,
            export_md_html,
            export_md_docx,
            export_filing_docx,
            save_editor_doc,
            case_chat,
            list_chat_history,
            cancel_chat,
            clear_chat_history,
            // V0.2 D7 · 本地知识库 + 元典积分
            detect_kb_status,
            create_local_kb,
            import_kb_from_zip,
            export_kb_to_zip,
            prune_yuandian_cache,
            get_yuandian_monthly_stats,
            get_yuandian_credits_overview,
            verify_embedding_key,
        ])
        .on_window_event(|_window, event| {
            // App 退出时清理子进程(llama-server)
            if let tauri::WindowEvent::Destroyed = event {
                lifecycle::shutdown();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
