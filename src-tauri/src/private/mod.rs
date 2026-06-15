//! 私人专属功能的 Rust 侧(双轨发布模型)——**开源版桩**。
//!
//! 命令签名与私人仓一致、直接返回 Err:这样 lib.rs 的 `mod private;` + generate_handler
//! 注册在开源仓照样编译(符号存在),只是私人功能不可用(前端也没有「独立」tab 去调它)。

use sqlx::SqlitePool;

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
