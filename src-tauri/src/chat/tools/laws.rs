//! 法规法条 5 个 tool(V0.2 D2-D3.B)。
//!
//! 全部走三段式:`try_kb_hit` → 调元典 `yuandian::*` → `save_and_wrap`。
//! 走 KB cache(法规法条永不过期,本地命中等于免费)。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    opt_bool, opt_str, opt_u32, require_str, save_and_wrap, try_kb_hit, yuandian_key, Tool,
    ToolContext, ToolError, ToolResult,
};
use crate::yuandian;

pub struct SearchLaws;

#[async_trait]
impl Tool for SearchLaws {
    fn name(&self) -> &str {
        "search_laws"
    }
    fn description(&self) -> &str {
        include_str!("descriptions/search_laws.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": {"type": "string", "description": "中文关键词,如「合同解除」「违约金」"},
                "effect_level": {"type": "string", "description": "枚举:宪法|法律|行政法规|地方性法规|司法解释"},
                "region": {"type": "string", "description": "省级地方法规过滤,如「浙江省」"},
                "top_k": {"type": "integer", "description": "默认 20,最大 50"}
            },
            "required": ["keyword"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        // D5-6:refer_date 只用于按条/按法规取详情(detail),ft_search 没有这个字段;
        // 不再在 search_laws schema 暴露一个会被忽略的参数(避免误导 LLM)。
        let keyword = require_str(args, "keyword")?;
        let effect_level = opt_str(args, "effect_level").map(String::from);
        let region = opt_str(args, "region").map(String::from);
        let top_k = opt_u32(args, "top_k");

        let cache_params = json!({"keyword": keyword, "top_k": top_k.unwrap_or(20)});
        if let Some(r) = try_kb_hit(ctx, "rh_ft_search", &cache_params) {
            return Ok(r);
        }

        let api_key = yuandian_key(ctx)?;
        let params = yuandian::FtSearchParams {
            keyword: keyword.to_string(),
            fgmc: None,
            effect_level,
            region,
            valid_only: None,
            top_k,
            publish_date_start: None,
            publish_date_end: None,
            implement_date_start: None,
            implement_date_end: None,
        };
        let resp = yuandian::ft_search(api_key, &params).await?;
        Ok(save_and_wrap(
            ctx,
            "rh_ft_search",
            &cache_params,
            keyword,
            resp,
            1,
        ))
    }
}

pub struct GetLawArticle;

#[async_trait]
impl Tool for GetLawArticle {
    fn name(&self) -> &str {
        "get_law_article"
    }
    fn description(&self) -> &str {
        include_str!("descriptions/get_law_article.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "元典法条 ID(优先填)"},
                "fgmc": {"type": "string", "description": "法规名(配 ftnum 用)"},
                "ftnum": {"type": "string", "description": "条号,纯数字字符串"},
                "fgid": {"type": "string", "description": "元典法规 ID(从 search_laws / law_vector_search 结果的 fgid 字段透传)。填了它 + ftnum 就走整部法规全文缓存,大幅省积分;同一法规后续条文 0 积分命中"},
                "refer_date": {"type": "string", "description": "YYYY-MM-DD,时点版本"}
            }
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        let id = opt_str(args, "id").map(String::from);
        let fgmc = opt_str(args, "fgmc").map(String::from);
        let ftnum = opt_str(args, "ftnum").map(String::from);
        let fgid = opt_str(args, "fgid").map(String::from);
        let refer_date = opt_str(args, "refer_date").map(String::from);
        // D5-3:接受 id / (fgmc+ftnum) / (fgid+ftnum) 三选一 —— 原 guard 漏了 fgid+ftnum,
        // 导致只带 fgid+ftnum(无 id/fgmc)时被前置拒掉、走不到下方省积分的全文路径。
        let has_fgid_ft = fgid.is_some() && ftnum.is_some();
        let has_fgmc_ft = fgmc.is_some() && ftnum.is_some();
        if id.is_none() && !has_fgmc_ft && !has_fgid_ft {
            return Err(ToolError::InvalidArgs(
                "需要填 id / (fgmc + ftnum) / (fgid + ftnum) 之一".into(),
            ));
        }
        // V0.2.2 · 法规全文路径(省积分):有 fgid + ftnum 时优先走「整部法规全文缓存 + 按条提取」。
        // fgid 保证版本正确(按法规名拉会拉错修订版、条号错位致命);提取/拉取失败自动降级到下方单条接口。
        if let (Some(fgid), Some(ftnum)) = (fgid.as_deref(), ftnum.as_deref()) {
            if let Some(r) = try_fulltext_article(ctx, fgid, ftnum, refer_date.as_deref()).await? {
                return Ok(r);
            }
        }
        let cache_key = id.clone().unwrap_or_else(|| {
            format!(
                "{}-{}",
                fgmc.as_deref().unwrap_or(""),
                ftnum.as_deref().unwrap_or("")
            )
        });
        let cache_params = json!({"key": cache_key});
        if let Some(r) = try_kb_hit(ctx, "rh_ft_detail", &cache_params) {
            return Ok(r);
        }
        let api_key = yuandian_key(ctx)?;
        let params = yuandian::FtDetailParams {
            id,
            fgmc,
            ftnum,
            refer_date,
        };
        let resp = yuandian::ft_detail(api_key, &params).await?;
        Ok(save_and_wrap(
            ctx,
            "rh_ft_detail",
            &cache_params,
            &cache_key,
            resp,
            1,
        ))
    }
}

/// V0.2.2 · 法规全文路径:按 `fgid`(法规 ID,保证版本正确)拿整部法规全文,从中按条号提取单条。
///
/// 返回:
/// - `Ok(Some(单条 ToolResult))` —— 成功提取(本地命中 0 积分 / 拉全文 1 积分,该法规后续条 0 积分)。
/// - `Ok(None)` —— 无 key / 拉全文失败 / 全文无此条号 → 调用方应**降级到单条接口**(不得编造)。
///
/// 安全网:版本由 `fgid` 保证(绝不按法规名拉,会拉错修订版);提取不到 → None 降级。
async fn try_fulltext_article(
    ctx: &ToolContext<'_>,
    fgid: &str,
    ftnum: &str,
    refer_date: Option<&str>,
) -> Result<Option<ToolResult>, ToolError> {
    let fg_params = json!({ "key": fgid });
    // 1) 本地法规全文缓存(直接读 sidecar,按 fgid)
    let cached: Option<Value> = ctx
        .local_kb
        .and_then(|kb| kb.load_raw_response("rh_fg_detail", &fg_params))
        .and_then(|s| serde_json::from_str(&s).ok());
    let (resp, hit) = match cached {
        Some(j) => (j, true),
        None => {
            // 2) 未命中 → 按 fgid 拉整部法规全文(版本正确),顺手缓存供后续 0 积分命中
            let Ok(api_key) = yuandian_key(ctx) else {
                return Ok(None); // 无 key → 降级单条
            };
            let params = yuandian::FgDetailParams {
                id: Some(fgid.to_string()),
                fgmc: None,
                refer_date: refer_date.map(String::from),
            };
            match yuandian::fg_detail(api_key, &params).await {
                Ok(r) => {
                    // P1:这条「拉整部法规全文进缓存」是老板最看重的省积分路径,原来只 save_raw_response
                    // 写了裸 .raw.json(无 .md 无索引、成了隐身孤儿,review 时最像可删垃圾)。改走
                    // persist_detail:写可读命名全文 MD + 索引 + sidecar,跟 save_and_wrap 详情类一致。
                    // 空全文(无 content)则不写(目录卫生)。
                    if let Some(kb) = ctx.local_kb {
                        if !super::response_is_empty("rh_fg_detail", &r) {
                            let body = serde_json::to_string_pretty(&r).unwrap_or_default();
                            super::persist_detail(kb, "rh_fg_detail", &fg_params, &r, &body);
                        }
                    }
                    (r, false)
                }
                Err(_) => return Ok(None), // 拉全文失败 → 降级单条
            }
        }
    };
    // 3) 从全文按条号提取单条
    let Some(content) = resp.pointer("/data/content").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    match super::law_fulltext::extract_article(content, ftnum) {
        Some(article) => {
            // D5-2:包成统一 JSON(与单条降级路径及其它工具的 pretty-JSON 结果一致),
            // 并保留法规名/fgid 元数据供引用协议用,而不是返回裸文本。
            let fgmc = resp
                .pointer("/data/fgmc")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let wrapped = json!({
                "fgmc": fgmc,
                "fgid": fgid,
                "ftnum": ftnum,
                "content": article,
            });
            Ok(Some(ToolResult {
                content: serde_json::to_string_pretty(&wrapped).unwrap_or(article),
                yuandian_credits_used: if hit { 0 } else { 1 },
                kb_hit: hit,
            }))
        }
        None => Ok(None), // 全文里没这条号 → 降级单条,绝不编造
    }
}

pub struct SearchRegulations;

#[async_trait]
impl Tool for SearchRegulations {
    fn name(&self) -> &str {
        "search_regulations"
    }
    fn description(&self) -> &str {
        include_str!("descriptions/search_regulations.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": {"type": "string"},
                "fgmc": {"type": "string", "description": "法规名模糊匹配"},
                "effect_level": {"type": "string"},
                "region": {"type": "string"},
                "valid_only": {"type": "boolean", "description": "默认 true"},
                "publish_date_start": {"type": "string", "description": "YYYY-MM-DD"},
                "publish_date_end": {"type": "string", "description": "YYYY-MM-DD"},
                "top_k": {"type": "integer", "description": "默认 20"}
            }
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        let keyword = opt_str(args, "keyword").map(String::from);
        let fgmc = opt_str(args, "fgmc").map(String::from);
        if keyword.is_none() && fgmc.is_none() {
            return Err(ToolError::InvalidArgs(
                "keyword 跟 fgmc 至少填一个,纯过滤无关键词易返回过宽".into(),
            ));
        }
        let effect_level = opt_str(args, "effect_level").map(String::from);
        let region = opt_str(args, "region").map(String::from);
        let valid_only = opt_bool(args, "valid_only");
        let publish_date_start = opt_str(args, "publish_date_start").map(String::from);
        let publish_date_end = opt_str(args, "publish_date_end").map(String::from);
        let top_k = opt_u32(args, "top_k");

        let cache_params = json!({
            "keyword": keyword.clone().unwrap_or_default(),
            "fgmc": fgmc.clone().unwrap_or_default(),
            "top_k": top_k.unwrap_or(20),
        });
        if let Some(r) = try_kb_hit(ctx, "rh_fg_search", &cache_params) {
            return Ok(r);
        }

        let api_key = yuandian_key(ctx)?;
        let params = yuandian::FgSearchParams {
            keyword: keyword.clone(),
            fgmc: fgmc.clone(),
            effect_level,
            region,
            valid_only,
            top_k,
            publish_date_start,
            publish_date_end,
            implement_date_start: None,
            implement_date_end: None,
        };
        let resp = yuandian::fg_search(api_key, &params).await?;
        let summary = keyword.or(fgmc).unwrap_or_default();
        Ok(save_and_wrap(
            ctx,
            "rh_fg_search",
            &cache_params,
            &summary,
            resp,
            1,
        ))
    }
}

pub struct GetRegulationDetail;

#[async_trait]
impl Tool for GetRegulationDetail {
    fn name(&self) -> &str {
        "get_regulation_detail"
    }
    fn description(&self) -> &str {
        include_str!("descriptions/get_regulation_detail.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "元典法规 ID(优先填)"},
                "fgmc": {"type": "string", "description": "法规全名"},
                "refer_date": {"type": "string", "description": "YYYY-MM-DD"},
                "full": {"type": "boolean", "description": "默认 false:只回法规元信息 + 正文预览(整部已缓存本地,具体条文请用 get_law_article(fgid+ftnum) 取单条,省上下文)。仅当用户明确要整部全文(如导出全文)才填 true"}
            }
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        let id = opt_str(args, "id").map(String::from);
        let fgmc = opt_str(args, "fgmc").map(String::from);
        if id.is_none() && fgmc.is_none() {
            return Err(ToolError::InvalidArgs("需要填 id 或 fgmc 二选一".into()));
        }
        // full=false(默认):不把整部全文喂主上下文,只回元信息+预览,引导走 get_law_article 取单条。
        let full = opt_bool(args, "full").unwrap_or(false);
        let cache_key = id
            .clone()
            .unwrap_or_else(|| fgmc.clone().unwrap_or_default());
        let cache_params = json!({"key": cache_key});
        if let Some(mut r) = try_kb_hit(ctx, "rh_fg_detail", &cache_params) {
            if !full {
                r.content = slim_regulation_for_llm(&r.content);
            }
            return Ok(r);
        }
        let api_key = yuandian_key(ctx)?;
        let params = yuandian::FgDetailParams {
            id,
            fgmc,
            refer_date: opt_str(args, "refer_date").map(String::from),
        };
        let resp = yuandian::fg_detail(api_key, &params).await?;
        // 整部仍缓存本地(供 get_law_article 按条提取);喂 LLM 默认精简。
        let mut r = save_and_wrap(ctx, "rh_fg_detail", &cache_params, &cache_key, resp, 1);
        if !full {
            r.content = slim_regulation_for_llm(&r.content);
        }
        Ok(r)
    }
}

/// 把整部法规详情精简成「喂 LLM 的元信息 + 正文预览」(不喂整部全文)。
/// 解析失败兜底返回原文。供 get_regulation_detail 默认路径用;整部仍缓存本地。
fn slim_regulation_for_llm(full_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(full_json) else {
        return full_json.to_string();
    };
    let data = v.get("data");
    let content = data
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let preview: String = content.chars().take(400).collect();
    let truncated = content.chars().count() > 400;
    let mut m = serde_json::Map::new();
    // 保留 content 以外的元信息(法规名 / fgid / 发布实施日期 / 效力级别等)
    if let Some(obj) = data.and_then(|d| d.as_object()) {
        for (k, val) in obj {
            if k != "content" {
                m.insert(k.clone(), val.clone());
            }
        }
    }
    m.insert(
        "正文预览".into(),
        Value::String(if truncated {
            format!("{preview}……")
        } else {
            preview
        }),
    );
    m.insert(
        "_note".into(),
        Value::String(
            "整部法规已缓存本地。**要具体某条全文,用 get_law_article(fgid+ftnum) 取单条**(省上下文);\
             确需整部全文(如给用户导出)再调 get_regulation_detail(full=true)。"
                .into(),
        ),
    );
    serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_else(|_| full_json.to_string())
}

pub struct LawVectorSearch;

#[async_trait]
impl Tool for LawVectorSearch {
    fn name(&self) -> &str {
        "law_vector_search"
    }
    fn description(&self) -> &str {
        include_str!("descriptions/law_vector_search.md")
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "自然语言描述,不是关键词"},
                "effect_level": {"type": "string"},
                "valid_only": {"type": "boolean", "description": "默认 true"},
                "top_k": {"type": "integer", "description": "默认 10"}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext<'_>) -> Result<ToolResult, ToolError> {
        let query = require_str(args, "query")?;
        let effect_level = opt_str(args, "effect_level").map(String::from);
        let valid_only = opt_bool(args, "valid_only");
        let top_k = opt_u32(args, "top_k");

        let cache_params = json!({"query": query, "top_k": top_k.unwrap_or(10)});
        if let Some(r) = try_kb_hit(ctx, "law_vector_search", &cache_params) {
            return Ok(r);
        }
        let api_key = yuandian_key(ctx)?;
        let params = yuandian::LawVectorSearchParams {
            query: query.to_string(),
            effect_level,
            valid_only,
            implement_date_start: None,
            implement_date_end: None,
            top_k,
        };
        let resp = yuandian::law_vector_search(api_key, &params).await?;
        Ok(save_and_wrap(
            ctx,
            "law_vector_search",
            &cache_params,
            query,
            resp,
            1,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_laws_schema_has_keyword_required() {
        let t = SearchLaws;
        let s = t.parameters_schema();
        assert_eq!(s["type"], "object");
        let required = s["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "keyword"));
    }

    #[test]
    fn get_law_article_schema_no_required() {
        // id 或 (fgmc, ftnum) 二选一,运行时校验,schema 无 required
        let t = GetLawArticle;
        let s = t.parameters_schema();
        assert!(s.get("required").is_none() || s["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn law_vector_search_requires_query() {
        let t = LawVectorSearch;
        let s = t.parameters_schema();
        let required = s["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn slim_regulation_drops_full_content_keeps_meta() {
        let big = "第一条 总则……".to_string() + &"正文很长".repeat(1000);
        let full = serde_json::to_string(&json!({
            "code": 0,
            "data": {"fgmc": "中华人民共和国民法典", "fgid": "x9", "content": big}
        }))
        .unwrap();
        let slim = slim_regulation_for_llm(&full);
        // 整部全文不进上下文
        assert!(
            !slim.contains(&"正文很长".repeat(1000)),
            "整部全文不应出现在精简结果里"
        );
        // 保留元信息 + 引导走单条
        assert!(slim.contains("中华人民共和国民法典"), "保留法规名");
        assert!(slim.contains("x9"), "保留 fgid");
        assert!(slim.contains("get_law_article"), "引导取单条");
        assert!(slim.contains("正文预览"), "给预览");
        // 解析失败兜底原样
        assert_eq!(slim_regulation_for_llm("不是json"), "不是json");
    }
}
