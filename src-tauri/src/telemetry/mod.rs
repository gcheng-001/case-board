//! 匿名使用遥测 —— 只回答「同事有没有在用 / 用了多久 / 会不会持续回来(留存)」。
//!
//! 隐私铁律(对应 CLAUDE.md):
//!   - **绝不上报任何案件数据**:无案件名、当事人、文件名、文本、API key。
//!   - 只上报:匿名设备ID(复用 settings.client_id,UUID,跟人无关)、app 版本、
//!     粗粒度 OS(如 "macos aarch64",不带系统版本号)、事件类型、本次开机的随机 session_id。
//!   - 性质:按「设备」区分(不记名,但区分到每台安装)—— 看留存所必需。
//!
//! 上报后端:Supabase REST(PostgREST)。表 + RLS 见 `telemetry/supabase_schema.sql`
//! (匿名 key 只能 INSERT,不能 SELECT)。
//!
//! 编译期注入(URL/KEY 不进 git,见 `telemetry/.env.telemetry` + `scripts/release.sh`):
//!   - `CASEBOARD_TELEMETRY_URL`  例:https://xxxx.supabase.co
//!   - `CASEBOARD_TELEMETRY_KEY`  Supabase publishable / anon key
//!
//! 两个**任一缺失** → 遥测整体静默禁用(`pnpm tauri dev` 不注入 → 开发期天然不上报,
//! 不污染线上数据)。
//!
//! 设计要点:
//!   - **时长靠心跳估算,不靠退出事件**:窗口关闭时 app 立即退出,异步「退出」上报多半
//!     发不出去。改成 session_start 立即发(算第一个 5 分钟桶)+ 每 5 分钟一次 heartbeat,
//!     后台数 heartbeat 即可粗估时长。
//!   - **全程 fire-and-forget**:短超时(5s),任何失败只 dlog,绝不阻塞/影响 app。
//!   - **测试期硬禁用**:`cfg(test)` 下 `enabled()` 恒 false,`cargo test` 不会联网。

use std::time::Duration;

use serde::Serialize;

/// 编译期注入的 Supabase 项目 URL(缺失 → 遥测禁用)。
const TELEMETRY_URL: Option<&str> = option_env!("CASEBOARD_TELEMETRY_URL");
/// 编译期注入的 Supabase 匿名 key(缺失 → 遥测禁用)。
const TELEMETRY_KEY: Option<&str> = option_env!("CASEBOARD_TELEMETRY_KEY");

/// 心跳间隔(秒)。每个心跳在后台≈5 分钟在用。
const HEARTBEAT_SECS: u64 = 300;
/// 单次上报超时(秒)。短,确保挂死的网络永不拖累 app。
const POST_TIMEOUT_SECS: u64 = 5;

/// 遥测是否启用:URL+KEY 都注入了 且 非测试构建。
fn enabled() -> bool {
    !cfg!(test) && TELEMETRY_URL.is_some() && TELEMETRY_KEY.is_some()
}

/// 上报载荷 —— 字段全是非敏感的计数/标识。
#[derive(Debug, Serialize)]
struct UsageEvent<'a> {
    device_id: &'a str,
    app_version: &'a str,
    os: &'a str,
    event_type: &'a str,
    session_id: &'a str,
}

/// 粗粒度 OS 字符串,如 "macos aarch64"。**故意不带系统版本号**,降低去匿名化。
fn os_label() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// 发一条事件(fire-and-forget,内部吞错)。
async fn post_event(device_id: &str, session_id: &str, event_type: &str) {
    let (base, key) = match (TELEMETRY_URL, TELEMETRY_KEY) {
        (Some(b), Some(k)) => (b, k),
        _ => return,
    };
    let url = format!("{}/rest/v1/usage_events", base.trim_end_matches('/'));
    let body = UsageEvent {
        device_id,
        app_version: env!("CARGO_PKG_VERSION"),
        os: &os_label(),
        event_type,
        session_id,
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(POST_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            crate::dlog!("[telemetry] client build 失败: {}", e);
            return;
        }
    };

    let resp = client
        .post(&url)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        // 不让 PostgREST 回传插入的行(RLS 无 SELECT 会报错),只要状态码
        .header("Prefer", "return=minimal")
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => crate::dlog!("[telemetry] {} 非 2xx: {}", event_type, r.status()),
        Err(e) => crate::dlog!("[telemetry] {} 发送失败: {}", event_type, e),
    }
}

/// 启动遥测后台任务。在 Tauri `setup` 钩子里调一次。
///
/// 没启用(dev / 未注入 key / 测试)直接返回,什么都不做。
/// 启用时:同步拿到匿名 device_id + 生成本次 session_id,然后 spawn 一个后台 task:
/// 立即发 session_start → 之后每 5 分钟发 heartbeat。task 随进程退出而终止。
pub fn start() {
    if !enabled() {
        return;
    }

    // device_id 复用反馈通道那个匿名 client_id(UUID,无个人信息)。
    let device_id = match crate::settings::ensure_client_id() {
        Ok(id) => id,
        Err(e) => {
            crate::dlog!("[telemetry] 取 client_id 失败,跳过遥测: {}", e);
            return;
        }
    };
    let session_id = uuid::Uuid::new_v4().to_string();

    tauri::async_runtime::spawn(async move {
        // 立即发一条 session_start(也算第一个 5 分钟桶 → 短会话也至少计一次)。
        post_event(&device_id, &session_id, "session_start").await;

        let mut ticker = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
        // 笔记本合盖休眠唤醒后,默认 Burst 会把错过的 tick 一次性补发,
        // 把"休眠时间"算成"在用",虚高时长。Skip:只按真实流逝的间隔发。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // 第一次 tick 立即返回,跳过(session_start 已覆盖首桶)
        loop {
            ticker.tick().await;
            post_event(&device_id, &session_id, "heartbeat").await;
        }
    });
}
