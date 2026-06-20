//! 私人专属功能的 Rust 侧(双轨发布模型)——**开源版桩**。
//!
//! 命令签名与私人仓一致、直接返回 Err:这样 lib.rs 的 `mod private;` + generate_handler
//! 注册在开源仓照样编译(符号存在),只是私人功能不可用(前端也没有「独立」tab 去调它)。

use sqlx::SqlitePool;

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
    let _ = (source_path, template_id);
    if !confirmed {
        return Err("未获得本次外部上传确认".into());
    }
    Err("当前为公开版，未安装经授权的要素式外部转换插件".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn external_conversion_requires_confirmation_and_plugin() {
        let denied = element_external_convert("fixture.docx".into(), "type".into(), false).await;
        assert!(denied.unwrap_err().contains("未获得"));

        let unavailable =
            element_external_convert("fixture.docx".into(), "type".into(), true).await;
        assert!(unavailable.unwrap_err().contains("未安装"));
    }
}
