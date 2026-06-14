//! 新建空 KB 目录结构 + 已存在 KB 的子目录补齐(只补不覆盖)。
//!
//! 跟 `legal-kb` skill 的主库结构对齐(详 § 6.7-bis):
//!   `raw/` + `raw/notes/` + `raw/companies/` + `raw/yuandian-cache/`
//!   + `wiki/` + `wiki/sources/` + `wiki/topics/` + `wiki/index.md` + `gap-log.md`

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::Serialize;

use super::KbError;

// SAFETY: PathBuf 在 KbInitResult struct 字段里用了,clippy 误判为 unused 可以忽略。
// (这条 use 也确实保留 PathBuf 让 struct 字段语义清楚)

/// 创建新空 KB 时给前端的回执。
#[derive(Debug, Clone, Serialize)]
pub struct KbInitResult {
    pub created_at: DateTime<Local>,
    pub path: PathBuf,
    pub files_created: u32,
    pub dirs_created: u32,
    /// `true` = 已存在,本次只补缺失子目录;`false` = 全新创建
    pub reused_existing: bool,
}

const SUBDIRS: &[&str] = &[
    "raw",
    "raw/notes",
    "raw/companies",
    "raw/yuandian-cache",
    "raw/cases-experience",
    "wiki",
    "wiki/sources",
    "wiki/topics",
];

const WELCOME_MD: &str = "# 法律知识库\n\n\
这是 CaseBoard 为你创建的空知识库。\n\n\
## 目录说明\n\
- `raw/notes/` — 你手动整理的原始笔记\n\
- `raw/companies/` — 企业档案\n\
- `raw/yuandian-cache/` — **CaseBoard / Claude Code 自动写入的元典缓存**(不建议手动改)\n\
- `raw/cases-experience/` — **CaseBoard 结案案件沉淀的办案经验卡片**(可被 search_local_kb 检索复用)\n\
- `wiki/sources/` — 你整理过的来源页(由 Claude Code + legal-kb skill 治理)\n\
- `wiki/topics/` — 专题页\n\n\
## 长期使用建议\n\
- 用 CaseBoard 跑案件 chat,法规/案例自动写入 `raw/yuandian-cache/`\n\
- 用 Claude Code + legal-kb skill 把重要内容升级到 `wiki/sources/`\n\
- 同事可以通过 CaseBoard 导出/导入资料包共享 `yuandian-cache/`\n";

const GAP_LOG_MD: &str = "# 缺口清单\n\n(暂无)\n";

/// 在指定路径创建空 KB 目录结构。已存在则走 [`reconcile_existing`](见同文件)。
pub fn create_empty_kb(target: &Path) -> Result<KbInitResult, KbError> {
    if target.exists() {
        return reconcile_existing(target);
    }
    std::fs::create_dir_all(target)?;
    let mut dirs_created = 1u32;
    for sub in SUBDIRS {
        let p = target.join(sub);
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
            dirs_created += 1;
        }
    }
    let mut files_created = 0u32;
    let wiki_index = target.join("wiki").join("index.md");
    if !wiki_index.exists() {
        std::fs::write(&wiki_index, WELCOME_MD)?;
        files_created += 1;
    }
    let gap_log = target.join("gap-log.md");
    if !gap_log.exists() {
        std::fs::write(&gap_log, GAP_LOG_MD)?;
        files_created += 1;
    }
    Ok(KbInitResult {
        created_at: Local::now(),
        path: target.to_path_buf(),
        files_created,
        dirs_created,
        reused_existing: false,
    })
}

/// 已存在路径:**只补缺失的子目录,绝不覆盖任何已有文件**。
/// 若用户选了一个已有 KB(或一个完全无关的目录),都走这条 — 补全到结构齐备即可。
pub fn reconcile_existing(target: &Path) -> Result<KbInitResult, KbError> {
    if !target.is_dir() {
        return Err(KbError::NotADir(target.to_path_buf()));
    }
    let mut dirs_created = 0u32;
    for sub in SUBDIRS {
        let p = target.join(sub);
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
            dirs_created += 1;
        }
    }
    // 文件**只补不覆盖** — 老板可能已经在 wiki/index.md 写了内容
    let mut files_created = 0u32;
    let wiki_index = target.join("wiki").join("index.md");
    if !wiki_index.exists() {
        std::fs::write(&wiki_index, WELCOME_MD)?;
        files_created += 1;
    }
    let gap_log = target.join("gap-log.md");
    if !gap_log.exists() {
        std::fs::write(&gap_log, GAP_LOG_MD)?;
        files_created += 1;
    }
    Ok(KbInitResult {
        created_at: Local::now(),
        path: target.to_path_buf(),
        files_created,
        dirs_created,
        reused_existing: true,
    })
}
