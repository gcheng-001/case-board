//! 版本检测 —— 启动时 / 手动触发,fetch lawtools.top 仓库的 version.json 比对当前版本。
//!
//! 2026-05-25 V0.1.8 加。
//!
//! 设计:
//!   - 数据源:`https://lawtools.top/version.json`(lawtools.top 的 OSS 公开域名)
//!     2026-05-25 V0.1.8 hotfix:原来用 raw.githubusercontent.com,但 lawtools.top 仓库
//!     是 PRIVATE,raw 永远 404。改用 OSS 公开域名,无需认证。
//!   - 当前版本:`env!("CARGO_PKG_VERSION")`,跟 Cargo.toml 一致
//!   - 比对:语义化版本(major.minor.patch),远程严格大于本地才算落后
//!   - 超时:8s。失败不报错,返回 `has_update=false` + error 字段给前端日志用
//!
//! 作者明确要求(2026-05-25):**不强制更新**,只提示。用户可点「取消」。

use serde::{Deserialize, Serialize};

const VERSION_JSON_URL: &str = "https://lawtools.top/version.json";
const FETCH_TIMEOUT_SEC: u64 = 8;

/// 远程 version.json 反序列化结构
#[derive(Debug, Clone, Deserialize)]
struct RemoteVersion {
    version: String,
    #[serde(default)]
    released_at: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
}

/// 给前端的检测结果(序列化为 JSON)
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    /// 当前本机版本(Cargo.toml)
    pub current: String,
    /// 远程最新版本(失败时 None)
    pub latest: Option<String>,
    /// 是否落后(latest > current 才 true)
    pub has_update: bool,
    /// 发布日期(YYYY-MM-DD)
    pub released_at: Option<String>,
    /// 更新说明(Markdown / 纯文本均可,前端按纯文本渲染避免 XSS)
    pub notes: Option<String>,
    /// 下载页 URL(用户点「去下载」开浏览器去这里)
    pub download_url: Option<String>,
    /// 检测失败时的错误描述(成功为 None)。前端只在调试时显示。
    pub error: Option<String>,
}

impl UpdateInfo {
    fn fail(current: &str, msg: impl Into<String>) -> Self {
        Self {
            current: current.to_string(),
            latest: None,
            has_update: false,
            released_at: None,
            notes: None,
            download_url: None,
            error: Some(msg.into()),
        }
    }
}

/// 检测远程最新版本。
pub async fn check_for_update() -> UpdateInfo {
    let current = env!("CARGO_PKG_VERSION").to_string();

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SEC))
        .build()
    {
        Ok(c) => c,
        Err(e) => return UpdateInfo::fail(&current, format!("HTTP 客户端创建失败: {}", e)),
    };

    let resp = match client
        .get(VERSION_JSON_URL)
        .header("Accept", "application/json")
        .header("User-Agent", format!("CaseBoard/{}", current))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return UpdateInfo::fail(&current, format!("拉取 version.json 失败: {}", e)),
    };

    if !resp.status().is_success() {
        return UpdateInfo::fail(&current, format!("HTTP {}", resp.status().as_u16()));
    }

    let remote: RemoteVersion = match resp.json().await {
        Ok(v) => v,
        Err(e) => return UpdateInfo::fail(&current, format!("解析 version.json 失败: {}", e)),
    };

    let has_update = is_strictly_newer(&remote.version, &current);

    UpdateInfo {
        current,
        latest: Some(remote.version),
        has_update,
        released_at: remote.released_at,
        notes: remote.notes,
        download_url: remote
            .download_url
            .or_else(|| Some("https://lawtools.top/".to_string())),
        error: None,
    }
}

/// 比较两个语义化版本 — 远程严格大于本地才返回 true。
///
/// 容忍格式:`major.minor.patch`(三段)或 `major.minor`(两段,补 0)。
/// 解析失败的段当 0。
fn is_strictly_newer(remote: &str, current: &str) -> bool {
    let r = parse_version(remote);
    let c = parse_version(current);
    r > c
}

fn parse_version(s: &str) -> (u32, u32, u32) {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.').map(|p| {
        // 去掉 pre-release 后缀(如 1.0.0-beta1 → 1, 0, 0)
        let num: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        num.parse::<u32>().unwrap_or(0)
    });
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    (major, minor, patch)
}
