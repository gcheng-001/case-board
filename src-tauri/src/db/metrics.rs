//! 抽取性能埋点(2026-05-26 V0.1.12)。
//!
//! 给反馈通道用:朋友实测后回传数据,作者用来对比本地 vs 云端 OCR 的速度 + 准确率。
//!
//! 写入路径:
//!   `extractor.rs` 在每个 stage 结束时产出 `MetricEntry`,
//!   `pipeline.rs` 拿到后批量 insert(避免在 extractor 里持有 pool)。
//!
//! 读取路径:
//!   `feedback::collect` 拉最近 N 条进 `DiagnosticInfo.metrics_tail`。

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricEntry {
    pub filename: String,
    pub ext: String,
    pub file_size_bytes: i64,
    /// "text_extract" / "ocr" / "llm_extract"
    pub stage: String,
    /// "pdf-inspector" / "pdftotext" / "textutil" / "read_direct"
    /// / "mineru-precision" / "local-vision"
    /// / "deepseek" / "local-llm"
    pub backend: String,
    /// "ok" / "failed" / "skipped"
    pub outcome: String,
    pub elapsed_ms: i64,
    pub text_chars: Option<i64>,
    pub error_short: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRow {
    pub filename: String,
    pub ext: String,
    pub file_size_bytes: i64,
    pub stage: String,
    pub backend: String,
    pub outcome: String,
    pub elapsed_ms: i64,
    pub text_chars: Option<i64>,
    pub error_short: Option<String>,
    pub created_at: String,
}

pub async fn insert_many(pool: &SqlitePool, entries: &[MetricEntry]) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| format!("开 tx 失败:{}", e))?;
    for e in entries {
        sqlx::query(
            "INSERT INTO extraction_metrics \
             (filename, ext, file_size_bytes, stage, backend, outcome, elapsed_ms, text_chars, error_short) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&e.filename)
        .bind(&e.ext)
        .bind(e.file_size_bytes)
        .bind(&e.stage)
        .bind(&e.backend)
        .bind(&e.outcome)
        .bind(e.elapsed_ms)
        .bind(e.text_chars)
        .bind(&e.error_short)
        .execute(&mut *tx)
        .await
        .map_err(|err| format!("insert metric 失败:{}", err))?;
    }
    tx.commit()
        .await
        .map_err(|e| format!("提交 tx 失败:{}", e))?;
    Ok(())
}

/// 拉最近 N 条(给反馈通道用)。
pub async fn query_recent(pool: &SqlitePool, limit: i64) -> Result<Vec<MetricRow>, String> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT filename, ext, file_size_bytes, stage, backend, outcome, elapsed_ms, \
                text_chars, error_short, created_at \
         FROM extraction_metrics ORDER BY created_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("查 extraction_metrics 失败:{}", e))?;

    Ok(rows
        .into_iter()
        .map(|r| MetricRow {
            filename: r.get("filename"),
            ext: r.get("ext"),
            file_size_bytes: r.get("file_size_bytes"),
            stage: r.get("stage"),
            backend: r.get("backend"),
            outcome: r.get("outcome"),
            elapsed_ms: r.get("elapsed_ms"),
            text_chars: r.get("text_chars"),
            error_short: r.get("error_short"),
            created_at: r.get("created_at"),
        })
        .collect())
}
