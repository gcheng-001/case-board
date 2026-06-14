//! 团队版网络层:mDNS 发现广播 + 内嵌极简 HTTP 服务(/exchange 接力互换、/join 入队)+ 同步轮。
//!
//! - 极简 HTTP/1.1 服务手搓(tokio TcpListener,仅 POST + Content-Length + JSON 两个端点),
//!   合本仓"手搓零依赖"先例(已知坑 #5 MinerU、MCP stdio/HTTP 客户端)。客户端用现成 reqwest。
//! - 鉴权:请求/响应体都带 `x-team-auth: hex(hmac_sha256(team_secret, body))`,验不过直接拒
//!   (挡同所隔壁团队;团队内信任模型,老板拍板)。/join 例外:申请者还没有 secret,验配对码。
//! - 全量互换不搞增量:团队 ~10 人 × 快照几 KB,<100KB/轮,简单可靠。
//! - settings 每次现读不缓存(已知坑 #16)。

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::store;
use super::{
    hmac_hex, Roster, RosterMember, SignedRoster, SnapshotEnvelope, TeamEdit, TeamIdentity,
};
use crate::settings::{read_settings, write_settings};

pub const SERVICE_TYPE: &str = "_caseboard-team._tcp.local.";
const MAX_BODY: usize = 4 * 1024 * 1024;
const CONN_TIMEOUT: Duration = Duration::from_secs(15);
const AUTH_HEADER: &str = "x-team-auth";

// ============================================================================
// 线缆协议形状
// ============================================================================

/// /exchange 请求与响应同形:把"我知道的全队快照全集 + 我的 roster"整包给对方。
#[derive(Debug, Serialize, Deserialize)]
pub struct ExchangeBody {
    pub team_id: String,
    pub from_member: String,
    pub snapshots: Vec<SnapshotEnvelope>,
    pub roster: Option<SignedRoster>,
    /// Phase 2:编辑请求一并接力(老版本节点缺字段 → 默认空,互不影响)。
    #[serde(default)]
    pub edits: Vec<TeamEdit>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinRequest {
    pub team_id: String,
    pub code: String,
    pub member_id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinResponse {
    pub team_secret: String,
    pub roster: SignedRoster,
}

/// 同步轮报告(给前端「立即同步」反馈)。
#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub peers_found: usize,
    pub peers_synced: usize,
    pub snapshots_merged: usize,
    pub errors: Vec<String>,
}

/// 局域网内发现的(可加入的)团队。
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredTeam {
    pub team_id: String,
    pub team_name: String,
    /// 团队长是否在线(入队必须团队长在线)。
    pub leader_online: bool,
    pub online_members: usize,
}

#[derive(Debug, Clone)]
struct PeerAddr {
    member_id: String,
    role: String,
    team_id: String,
    team_name: String,
    ip: String,
    port: u16,
}

// ============================================================================
// 运行时(随团队配置启停;Tauri State 持有)
// ============================================================================

pub struct TeamNet {
    mdns: mdns_sd::ServiceDaemon,
    fullname: String,
    listener_task: tokio::task::JoinHandle<()>,
    periodic_task: tokio::task::JoinHandle<()>,
    pub port: u16,
}

impl TeamNet {
    pub fn shutdown(self) {
        let _ = self.mdns.unregister(&self.fullname);
        let _ = self.mdns.shutdown();
        self.listener_task.abort();
        self.periodic_task.abort();
    }
}

/// 启动团队网络(监听 + 广播 + 周期同步)。调用前提:settings.team 已配置。
pub async fn start(pool: SqlitePool) -> Result<TeamNet, String> {
    let identity = read_settings()?.team.ok_or("未加入团队,无法启动团队网络")?;

    // 1) 监听随机端口(0.0.0.0,局域网可达)
    let listener = TcpListener::bind(("0.0.0.0", 0))
        .await
        .map_err(|e| format!("绑定监听端口失败: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let accept_pool = pool.clone();
    let listener_task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let pool = accept_pool.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(CONN_TIMEOUT, handle_conn(stream, pool)).await;
                    });
                }
                Err(e) => {
                    crate::dlog!("团队监听 accept 失败: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    // 2) mDNS 广播自己
    let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mDNS 启动失败: {e}"))?;
    let mut props = HashMap::new();
    props.insert("tid".to_string(), identity.team_id.clone());
    props.insert("tname".to_string(), identity.team_name.clone());
    props.insert("mid".to_string(), identity.member_id.clone());
    props.insert("role".to_string(), identity.role.clone());
    let host = format!(
        "caseboard-{}.local.",
        &identity.member_id[..8.min(identity.member_id.len())]
    );
    let service =
        mdns_sd::ServiceInfo::new(SERVICE_TYPE, &identity.member_id, &host, "", port, props)
            .map_err(|e| format!("mDNS 服务信息构建失败: {e}"))?
            .enable_addr_auto();
    let fullname = service.get_fullname().to_string();
    mdns.register(service)
        .map_err(|e| format!("mDNS 注册失败: {e}"))?;

    // 3) 周期同步(10 分钟;启动后先跑一轮,失败静默只记日志)
    let periodic_pool = pool.clone();
    let periodic_task = tokio::spawn(async move {
        loop {
            match sync_round(&periodic_pool).await {
                Ok(r) if r.peers_found > 0 => crate::dlog!(
                    "团队周期同步: 在场 {} 台,合并 {} 份快照",
                    r.peers_found,
                    r.snapshots_merged
                ),
                Ok(_) => {}
                Err(e) => crate::dlog!("团队周期同步失败(下轮再试): {e}"),
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });

    Ok(TeamNet {
        mdns,
        fullname,
        listener_task,
        periodic_task,
        port,
    })
}

// ============================================================================
// 极简 HTTP 服务端
// ============================================================================

async fn handle_conn(mut stream: TcpStream, pool: SqlitePool) {
    let Ok(req) = read_http_request(&mut stream).await else {
        let _ = write_http(&mut stream, 400, "{\"error\":\"bad request\"}", None).await;
        return;
    };
    let (status, body, auth) = route(&req, &pool).await;
    let _ = write_http(&mut stream, status, &body, auth.as_deref()).await;
}

struct HttpReq {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// 读一个「POST + Content-Length + 小 JSON」请求(只支持我们自己客户端发的形状)。
async fn read_http_request(stream: &mut TcpStream) -> Result<HttpReq, String> {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    // 读到头结束符
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err("头部过大".into());
        }
        let n = stream.read(&mut tmp).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("连接提前关闭".into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default().to_string();
    if method != "POST" {
        return Err("只支持 POST".into());
    }
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let content_len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .ok_or("缺 Content-Length")?;
    if content_len > MAX_BODY {
        return Err("body 过大".into());
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_len {
        let n = stream.read(&mut tmp).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("body 不完整".into());
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_len);
    Ok(HttpReq {
        path,
        headers,
        body,
    })
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn write_http(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
    auth: Option<&str>,
) -> Result<(), String> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let auth_line = auth
        .map(|a| format!("{AUTH_HEADER}: {a}\r\n"))
        .unwrap_or_default();
    let resp = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{auth_line}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(resp.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let _ = stream.flush().await;
    Ok(())
}

/// 路由两个端点。返回 (状态码, body, 响应 hmac)。
async fn route(req: &HttpReq, pool: &SqlitePool) -> (u16, String, Option<String>) {
    match req.path.as_str() {
        "/exchange" => handle_exchange(req, pool).await,
        "/join" => handle_join(req, pool).await,
        _ => (404, "{\"error\":\"not found\"}".into(), None),
    }
}

/// 接力互换:验身 → 合并对方快照/roster → 回我们的全集。
async fn handle_exchange(req: &HttpReq, pool: &SqlitePool) -> (u16, String, Option<String>) {
    let Ok(settings) = read_settings() else {
        return (403, "{\"error\":\"no settings\"}".into(), None);
    };
    let Some(identity) = settings.team else {
        return (403, "{\"error\":\"not in team\"}".into(), None);
    };
    // 验 HMAC(防隔壁团队)
    let claimed = req.headers.get(AUTH_HEADER).cloned().unwrap_or_default();
    if hmac_hex(&identity.team_secret, &req.body) != claimed {
        return (403, "{\"error\":\"auth failed\"}".into(), None);
    }
    let Ok(incoming) = serde_json::from_slice::<ExchangeBody>(&req.body) else {
        return (400, "{\"error\":\"bad body\"}".into(), None);
    };
    if incoming.team_id != identity.team_id {
        return (403, "{\"error\":\"team mismatch\"}".into(), None);
    }
    if let Err(e) = merge_incoming(pool, &identity, &incoming).await {
        crate::dlog!("合并对方数据失败: {e}");
    }
    match build_exchange_body(pool, &identity).await {
        Ok(body) => {
            let json = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
            let mac = hmac_hex(&identity.team_secret, json.as_bytes());
            (200, json, Some(mac))
        }
        Err(e) => (400, format!("{{\"error\":\"{e}\"}}"), None),
    }
}

/// 入队:只有团队长能批(配对码只在他机器上)。
async fn handle_join(req: &HttpReq, pool: &SqlitePool) -> (u16, String, Option<String>) {
    let Ok(settings) = read_settings() else {
        return (403, "{\"error\":\"no settings\"}".into(), None);
    };
    let Some(identity) = settings.team.clone() else {
        return (403, "{\"error\":\"not in team\"}".into(), None);
    };
    if !identity.is_leader() {
        return (
            404,
            "{\"error\":\"仅团队长可批准入队,请联系团队长\"}".into(),
            None,
        );
    }
    let Ok(join) = serde_json::from_slice::<JoinRequest>(&req.body) else {
        return (400, "{\"error\":\"bad body\"}".into(), None);
    };
    if join.team_id != identity.team_id {
        return (403, "{\"error\":\"team mismatch\"}".into(), None);
    }
    let code_ok = identity
        .pairing_code
        .as_deref()
        .is_some_and(|c| c == join.code.trim());
    if !code_ok {
        return (403, "{\"error\":\"配对码不正确\"}".into(), None);
    }
    // 加进 roster(已在则只更新名字),seq+1 重签
    let roster = match mutate_roster(pool, &identity, |r| {
        match r.members.iter_mut().find(|m| m.member_id == join.member_id) {
            Some(m) => m.name = join.name.clone(),
            None => r.members.push(RosterMember {
                member_id: join.member_id.clone(),
                name: join.name.clone(),
                role: "member".into(),
                view: None, // 默认全队可见(老板拍板的默认透明)
                edit: vec![],
            }),
        }
    })
    .await
    {
        Ok(r) => r,
        Err(e) => return (400, format!("{{\"error\":\"{e}\"}}"), None),
    };
    // 配对码一次性(老板拍板):用过即作废自动换新 —— 防码扩散后被外人入队。
    // 下一位加入需团队长念新码(管理区实时可见)。
    if let Ok(mut s) = read_settings() {
        if let Some(t) = s.team.as_mut() {
            t.pairing_code = Some(super::gen_pairing_code());
            if let Err(e) = write_settings(&s) {
                crate::dlog!("入队后轮换配对码失败: {e}");
            }
        }
    }
    let resp = JoinResponse {
        team_secret: identity.team_secret.clone(),
        roster,
    };
    let json = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
    (200, json, None)
}

// ============================================================================
// 合并与构包(服务端/客户端共用)
// ============================================================================

/// 合并对方的快照 + roster(验签 + seq 更高才收)+ 编辑请求;检测自己被踢;
/// 应用「目标是我」的 pending 编辑(应用成功 → 重建本人快照,让改动立即随响应传出去)。
async fn merge_incoming(
    pool: &SqlitePool,
    identity: &TeamIdentity,
    incoming: &ExchangeBody,
) -> Result<usize, String> {
    if let Some(sr) = &incoming.roster {
        if let Ok(remote) = sr.verify(&identity.team_secret) {
            let local_seq = match store::load_signed_roster(pool).await? {
                Some(local) => local
                    .verify(&identity.team_secret)
                    .map(|r| r.seq)
                    .unwrap_or(-1),
                None => -1,
            };
            if remote.seq > local_seq {
                store::save_signed_roster(pool, sr).await?;
                // 被踢检测:新名单里没有我 → 留一次性通知(身份清理由 take_kicked_notice 的调用方做)
                if remote.find(&identity.member_id).is_none() {
                    store::set_kicked_notice(pool, &remote.team_name).await?;
                }
            }
        }
    }
    let merged = store::merge_snapshots(pool, &incoming.snapshots).await?;
    // 只收本团队的编辑请求(防御:对方缓存里若残留旧团队记录,不让它串进来)
    let my_team_edits: Vec<_> = incoming
        .edits
        .iter()
        .filter(|e| e.team_id == identity.team_id)
        .cloned()
        .collect();
    store::merge_edits(pool, &my_team_edits).await?;
    // 应用目标是我的 pending(权限以本机 roster 为准)
    if let Some(sr) = store::load_signed_roster(pool).await? {
        if let Ok(roster) = sr.verify(&identity.team_secret) {
            let applied = store::apply_my_pending_edits(pool, identity, &roster).await?;
            if applied > 0 {
                store::rebuild_own_snapshot(pool, identity).await?;
                crate::dlog!("应用了 {} 条队友编辑(已重建快照)", applied);
            }
        }
    }
    Ok(merged)
}

/// 构造发给对方的全集包(快照全集 + 本地 roster)。
async fn build_exchange_body(
    pool: &SqlitePool,
    identity: &TeamIdentity,
) -> Result<ExchangeBody, String> {
    Ok(ExchangeBody {
        team_id: identity.team_id.clone(),
        from_member: identity.member_id.clone(),
        snapshots: store::load_all_snapshots(pool).await?,
        roster: store::load_signed_roster(pool).await?,
        edits: store::load_recent_edits(pool).await?,
    })
}

/// 团队长改 roster 的统一入口:读 → 改 → seq+1 → 重签 → 存。返回新签名件。
pub async fn mutate_roster(
    pool: &SqlitePool,
    identity: &TeamIdentity,
    f: impl FnOnce(&mut Roster),
) -> Result<SignedRoster, String> {
    if !identity.is_leader() {
        return Err("仅团队长可修改成员与权限".into());
    }
    let mut roster = match store::load_signed_roster(pool).await? {
        Some(sr) => sr.verify(&identity.team_secret)?,
        None => return Err("本地没有 roster(团队数据异常)".into()),
    };
    f(&mut roster);
    roster.seq += 1;
    roster.updated_at = chrono::Local::now().to_rfc3339();
    let signed = SignedRoster::sign(&roster, &identity.team_secret)?;
    store::save_signed_roster(pool, &signed).await?;
    Ok(signed)
}

// ============================================================================
// 客户端:发现 / 同步轮 / 入队
// ============================================================================

/// 浏览局域网 `timeout_ms` 毫秒,收集 CaseBoard 团队实例。
async fn browse_peers(timeout_ms: u64) -> Result<Vec<PeerAddr>, String> {
    let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mDNS 启动失败: {e}"))?;
    let receiver = mdns
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("mDNS 浏览失败: {e}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut peers: Vec<PeerAddr> = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remain = deadline - now;
        let event = tokio::task::block_in_place(|| receiver.recv_timeout(remain));
        match event {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let get = |k: &str| {
                    info.get_property_val_str(k)
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                };
                let ip = info
                    .get_addresses()
                    .iter()
                    .find(|a| a.is_ipv4())
                    .map(|a| a.to_string());
                if let Some(ip) = ip {
                    let peer = PeerAddr {
                        member_id: get("mid"),
                        role: get("role"),
                        team_id: get("tid"),
                        team_name: get("tname"),
                        ip,
                        port: info.get_port(),
                    };
                    if !peer.team_id.is_empty()
                        && !peers.iter().any(|p| p.member_id == peer.member_id)
                    {
                        peers.push(peer);
                    }
                }
            }
            Ok(_) => {}
            Err(_) => break, // 超时
        }
    }
    let _ = mdns.shutdown();
    Ok(peers)
}

/// 一轮接力同步:重建本人快照 → 找在场队友 → 逐个互换合并。
pub async fn sync_round(pool: &SqlitePool) -> Result<SyncReport, String> {
    let identity = read_settings()?.team.ok_or("未加入团队")?;
    store::rebuild_own_snapshot(pool, &identity).await?;

    let peers: Vec<PeerAddr> = browse_peers(2500)
        .await?
        .into_iter()
        .filter(|p| p.team_id == identity.team_id && p.member_id != identity.member_id)
        .collect();

    let mut report = SyncReport {
        peers_found: peers.len(),
        ..Default::default()
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    for peer in peers {
        match exchange_with(&client, pool, &identity, &peer).await {
            Ok(merged) => {
                report.peers_synced += 1;
                report.snapshots_merged += merged;
            }
            Err(e) => report.errors.push(format!("{}: {e}", peer.member_id)),
        }
    }
    Ok(report)
}

async fn exchange_with(
    client: &reqwest::Client,
    pool: &SqlitePool,
    identity: &TeamIdentity,
    peer: &PeerAddr,
) -> Result<usize, String> {
    let body = build_exchange_body(pool, identity).await?;
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let mac = hmac_hex(&identity.team_secret, json.as_bytes());
    let url = format!("http://{}:{}/exchange", peer.ip, peer.port);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header(AUTH_HEADER, mac)
        .body(json)
        .send()
        .await
        .map_err(|e| format!("连不上: {e}"))?;
    let status = resp.status();
    let resp_auth = resp
        .headers()
        .get(AUTH_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {status}: {}",
            text.chars().take(120).collect::<String>()
        ));
    }
    // 验响应签名(防 LAN 上的冒充响应)
    if hmac_hex(&identity.team_secret, text.as_bytes()) != resp_auth {
        return Err("响应签名校验失败".into());
    }
    let incoming: ExchangeBody =
        serde_json::from_str(&text).map_err(|e| format!("响应解析失败: {e}"))?;
    merge_incoming(pool, identity, &incoming).await
}

/// 「加入团队」第一步:扫描局域网,按团队聚合。
pub async fn discover_teams() -> Result<Vec<DiscoveredTeam>, String> {
    let peers = browse_peers(3000).await?;
    let mut by_team: HashMap<String, DiscoveredTeam> = HashMap::new();
    for p in peers {
        let t = by_team.entry(p.team_id.clone()).or_insert(DiscoveredTeam {
            team_id: p.team_id.clone(),
            team_name: p.team_name.clone(),
            leader_online: false,
            online_members: 0,
        });
        t.online_members += 1;
        if p.role == "leader" {
            t.leader_online = true;
        }
    }
    let mut v: Vec<_> = by_team.into_values().collect();
    v.sort_by(|a, b| a.team_name.cmp(&b.team_name));
    Ok(v)
}

/// 「加入团队」第二步:找到团队长实例,验配对码换 secret + roster,写本机身份。
pub async fn join_team(
    pool: &SqlitePool,
    team_id: &str,
    code: &str,
    my_name: &str,
) -> Result<TeamIdentity, String> {
    let peers = browse_peers(3000).await?;
    let leader = peers
        .iter()
        .find(|p| p.team_id == team_id && p.role == "leader")
        .ok_or("没找到团队长在线的实例 —— 入队需要团队长打开 App,请联系团队长后重试")?;

    let member_id = uuid::Uuid::new_v4().to_string();
    let join = JoinRequest {
        team_id: team_id.to_string(),
        code: code.trim().to_string(),
        member_id: member_id.clone(),
        name: my_name.trim().to_string(),
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("http://{}:{}/join", leader.ip, leader.port);
    let resp = client
        .post(&url)
        .json(&join)
        .send()
        .await
        .map_err(|e| format!("连不上团队长: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        // 透传服务端给的人话错误(配对码不正确等)
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(msg);
    }
    let jr: JoinResponse =
        serde_json::from_str(&text).map_err(|e| format!("入队响应解析失败: {e}"))?;
    let roster = jr.roster.verify(&jr.team_secret)?;

    let identity = TeamIdentity {
        team_id: roster.team_id.clone(),
        team_name: roster.team_name.clone(),
        team_secret: jr.team_secret.clone(),
        member_id,
        my_name: my_name.trim().to_string(),
        role: "member".into(),
        pairing_code: None,
    };
    // 落库 + 落 settings
    store::save_signed_roster(pool, &jr.roster).await?;
    let mut settings = read_settings()?;
    settings.team = Some(identity.clone());
    write_settings(&settings)?;
    store::rebuild_own_snapshot(pool, &identity).await?;
    Ok(identity)
}
