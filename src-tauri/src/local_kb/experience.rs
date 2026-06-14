//! 办案经验卡片入本地知识库(项目1:结案案件沉淀)。
//!
//! 写到 `<local_kb_root>/raw/cases-experience/`,`search_local_kb` 整库检索默认就能搜到
//! (0 积分复用),新案办理时 AI 助手可调出同类经验。**不脱敏**(作者本人案件、本机自用)。

use std::path::PathBuf;

use super::KbError;
use crate::settings::Settings;

/// 经验卡片子目录(相对 local_kb_root)。
pub const EXPERIENCE_SUBDIR: &str = "raw/cases-experience";

/// 把一张办案经验卡片(已是 Markdown)写入本地知识库,返回写入文件的绝对路径。
/// 要求 `settings.local_kb_enabled` 且 `local_kb_root` 是已存在目录。
pub fn save_case_experience(
    settings: &Settings,
    case_id: &str,
    case_name: &str,
    markdown: &str,
) -> Result<PathBuf, KbError> {
    let root = settings
        .local_kb_root
        .as_deref()
        .filter(|_| settings.local_kb_enabled == Some(true))
        .map(PathBuf::from)
        .ok_or_else(|| KbError::NoPath(PathBuf::from("(未配置或未启用本地知识库)")))?;
    if !root.is_dir() {
        return Err(KbError::NoPath(root));
    }
    let dir = root.join(EXPERIENCE_SUBDIR);
    std::fs::create_dir_all(&dir)?;
    let fname = format!("{}_{}.md", sanitize(case_id), sanitize(case_name));
    let path = dir.join(fname);
    std::fs::write(&path, markdown)?;
    Ok(path)
}

/// 文件名安全化:去路径分隔符 / 控制字符,截断到 60 字符。
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    cleaned.trim().trim_matches('.').chars().take(60).collect()
}
