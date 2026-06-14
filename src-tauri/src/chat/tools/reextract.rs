//! 文档维护工具:`reextract_document`(V0.3 · 2026-05-31)。
//!
//! 让案件 AI 助手能触发某份源文档的**后台重抽**(重跑 OCR + 字段抽取),等同源文件列表
//! 「重新抽取」按钮。复用 `pipeline::trigger_reextract`(与 Tauri 命令同一逻辑,防漂移)
//! 和 `docs::resolve_doc`(id 或 filename 都能匹配,适配 LLM 常传文件名的现实)。
//!
//! 这是 **mutating + 烧积分** 工具(重置状态 + spawn 后台 OCR/LLM,PDF 走云端 OCR 烧
//! MinerU 积分),description 里要求 LLM 仅在用户需要时调用、不擅自批量重抽。
//!
//! **fire-and-forget**:只触发,本轮拿不到重抽后的新文本(抽取异步,几十秒~分钟),
//! description 明确告知 LLM 不要同轮 read_case_doc 期待新内容。
//!
//! 需要 `ToolContext.app`(AppHandle)来 emit 进度事件;`None`(单测 / 无 GUI)时优雅报错。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::docs::resolve_doc;
use super::{require_str, Tool, ToolContext, ToolError, ToolResult};
use crate::db::documents::list_documents_by_case;

pub struct ReextractDocument;

#[async_trait]
impl Tool for ReextractDocument {
    fn name(&self) -> &str {
        "reextract_document"
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        include_str!("descriptions/reextract_document.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "doc_id": {
                    "type": "string",
                    "description": "要重抽的文档标识:可填 list_case_docs 拿到的 id(UUID,最稳),也可直接填文件名(如「离婚补偿协议.pdf」)"
                }
            },
            "required": ["doc_id"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        let case_id = ctx.case_id.ok_or(ToolError::NoCaseBound)?;
        let key = require_str(args, "doc_id")?;

        // 需要 AppHandle 才能 spawn 后台抽取(发进度事件)。无 GUI 上下文(单测)优雅报错,不 panic。
        let app = ctx.app.clone().ok_or_else(|| {
            ToolError::Runtime(
                "当前环境无法触发重抽(缺 AppHandle)。请提示用户在源文件列表手动点「重新抽取」。"
                    .into(),
            )
        })?;

        let docs = list_documents_by_case(ctx.pool, case_id).await?;
        let doc = resolve_doc(docs, key)?;

        // AI 产物(分析报告 / 起草的文书)没有原始文件可 OCR,挡掉避免无意义重抽。
        if doc.is_ai_artifact {
            return Err(ToolError::InvalidArgs(format!(
                "「{}」是 AI 生成的文档,没有可重抽的原始文件,无法重抽。",
                doc.filename
            )));
        }

        let doc_id = doc.id.clone();
        let filename = crate::ingest::pipeline::trigger_reextract(app, ctx.pool, &doc_id, None)
            .await
            .map_err(ToolError::Runtime)?;

        Ok(ToolResult::plain(format!(
            "✅ 已触发后台重新抽取《{filename}》(doc_id={doc_id})。\
             \n这是**异步任务**:PDF/扫描件走云端 OCR 可能要数十秒到几分钟。完成后该文档的抽取文本会更新,\
             源文件列表会显示进度与结果。\
             \n⚠️ 本轮无法立即读到重抽后的新内容 —— 请提示用户等待完成,稍后再读取/分析,别在本轮紧接着 read_case_doc。\
             \n⚠️ 重抽会重跑 OCR/LLM,PDF 走云端 OCR 会消耗 MinerU 积分。"
        )))
    }
}
