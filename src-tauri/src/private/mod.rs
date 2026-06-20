//! 私人专属功能的 Rust 侧(双轨发布模型)。
//!
//! 要素式转换复用公开的智能转写服务协议,不依赖另行部署 Fachuan 后端。

use std::path::Path;
use std::time::Duration;

use base64::Engine;
use reqwest::multipart;
use serde_json::{json, Value};
use sqlx::SqlitePool;

const GDZQFY_AUTH_URL: &str = "https://www.gdzqfy.gov.cn/api/utils/getscwsurl";
const ZNSZJ_BASE: &str = "https://wxfxpg.susong51.com/znszj-touch";
const EXTERNAL_CONVERT_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, serde::Serialize)]
pub struct ExternalElementResult {
    pub filename: String,
    pub data_base64: String,
    pub preview_text: String,
}

/// 私人功能(代理读 Supabase 遥测)——开源版不提供。
#[tauri::command]
pub async fn telemetry_get(
    base: String,
    key: String,
    path: String,
    range_start: u32,
    range_end: u32,
) -> Result<String, String> {
    let _ = (base, key, path, range_start, range_end);
    Err("该功能仅在作者自用版提供".into())
}

/// 私人功能(清空元典积分账,测命中率用)——开源版不提供。
#[tauri::command]
pub async fn reset_yuandian_credits(_pool: tauri::State<'_, SqlitePool>) -> Result<u64, String> {
    Err("该功能仅在作者自用版提供".into())
}

/// 要素式外部转换的公开版安全桩。
///
/// `confirmed` 由前端在当次原生确认对话框通过后传入；私人版必须保持
/// 相同签名并二次校验，禁止记住或默认授权。
#[tauri::command]
pub async fn element_external_convert(
    source_path: String,
    template_id: String,
    confirmed: bool,
) -> Result<ExternalElementResult, String> {
    if !confirmed {
        return Err("未获得本次外部上传确认".into());
    }
    let mbid = fachuan_mbid_for_template(&template_id)
        .ok_or_else(|| "该文书类型尚未映射到 Fachuan 外部转换格式".to_string())?;
    let path = Path::new(&source_path);
    let metadata = std::fs::metadata(path).map_err(|e| format!("无法读取源文书: {e}"))?;
    if metadata.len() > 20 * 1024 * 1024 {
        return Err("文书超过 20MB 上限".into());
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "docx" | "doc" | "pdf") {
        return Err("仅支持 .docx、.doc 和 .pdf 格式".into());
    }
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("document.docx")
        .to_string();
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("读取源文书失败: {e}"))?;
    let result = tokio::time::timeout(
        Duration::from_secs(EXTERNAL_CONVERT_TIMEOUT_SECS),
        znszj_convert_document(bytes, filename.clone(), mbid),
    )
    .await
    .map_err(|_| {
        "要素式转换超过 60 秒未完成，请稍后重试，或改用法院网页登录流程。".to_string()
    })??;
    if result.len() > 20 * 1024 * 1024 {
        return Err("要素式转换结果超过 20MB 上限".into());
    }
    if !result.starts_with(b"PK") {
        return Err("智能转写服务未返回有效 DOCX 文件".into());
    }
    let output_name = element_filename(&filename);
    Ok(ExternalElementResult {
        filename: output_name,
        data_base64: base64::engine::general_purpose::STANDARD.encode(result),
        preview_text: "智能转写服务已返回 Word 文件。请保存后在 Word 中审阅表格内容。".into(),
    })
}

async fn znszj_convert_document(
    file_content: Vec<u8>,
    filename: String,
    mbid: &str,
) -> Result<Vec<u8>, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(EXTERNAL_CONVERT_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(10))
        // 两个地址均为上方固定白名单；兼容本机 HTTPS 代理注入的证书。
        .danger_accept_invalid_certs(true);
    if let Some(proxy_url) = configured_https_proxy() {
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(|e| format!("系统 HTTPS 代理配置无效: {e}"))?;
        builder = builder.proxy(proxy);
    }
    let client = builder
        .build()
        .map_err(|e| format!("初始化智能转写客户端失败: {e}"))?;

    let auth_entry = fetch_auth_entry(&client).await?;
    if auth_entry.get("code").and_then(Value::as_str) != Some("200") {
        return Err(service_error("获取智能转写入口失败", &auth_entry));
    }
    let auth_url = required_str(&auth_entry, "data", "智能转写入口缺少认证地址")?;
    let signature_code = auth_url
        .split_once("signatureCode=")
        .map(|(_, value)| value.split('&').next().unwrap_or(value))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "智能转写入口缺少 signatureCode".to_string())?;

    let auth = checked_json(
        client
            .post(format!("{ZNSZJ_BASE}/api/v1/pcqsz/authentication"))
            .json(&json!({"signatureCode": signature_code, "sessionId": "", "mbid": ""}))
            .send()
            .await,
        "智能转写认证",
    )
    .await?;
    require_success(&auth, "智能转写认证失败")?;
    let auth_data = auth
        .get("data")
        .ok_or_else(|| "智能转写认证结果缺少 data".to_string())?;
    let token = required_str(auth_data, "token", "智能转写认证结果缺少 token")?;
    let mac = required_str(auth_data, "mac", "智能转写认证结果缺少 mac")?;

    let device = checked_json(
        client
            .post(format!("{ZNSZJ_BASE}/touch/getCodeByMac"))
            .query(&[("mac", mac)])
            .send()
            .await,
        "获取智能转写设备标识",
    )
    .await?;
    require_success(&device, "获取智能转写设备标识失败")?;
    let sbbs = required_str(&device, "code", "智能转写结果缺少设备标识")?;

    let upload = checked_json(
        client
            .post(format!("{ZNSZJ_BASE}/api/v1/tableTemplate/uploadOriginQsz"))
            .header("mac", mac)
            .multipart(multipart::Form::new().part(
                "file",
                multipart::Part::bytes(file_content).file_name(filename),
            ))
            .send()
            .await,
        "上传传统文书",
    )
    .await?;
    require_success(&upload, "传统文书上传失败")?;
    let extracted_text = required_str(&upload, "data", "传统文书识别结果为空")?;

    let converted = checked_json(
        client
            .post(format!("{ZNSZJ_BASE}/api/v1/tableTemplate/text2model"))
            .header("token", token)
            .header("mac", mac)
            .json(&json!({"text": extracted_text, "mbid": mbid}))
            .send()
            .await,
        "传统文书智能转写",
    )
    .await?;
    require_success(&converted, "传统文书智能转写失败")?;
    let structured = converted
        .get("data")
        .ok_or_else(|| "智能转写结果缺少结构化数据".to_string())?;

    let saved = checked_json(
        client
            .post(format!(
                "{ZNSZJ_BASE}/api/v1/tableTemplate/saveAndGetDownloadUrl"
            ))
            .header("token", token)
            .header("mac", mac)
            .json(&json!({"mbid": mbid, "data": structured, "sbbs": sbbs}))
            .send()
            .await,
        "生成要素式 Word",
    )
    .await?;
    require_success(&saved, "生成要素式 Word 失败")?;
    let download_url = required_str(&saved, "data", "生成结果缺少下载地址")?;
    let parsed = reqwest::Url::parse(download_url)
        .or_else(|_| reqwest::Url::parse(&format!("https://placeholder.invalid{download_url}")))
        .map_err(|_| "无法解析要素式 Word 下载地址".to_string())?;
    let params: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let response = client
        .get(format!("{ZNSZJ_BASE}/api/v1/tableTemplate/download/docx"))
        .header("token", token)
        .header("mac", mac)
        .query(&params)
        .send()
        .await
        .map_err(|e| classify_znszj_error(&e, "下载要素式 Word"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "下载要素式 Word 失败，HTTP 状态码 {}",
            status.as_u16()
        ));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|e| format!("读取要素式 Word 失败: {e}"))
}

async fn fetch_auth_entry(client: &reqwest::Client) -> Result<Value, String> {
    let mut last_error = String::new();
    for attempt in 1..=2 {
        let result = checked_json(
            client
                .post(GDZQFY_AUTH_URL)
                .header(reqwest::header::CONTENT_LENGTH, "0")
                .body(Vec::new())
                .send()
                .await,
            "获取智能转写入口",
        )
        .await;
        match result {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error,
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_secs(attempt)).await;
        }
    }
    Err(format!("获取智能转写入口失败，已重试 2 次: {last_error}"))
}

fn configured_https_proxy() -> Option<String> {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    macos_system_https_proxy()
}

#[cfg(target_os = "macos")]
fn macos_system_https_proxy() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut enabled = false;
    let mut host = None;
    let mut port = None;
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("HTTPSEnable :") {
            enabled = value.trim() == "1";
        } else if let Some(value) = line.strip_prefix("HTTPSProxy :") {
            host = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("HTTPSPort :") {
            port = value.trim().parse::<u16>().ok();
        }
    }
    match (enabled, host, port) {
        (true, Some(host), Some(port)) if !host.is_empty() => Some(format!("http://{host}:{port}")),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn macos_system_https_proxy() -> Option<String> {
    None
}

async fn checked_json(
    response: Result<reqwest::Response, reqwest::Error>,
    step: &str,
) -> Result<Value, String> {
    let response = response.map_err(|e| classify_znszj_error(&e, step))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{step}失败，HTTP 状态码 {}", status.as_u16()));
    }
    response
        .json::<Value>()
        .await
        .map_err(|e| format!("{step}返回格式异常: {e}"))
}

fn require_success(value: &Value, message: &str) -> Result<(), String> {
    if value.get("success").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(service_error(message, value))
    }
}

fn required_str<'a>(value: &'a Value, key: &str, message: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| message.to_string())
}

fn service_error(prefix: &str, value: &Value) -> String {
    let detail = value
        .get("message")
        .or_else(|| value.get("msg"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    detail.map_or_else(
        || prefix.to_string(),
        |detail| format!("{prefix}: {detail}"),
    )
}

fn classify_znszj_error(error: &reqwest::Error, step: &str) -> String {
    if error.is_timeout() {
        format!("{step}超时({EXTERNAL_CONVERT_TIMEOUT_SECS} 秒)，请稍后重试")
    } else if error.is_connect() {
        format!("{step}无法连接智能转写服务，请检查网络")
    } else {
        format!("{step}失败: {error}")
    }
}

fn element_filename(filename: &str) -> String {
    let stem = filename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(filename);
    let converted = stem.replace("传统", "要素式");
    if converted == stem {
        format!("要素式{stem}.docx")
    } else {
        format!("{converted}.docx")
    }
}

fn fachuan_mbid_for_template(template_id: &str) -> Option<&'static str> {
    Some(match template_id {
        "complaint_private_lending" => "mjjdqsz",
        "complaint_divorce" => "lhjfqsz",
        "complaint_sales" => "mmhtqsz",
        "complaint_property_service" => "wyfwqsz",
        "complaint_labor" => "ldzyqsz",
        "complaint_traffic" => "jdcjtsgqsz",
        "complaint_financial_loan" => "jrjkqsz",
        "complaint_credit_card" => "yhxykqsz",
        "complaint_finance_lease" => "rzzlhtqsz",
        "complaint_guarantee_insurance" => "bzbxhtqsz",
        "complaint_securities_misstatement" => "zqxjcszrqsz",
        "complaint_house_sale" => "fwmmhtjfmsqsz",
        "complaint_house_lease" => "fwzlhtjfmsqsz",
        "complaint_property_loss_insurance" => "ccssbxhtjfmsqsz",
        "complaint_construction" => "jsgcsghtjfmsqsz",
        "complaint_liability_insurance" => "zrbxhtjfmsqsz",
        "complaint_personal_insurance" => "rsbxhtjfmsqsz",
        "complaint_technology" => "jshtjfmsqsz",
        "application_enforcement" => "qzzxsqs",
        "application_lift_travel_restriction" => "zsjcczfjgtxzcssqs",
        "application_distribution" => "cyfpsqs",
        "application_enforcement_guarantee" => "zxdbsqs",
        "application_enforcement_objection" => "zxyysqs",
        "application_enforcement_reconsideration" => "zxfysqs",
        "application_enforcement_supervision" => "zxjdsqs",
        "application_preemptive_right" => "qryxgmqsqs",
        "application_non_enforcement" => "byzxzccjdjshgzzqwssqs",
        "defense_private_lending" => "mjjddbz",
        "defense_divorce" => "lhjfdbz",
        "defense_sales" => "mmhtdbz",
        "defense_property_service" => "wyfwdbz",
        "defense_labor" => "ldzydbz",
        "defense_traffic" => "jdcjtsgdbz",
        "defense_financial_loan" => "jrjkdbz",
        "defense_credit_card" => "yhxykdbz",
        "defense_finance_lease" => "rzzlhtdbz",
        "defense_guarantee_insurance" => "bzbxhtdbz",
        "defense_securities_misstatement" => "zqxjcszrdbz",
        "defense_house_sale" => "fwmmhtjfmsdbz",
        "defense_house_lease" => "fwzlhtjfmsdbz",
        "defense_property_loss_insurance" => "ccssbxhtjfmsdbz",
        "defense_construction" => "jsgcsghtjfmsdbz",
        "defense_liability_insurance" => "zrbxhtjfmsdbz",
        "defense_personal_insurance" => "rsbxhtjfmsdbz",
        "defense_technology" => "jshtjfmsdbz",
        "defense_administrative" => "xzdbz",
        "statement_trademark_revocation" => "sbcxfsxzjfdsryjcss",
        "statement_trademark_invalidity" => "sbwxxzjfdsryjcss",
        "statement_patent_invalidity" => "zlwxxzjfdsryjcss",
        "mediation_application_private_lending" => "mjjdjftjsqs",
        "mediation_application_divorce" => "lhjftjsqs",
        "mediation_application_labor" => "ldjftjsqs",
        "mediation_application_traffic" => "jdcjtsgzrjftjsqs",
        "mediation_defense_private_lending" => "mjjdjftjdbyjs",
        "mediation_defense_divorce" => "lhjftjdbyjs",
        "mediation_defense_labor" => "ldjftjdbyjs",
        "mediation_defense_traffic" => "jdcjtsgzrjftjdbyjs",
        "other_evidence_list" => "zjqd",
        "other_arbitration_application" => "zcsqs",
        "other_power_of_attorney_personal" => "sqwtsgr",
        "other_administrative_reconsideration_personal" => "xzfysqsgr",
        "other_administrative_reconsideration_entity" => "xzfysqsdw",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn external_conversion_requires_confirmation_before_io() {
        let denied = element_external_convert("fixture.docx".into(), "type".into(), false).await;
        assert!(denied.unwrap_err().contains("未获得"));

        assert_eq!(
            fachuan_mbid_for_template("complaint_private_lending"),
            Some("mjjdqsz")
        );
        assert_eq!(
            element_filename("传统民间借贷起诉状.docx"),
            "要素式民间借贷起诉状.docx"
        );
        assert_eq!(
            GDZQFY_AUTH_URL,
            "https://www.gdzqfy.gov.cn/api/utils/getscwsurl"
        );
    }

    #[tokio::test]
    #[ignore = "需要显式提供测试文书并会上传到外部智能转写服务"]
    async fn live_znszj_conversion_returns_docx() {
        let source =
            std::env::var("CASEBOARD_ELEMENT_TEST_DOC").expect("CASEBOARD_ELEMENT_TEST_DOC");
        let output =
            std::env::var("CASEBOARD_ELEMENT_TEST_OUTPUT").expect("CASEBOARD_ELEMENT_TEST_OUTPUT");
        let bytes = tokio::fs::read(&source).await.unwrap();
        let filename = Path::new(&source)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let result = znszj_convert_document(bytes, filename, "mjjdqsz")
            .await
            .unwrap();
        assert!(result.starts_with(b"PK"));
        tokio::fs::write(output, result).await.unwrap();
    }
}
