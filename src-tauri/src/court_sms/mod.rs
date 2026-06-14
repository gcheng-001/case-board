//! 法院短信处理(V0.3)。粘贴「人民法院在线服务/一张网」(zxfw.court.gov.cn)的送达短信 →
//! 解析(链接参数 + 案号 + 法院)→ 拉送达文书列表 → 匹配在办案件 → 下载 PDF 进案件
//! `source_folder` → 复用现有「刷新源文件」抽取管线(scan+sync+spawn_extraction)上看板。
//!
//! **只支持一张网纯 API 路径**:送达链接里的 `qdbh/sdbh/sdsin` 自带签名,POST
//! `getWsListBySdbhNew` 即可拿文书列表(**无需登录/token/验证码/浏览器**,已真机实测)。
//! 省级自建平台(湖北/江苏微解纷等)要浏览器自动化 + 凭证,本期不做。
//!
//! 纯逻辑(解析/归一化/HTTP)放本模块;Tauri 命令(匹配案件 + 落盘 + 触发抽取)在 lib.rs。

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 一张网送达链接参数(送达编号 / 渠道编号 / 签名)。三者齐全才能拉文书。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZxfwLink {
    pub sdbh: String,
    pub qdbh: String,
    pub sdsin: String,
}

/// 解析短信得到的结构化信息。
#[derive(Debug, Clone, Serialize, Default)]
pub struct ParsedCourtSms {
    /// 法院全称(从【】抓,可能为空)
    pub court: Option<String>,
    /// 案号原文(未归一化,展示用)
    pub case_no: Option<String>,
    /// 一张网链接参数(None = 短信里没有一张网链接)
    pub link: Option<ZxfwLink>,
}

/// 一张网 `getWsListBySdbhNew` 返回的单份文书。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZxfwDoc {
    /// 文书名称(如「民事调解书」「开庭传票」)
    #[serde(rename = "c_wsmc", default)]
    pub name: String,
    /// 文件格式(pdf 等)
    #[serde(rename = "c_wjgs", default = "default_pdf")]
    pub ext: String,
    /// 下载地址(阿里云政务 OSS 预签名 URL,**有时效**,用前现拉)
    #[serde(default)]
    pub wjlj: String,
    /// 法院名称
    #[serde(rename = "c_fymc", default)]
    pub court: Option<String>,
}

fn default_pdf() -> String {
    "pdf".into()
}

/// 案号归一化:去掉所有空白 + 全角括号 `（）` → 半角 `()`。
/// 比对前**两边都归一**,否则「(2026)苏0214民初4654号」与短信里的全角
/// 「（2026）苏0214民初4654号」、传票里带空格的「(2025)苏 0213 民初 14150 号」会匹配不上。
pub fn normalize_case_no(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '（' => '(',
            '）' => ')',
            _ => c,
        })
        .collect()
}

/// 从短信文本解析法院 / 案号 / 一张网链接参数。
pub fn parse_sms(text: &str) -> ParsedCourtSms {
    ParsedCourtSms {
        court: extract_court(text),
        case_no: extract_case_no(text),
        link: extract_zxfw_link(text),
    }
}

/// 法院:取第一个【…法院】。
fn extract_court(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"【([^【】]*?法院)】").ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// 案号:`(YYYY)<代字><数字><类型><数字>号`,容忍全/半角括号与内部空格。
/// 例:`（2026）苏0214民初4654号` / `(2025)苏 0213 民初 14150 号`。
fn extract_case_no(text: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"[（(]\s*\d{4}\s*[）)]\s*[一-龥]{1,3}\s*\d{2,6}\s*[一-龥]{1,4}\s*\d+\s*号",
    )
    .ok()?;
    re.find(text).map(|m| m.as_str().trim().to_string())
}

/// 一张网链接参数:直接从文本里抓 qdbh / sdbh / sdsin(只在送达链接里出现,正则直取最稳)。
fn extract_zxfw_link(text: &str) -> Option<ZxfwLink> {
    // 必须含一张网域名才认(避免误抓其它平台带同名参数)
    if !text.contains("zxfw.court.gov.cn") {
        return None;
    }
    let grab = |key: &str| -> Option<String> {
        let re = regex::Regex::new(&format!(r"{}=([0-9a-zA-Z]+)", key)).ok()?;
        re.captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    };
    let sdbh = grab("sdbh")?;
    let qdbh = grab("qdbh")?;
    let sdsin = grab("sdsin")?;
    Some(ZxfwLink { sdbh, qdbh, sdsin })
}

const ZXFW_LIST_API: &str =
    "https://zxfw.court.gov.cn/yzw/yzw-zxfw-sdfw/api/v1/sdfw/getWsListBySdbhNew";

fn zxfw_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .build()
        .map_err(|e| format!("构造 HTTP 客户端失败: {}", e))
}

/// 调一张网拿送达文书列表。错误透传真错(已知坑#8)。
pub async fn fetch_zxfw_doc_list(link: &ZxfwLink) -> Result<Vec<ZxfwDoc>, String> {
    let client = zxfw_client()?;
    let body = serde_json::json!({
        "sdbh": link.sdbh, "qdbh": link.qdbh, "sdsin": link.sdsin,
    });
    let resp = client
        .post(ZXFW_LIST_API)
        .header("Content-Type", "application/json")
        .header("Origin", "https://zxfw.court.gov.cn")
        .header("Referer", "https://zxfw.court.gov.cn/zxfw/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36",
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求一张网失败: {}", e))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取一张网响应失败: {}", e))?;
    if !status.is_success() {
        return Err(format!(
            "一张网返回 HTTP {}: {}",
            status,
            truncate(&text, 300)
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("一张网响应非 JSON: {} · {}", e, truncate(&text, 300)))?;
    let code = parsed.get("code").and_then(|v| v.as_i64());
    if code != Some(200) {
        let msg = parsed.get("msg").and_then(|v| v.as_str()).unwrap_or("未知");
        return Err(format!("一张网业务错误(code={:?}): {}", code, msg));
    }
    let data = parsed
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let docs: Vec<ZxfwDoc> =
        serde_json::from_value(data).map_err(|e| format!("解析文书列表失败: {}", e))?;
    Ok(docs)
}

/// 下载单份文书到 `dest`(GET 预签名 URL)。
pub async fn download_doc(wjlj: &str, dest: &std::path::Path) -> Result<u64, String> {
    if wjlj.trim().is_empty() {
        return Err("文书缺少下载地址(wjlj)".into());
    }
    let client = zxfw_client()?;
    let resp = client
        .get(wjlj)
        .send()
        .await
        .map_err(|e| format!("下载文书失败: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("下载文书 HTTP {}", status));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取文书内容失败: {}", e))?;
    std::fs::write(dest, &bytes).map_err(|e| format!("写入文件失败: {}", e))?;
    Ok(bytes.len() as u64)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
