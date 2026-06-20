//! 本机服务 lifecycle 管理。
//!
//! 2026-05-23 晚六 作者需求(详见 docs/产品决策与理念.md 第 2.6 节):
//! - 用户不应该自己开终端跑 `llama-server`,App 后台自动拉起
//! - 用户不应该自己手敲 `brew install` 装依赖,App 后台帮忙
//! - 用户不应该自己手动下载 1.8GB 模型,App 提供"一键下载"
//! - 前端只显示运行状态,不暴露技术细节
//!
//! V0.1 实现:
//! - [x] detect_local_models:检测本地模型文件是否存在
//! - [x] ping_llama_server:检测 :8899 是否就绪
//! - [x] auto_start_llama_server:后台 spawn llama-server 子进程
//! - [x] wait_until_ready:等服务就绪(轮询 /v1/models)
//! - [x] ensure_local_ready:总入口,需要本机时一站式准备好
//!
//! V0.1.1 实现:
//! - [ ] download_models:从国内云盘 / Hugging Face 下载
//! - [ ] install_llama_cpp:自动 brew install llama.cpp
//! - [ ] SHA-256 校验

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// 本机模型必备的两个文件
pub const MAIN_MODEL_FILENAME: &str = "MiniCPM-V-4_6-Q8_0.gguf";
pub const MMPROJ_FILENAME: &str = "mmproj-model-f16.gguf";

/// llama-server 默认监听
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8899;
/// llama-server 启动 + 模型加载的最大等待时间(秒)
const SERVER_READY_TIMEOUT_SEC: u64 = 90;

/// 全局子进程引用,App 退出时清理。
/// 类型是 Option<std::process::Child>,因为只在 App lifecycle 维护一份。
static LLAMA_SERVER_CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);

#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalReadiness {
    pub model_dir: Option<String>,
    pub has_main_model: bool,
    pub has_mmproj: bool,
    pub llama_cpp_installed: bool,
    pub server_running: bool,
    pub server_endpoint: String,
}

/// 检测本地模型 + llama-server 状态(给 onboarding 用)
pub fn detect_local_readiness(model_dir: Option<&str>) -> LocalReadiness {
    let dir = resolve_model_dir(model_dir);
    let main_path = dir.as_ref().map(|d| d.join(MAIN_MODEL_FILENAME));
    let mmproj_path = dir.as_ref().map(|d| d.join(MMPROJ_FILENAME));

    LocalReadiness {
        model_dir: dir.as_ref().map(|d| d.display().to_string()),
        has_main_model: main_path.as_ref().is_some_and(|p| p.exists()),
        has_mmproj: mmproj_path.as_ref().is_some_and(|p| p.exists()),
        llama_cpp_installed: which_llama_server().is_some(),
        server_running: ping_llama_server_blocking(),
        server_endpoint: format!("http://{}:{}", DEFAULT_HOST, DEFAULT_PORT),
    }
}

/// 找用户的本机模型目录。优先级:
/// 1. 用户在 settings 显式指定的
/// 2. 作者的 LM Studio 路径(~/.lmstudio/models/openbmb/MiniCPM-V-4.6-gguf/)
/// 3. App 默认下载目录(~/.cache/caseboard/models/)
fn resolve_model_dir(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    // 作者的 LM Studio 默认位置(国内 LLM 用户常见)
    if let Some(home) = directories::UserDirs::new() {
        let lmstudio = home
            .home_dir()
            .join(".lmstudio/models/openbmb/MiniCPM-V-4.6-gguf");
        if lmstudio.exists() {
            return Some(lmstudio);
        }
    }
    // App 默认目录
    if let Some(proj) = directories::ProjectDirs::from("io", "caseboard", "CaseBoard") {
        let dir = proj.cache_dir().join("models");
        if dir.exists() {
            return Some(dir);
        }
    }
    None
}

/// 找 llama-server 可执行文件路径
fn which_llama_server() -> Option<PathBuf> {
    // PATH 里找
    for p in std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = p.join("llama-server");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // brew 默认路径兜底
    let brew = PathBuf::from("/opt/homebrew/bin/llama-server");
    if brew.exists() {
        return Some(brew);
    }
    None
}

/// 同步 ping `:8899/v1/models`
fn ping_llama_server_blocking() -> bool {
    let url = format!("http://{}:{}/v1/models", DEFAULT_HOST, DEFAULT_PORT);
    ureq::get(&url)
        .timeout(Duration::from_secs(2))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

/// 启动 llama-server 子进程(spawn 后立即返回,后续需调 wait_until_ready)
pub fn spawn_llama_server(model_dir: &Path) -> Result<(), String> {
    // 已经在跑就跳
    if ping_llama_server_blocking() {
        return Ok(());
    }

    let bin = which_llama_server()
        .ok_or_else(|| "llama-server 没装 — 请先运行 `brew install llama.cpp`".to_string())?;
    let main = model_dir.join(MAIN_MODEL_FILENAME);
    let mmproj = model_dir.join(MMPROJ_FILENAME);
    if !main.exists() {
        return Err(format!("主模型缺失: {}", main.display()));
    }
    if !mmproj.exists() {
        return Err(format!("视觉投影器缺失: {}", mmproj.display()));
    }

    // 日志目录
    let log_dir = directories::ProjectDirs::from("io", "caseboard", "CaseBoard")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&log_dir).ok();
    let log_path = log_dir.join("llama-server.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("无法打开 llama-server 日志: {}", e))?;
    let stderr_file = log_file.try_clone().map_err(|e| e.to_string())?;

    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("-m")
        .arg(&main)
        .arg("--mmproj")
        .arg(&mmproj)
        .arg("--host")
        .arg(DEFAULT_HOST)
        .arg("--port")
        .arg(DEFAULT_PORT.to_string())
        .arg("-c")
        .arg("8192")
        .arg("-ngl")
        .arg("999")
        .stdout(log_file)
        .stderr(stderr_file);
    crate::proc_util::hide_console_window_std(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("启动 llama-server 失败: {}", e))?;

    crate::dlog!(
        "[lifecycle] spawned llama-server pid={}, log={}",
        child.id(),
        log_path.display()
    );

    *LLAMA_SERVER_CHILD.lock().unwrap() = Some(child);
    Ok(())
}

/// 等 llama-server 加载完模型就绪
pub async fn wait_until_ready() -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(SERVER_READY_TIMEOUT_SEC);
    let mut tried = 0u32;
    while std::time::Instant::now() < deadline {
        if ping_llama_server_blocking() {
            return Ok(());
        }
        tried += 1;
        if tried.is_multiple_of(5) {
            crate::dlog!(
                "[lifecycle] waiting for llama-server :8899... ({}s)",
                tried * 2
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(format!(
        "等了 {}s,llama-server :8899 还没就绪。检查 ~/.local/share/CaseBoard/llama-server.log",
        SERVER_READY_TIMEOUT_SEC
    ))
}

/// 用户需要本机服务时的一站式入口:检测/启动/等就绪。
/// pipeline 在跑抽取前调一次,失败抛错给前端。
pub async fn ensure_local_ready(model_dir_hint: Option<&str>) -> Result<(), String> {
    // 1. 已经在跑就 OK
    if ping_llama_server_blocking() {
        return Ok(());
    }

    // 2. 找模型目录
    let dir = resolve_model_dir(model_dir_hint)
        .ok_or_else(|| "找不到本机模型目录 — 请先去 Settings 配置或下载模型".to_string())?;

    // 3. spawn
    spawn_llama_server(&dir)?;

    // 4. 等就绪
    wait_until_ready().await
}

/// App 退出时清理子进程
pub fn shutdown() {
    if let Some(mut child) = LLAMA_SERVER_CHILD.lock().unwrap().take() {
        crate::dlog!("[lifecycle] killing llama-server pid={}", child.id());
        let _ = child.kill();
        let _ = child.wait();
    }
}
