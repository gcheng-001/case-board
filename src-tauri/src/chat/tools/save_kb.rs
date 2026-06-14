//! 入库工具:`save_company_report`(P2 · 把企业调查报告落进本地知识库 `raw/companies/`)。
//!
//! 老板的目标:把元典查到的企业多维度信息**综合成一份调查报告、沉淀进知识库复用**,而不是
//! 散在聊天里查完即弃。本工具让 AI 助手在**用户明确要求「把这家公司存档 / 出调查报告入库」**时,
//! 把它已经综合好的报告写进 `<KB>/raw/companies/{公司名}.md`(L1 原始层,跟现有元典报告同目录同格式)。
//!
//! 边界(为什么只写 `raw/companies`、不写 `wiki/sources`):`wiki/sources` 是受 `.wiki-schema.md` +
//! legal-kb skill 治理的策展层(L2/L3 结构 + 回链),LLM 直接写会污染信任层。提升到 `wiki/sources`
//! 仍由老板 / legal-kb skill 决策,本工具不碰。
//!
//! 安全:只写死的 `raw/companies` 目录、文件名安全化(防穿越)、**不覆盖已存在文件**(防冲掉
//! 人工标注过的旧报告);`is_mutating`(agent_loop 一轮里串行独占)。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{require_str, Tool, ToolContext, ToolError, ToolResult};

/// 公司名 → 安全文件名 stem:剥路径危险字符(保留中文 / 全角括号),限长 80。
fn safe_company_stem(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => '_',
            c => c,
        })
        .collect();
    cleaned.trim().chars().take(80).collect()
}

/// 把报告写进 `<kb_root>/raw/companies/{安全名}.md`。
/// 返回 `Ok(Some(relpath))` = 已写;`Ok(None)` = 已存在未覆盖。空名由调用方前置拦。
fn write_company_report(
    kb_root: &std::path::Path,
    company_name: &str,
    content_md: &str,
    today: &str,
) -> std::io::Result<Option<String>> {
    let stem = safe_company_stem(company_name);
    let dir = kb_root.join("raw").join("companies");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.md", stem));
    if path.exists() {
        return Ok(None); // 不覆盖
    }
    let body = format!(
        "# {company_name} — 调查报告\n\n\
         **入库时间:** {today}\n\
         **数据来源:** 元典企业信息接口(CaseBoard AI 助手综合)\n\n\
         ---\n\n{content_md}\n"
    );
    std::fs::write(&path, &body)?;
    Ok(Some(format!("raw/companies/{stem}.md")))
}

pub struct SaveCompanyReport;

#[async_trait]
impl Tool for SaveCompanyReport {
    fn name(&self) -> &str {
        "save_company_report"
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        include_str!("descriptions/save_company_report.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "company_name": {"type": "string", "description": "企业全称(用作文件名 + 报告标题),如「温州示例科技有限公司」"},
                "content_md": {"type": "string", "description": "调查报告正文 Markdown(不含顶部大标题,本工具会自动加)。建议含:主体概况 / 股权结构 / 关键发现 / 风险记录(失信 / 被执行 / 冻结 / 出质 / 处罚 / 异常 / 欠税)/ 综合判断。数据须来自元典工具真实返回,不得编造"}
            },
            "required": ["company_name", "content_md"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        let company_name = require_str(args, "company_name")?;
        let content_md = require_str(args, "content_md")?;
        let kb = ctx.local_kb.ok_or_else(|| {
            ToolError::Runtime(
                "本地知识库未启用(用户未设置 local_kb_root 或路径不存在),无法入库".into(),
            )
        })?;
        if safe_company_stem(company_name).is_empty() {
            return Err(ToolError::InvalidArgs("company_name 不能为空".into()));
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        match write_company_report(&kb.root, company_name, content_md, &today)? {
            Some(rel) => Ok(ToolResult::plain(format!(
                "✅ 已入库:{rel}。之后 search_local_kb 可直接检索到这家公司(0 积分),\
                 不必再调元典重查。\n⚠️ 报告系元典数据综合,关键结论请人工复核后用于办案。"
            ))),
            None => Ok(ToolResult::plain(format!(
                "raw/companies/{}.md 已存在,未覆盖(防冲掉人工标注过的旧档)。\
                 若要更新,请让用户确认后手动处理,或换带日期 / 区分的标题再存。",
                safe_company_stem(company_name)
            ))),
        }
    }
}
