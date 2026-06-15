//! 聊天录屏取证任务(chat_evidence_jobs 表)的 CRUD。

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

/// 取证任务行结构(对应 chat_evidence_jobs 表)。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatEvidenceJob {
    pub id: String,
    pub case_id: String,
    pub video_path: String,
    pub preset: String,
    pub status: String,
    pub output_dir: Option<String>,
    pub pdf_path: Option<String>,
    pub frame_count: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 新建任务入参(前端传)。
#[derive(Debug, Clone, Deserialize)]
pub struct NewChatEvidenceJob {
    pub case_id: String,
    pub video_path: String,
    pub preset: String,
}

/// 插入一条 pending 记录,返回完整行。
pub async fn insert(
    pool: &SqlitePool,
    j: &NewChatEvidenceJob,
) -> Result<ChatEvidenceJob, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO chat_evidence_jobs (id, case_id, video_path, preset) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&j.case_id)
    .bind(&j.video_path)
    .bind(&j.preset)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, ChatEvidenceJob>("SELECT * FROM chat_evidence_jobs WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await
}

/// 查某案件的全部取证记录,按 created_at 倒序。
pub async fn list_by_case(
    pool: &SqlitePool,
    case_id: &str,
) -> Result<Vec<ChatEvidenceJob>, sqlx::Error> {
    sqlx::query_as::<_, ChatEvidenceJob>(
        "SELECT * FROM chat_evidence_jobs WHERE case_id = ? ORDER BY created_at DESC",
    )
    .bind(case_id)
    .fetch_all(pool)
    .await
}

/// 更新任务状态(后台 spawn 完成后调用)。
#[allow(clippy::too_many_arguments)]
pub async fn update_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    output_dir: Option<&str>,
    pdf_path: Option<&str>,
    frame_count: Option<i64>,
    elapsed_ms: Option<i64>,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE chat_evidence_jobs SET status = ?, output_dir = ?, pdf_path = ?, \
         frame_count = ?, elapsed_ms = ?, error = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(status)
    .bind(output_dir)
    .bind(pdf_path)
    .bind(frame_count)
    .bind(elapsed_ms)
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
