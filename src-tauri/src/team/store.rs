//! 团队版存取层:team_snapshots / team_state 表(migration 0020)+ 从 cases 表构建本人快照。
//!
//! 合并规则统一走 [`super::should_replace`](seq 新者胜),本文件不自创第二套判断。

use serde_json::Value;
use sqlx::SqlitePool;

use super::{
    edit_status_rank, should_replace, Roster, SignedRoster, SnapshotCase, SnapshotDate,
    SnapshotEnvelope, SnapshotPayload, TeamEdit, TeamIdentity,
};
use crate::db::cases::{list_cases, Case};

const STATE_ROSTER: &str = "roster";
const STATE_OWN_SEQ: &str = "own_seq";
const STATE_KICKED_NOTICE: &str = "kicked_notice";

// ============================================================================
// team_state k/v
// ============================================================================

pub async fn get_state(pool: &SqlitePool, key: &str) -> Result<Option<String>, String> {
    sqlx::query_scalar::<_, String>("SELECT value FROM team_state WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("读 team_state 失败: {e}"))
}

pub async fn set_state(pool: &SqlitePool, key: &str, value: &str) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO team_state (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .map_err(|e| format!("写 team_state 失败: {e}"))?;
    Ok(())
}

pub async fn load_signed_roster(pool: &SqlitePool) -> Result<Option<SignedRoster>, String> {
    match get_state(pool, STATE_ROSTER).await? {
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("本地 roster 解析失败: {e}")),
        None => Ok(None),
    }
}

pub async fn save_signed_roster(pool: &SqlitePool, sr: &SignedRoster) -> Result<(), String> {
    let s = serde_json::to_string(sr).map_err(|e| e.to_string())?;
    set_state(pool, STATE_ROSTER, &s).await
}

/// 被踢提示:合并 roster 时发现自己被移除 → 写一条一次性通知,前端取走即清。
pub async fn set_kicked_notice(pool: &SqlitePool, team_name: &str) -> Result<(), String> {
    set_state(pool, STATE_KICKED_NOTICE, team_name).await
}

pub async fn take_kicked_notice(pool: &SqlitePool) -> Result<Option<String>, String> {
    let v = get_state(pool, STATE_KICKED_NOTICE).await?;
    if v.is_some() {
        sqlx::query("DELETE FROM team_state WHERE key = ?")
            .bind(STATE_KICKED_NOTICE)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(v)
}

// ============================================================================
// 快照存取与合并
// ============================================================================

pub async fn load_all_snapshots(pool: &SqlitePool) -> Result<Vec<SnapshotEnvelope>, String> {
    let rows = sqlx::query_as::<_, (String, String, i64, String, String)>(
        "SELECT member_id, name, seq, updated_at, payload FROM team_snapshots",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("读 team_snapshots 失败: {e}"))?;
    Ok(rows
        .into_iter()
        .map(
            |(member_id, name, seq, updated_at, payload)| SnapshotEnvelope {
                member_id,
                name,
                seq,
                updated_at,
                payload,
            },
        )
        .collect())
}

/// 合并一批快照(seq 新者胜),返回真正落库的条数。
pub async fn merge_snapshots(
    pool: &SqlitePool,
    incoming: &[SnapshotEnvelope],
) -> Result<usize, String> {
    let mut merged = 0usize;
    for env in incoming {
        let existing =
            sqlx::query_scalar::<_, i64>("SELECT seq FROM team_snapshots WHERE member_id = ?")
                .bind(&env.member_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;
        if !should_replace(existing, env.seq) {
            continue;
        }
        sqlx::query(
            "INSERT INTO team_snapshots (member_id, name, seq, updated_at, payload)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(member_id) DO UPDATE SET
               name = excluded.name, seq = excluded.seq,
               updated_at = excluded.updated_at, payload = excluded.payload",
        )
        .bind(&env.member_id)
        .bind(&env.name)
        .bind(env.seq)
        .bind(&env.updated_at)
        .bind(&env.payload)
        .execute(pool)
        .await
        .map_err(|e| format!("写 team_snapshots 失败: {e}"))?;
        merged += 1;
    }
    Ok(merged)
}

/// 退出/解散/被踢:清空团队数据(身份在 settings.json,由调用方清)。
pub async fn clear_team_data(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("DELETE FROM team_snapshots")
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM team_state")
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    // team_edits 必须一起清(0.3.11 遗漏,0.3.12 修):否则退团再加入新团队,
    // 旧团队的改动记录(含案件名/备注内容)会随接力同步外带给新队友。
    sqlx::query("DELETE FROM team_edits")
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================================
// 编辑请求(Phase 2):合并(状态只升不降)/ 应用 / 撤销
// ============================================================================

const EDITS_CAP: i64 = 500;

/// team_edits 一行的元组形态(sqlx query_as 用;13 列与 EDIT_COLS 顺序一致)。
type EditRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
);

fn edit_from_row(r: EditRow) -> TeamEdit {
    TeamEdit {
        id: r.0,
        team_id: r.1,
        editor_id: r.2,
        editor_name: r.3,
        target_member_id: r.4,
        case_id: r.5,
        case_name: r.6,
        field: r.7,
        value: r.8,
        prev_value: r.9,
        status: r.10,
        created_at: r.11,
        applied_at: r.12,
    }
}

const EDIT_COLS: &str = "id, team_id, editor_id, editor_name, target_member_id, case_id, \
     case_name, field, value, prev_value, status, created_at, applied_at";

pub async fn load_recent_edits(pool: &SqlitePool) -> Result<Vec<TeamEdit>, String> {
    let sql =
        format!("SELECT {EDIT_COLS} FROM team_edits ORDER BY created_at DESC LIMIT {EDITS_CAP}");
    let rows = sqlx::query_as(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("读 team_edits 失败: {e}"))?;
    Ok(rows.into_iter().map(edit_from_row).collect())
}

async fn write_edit(pool: &SqlitePool, e: &TeamEdit) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO team_edits (id, team_id, editor_id, editor_name, target_member_id, case_id,
            case_name, field, value, prev_value, status, created_at, applied_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            prev_value = excluded.prev_value, status = excluded.status,
            applied_at = excluded.applied_at",
    )
    .bind(&e.id)
    .bind(&e.team_id)
    .bind(&e.editor_id)
    .bind(&e.editor_name)
    .bind(&e.target_member_id)
    .bind(&e.case_id)
    .bind(&e.case_name)
    .bind(&e.field)
    .bind(&e.value)
    .bind(&e.prev_value)
    .bind(&e.status)
    .bind(&e.created_at)
    .bind(&e.applied_at)
    .execute(pool)
    .await
    .map_err(|e| format!("写 team_edits 失败: {e}"))?;
    Ok(())
}

/// 合并一批编辑请求:未知 id 收下;已知 id 状态只升不降。返回真正落库条数。
pub async fn merge_edits(pool: &SqlitePool, incoming: &[TeamEdit]) -> Result<usize, String> {
    let mut merged = 0usize;
    for e in incoming {
        if !super::EDITABLE_FIELDS.contains(&e.field.as_str()) {
            continue; // 字段白名单兜底(防未来版本/恶意构造塞进非登记字段)
        }
        let existing =
            sqlx::query_scalar::<_, String>("SELECT status FROM team_edits WHERE id = ?")
                .bind(&e.id)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;
        let should = match existing {
            None => true,
            Some(cur) => edit_status_rank(&e.status) > edit_status_rank(&cur),
        };
        if should {
            write_edit(pool, e).await?;
            merged += 1;
        }
    }
    Ok(merged)
}

/// 新建一条本机发起的编辑请求(pending)。
pub async fn insert_pending_edit(pool: &SqlitePool, e: &TeamEdit) -> Result<(), String> {
    write_edit(pool, e).await
}

/// 应用所有「目标是我」的 pending 编辑:验编辑者权限 → 改 cases / 备注直接生效,
/// 回填 prev_value;无权限或案件不存在 → rejected。返回应用条数(>0 调用方该重建快照)。
pub async fn apply_my_pending_edits(
    pool: &SqlitePool,
    identity: &TeamIdentity,
    roster: &Roster,
) -> Result<usize, String> {
    let sql = format!(
        "SELECT {EDIT_COLS} FROM team_edits WHERE target_member_id = ? AND status = 'pending'"
    );
    let rows = sqlx::query_as(&sql)
        .bind(&identity.member_id)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("读待应用编辑失败: {e}"))?;
    let mut applied = 0usize;
    for row in rows {
        let mut e: TeamEdit = edit_from_row(row);
        let now = chrono::Local::now().to_rfc3339();
        // 权限以**我本机的 roster**为准(团队内信任模型下的最后一道闸)。
        // 备注 = 可见即可写(老板 2026-06-10 拍板"谁都可以写备注");改状态才要编辑权。
        let allowed = match e.field.as_str() {
            "note" => roster.can_view(&e.editor_id, &identity.member_id),
            _ => roster.can_edit(&e.editor_id, &identity.member_id),
        };
        if !allowed {
            e.status = "rejected".into();
            write_edit(pool, &e).await?;
            continue;
        }
        match e.field.as_str() {
            "workflow_status" => {
                // 2026-06-13 老板拍板:团队是只读镜像 + 备注,**不再支持改状态**。
                // EDITABLE_FIELDS 已去掉 workflow_status(新请求在入口就被拒);历史遗留的
                // workflow_status pending 一律 rejected,**绝不回写本地 cases** —— 本地 owner 的
                // 状态(首页 update_workflow_status)才是唯一权威源,曾因团队改状态回写刷掉本地手设(bug C)。
                e.status = "rejected".into();
                write_edit(pool, &e).await?;
            }
            "note" => {
                // 备注是团队层标注,不动所有人案件数据,直接生效
                e.status = "applied".into();
                e.applied_at = Some(now);
                write_edit(pool, &e).await?;
                applied += 1;
            }
            _ => {
                e.status = "rejected".into();
                write_edit(pool, &e).await?;
            }
        }
    }
    Ok(applied)
}

/// 所有人撤销一条已应用的编辑(现仅备注):状态改 reverted,不回写本地 cases(团队不碰状态)。
pub async fn revert_edit(
    pool: &SqlitePool,
    identity: &TeamIdentity,
    edit_id: &str,
) -> Result<(), String> {
    let sql = format!("SELECT {EDIT_COLS} FROM team_edits WHERE id = ?");
    let row = sqlx::query_as(&sql)
        .bind(edit_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("没有这条改动记录")?;
    let mut e: TeamEdit = edit_from_row(row);
    if e.target_member_id != identity.member_id {
        return Err("只能撤销别人对**你的**案件的改动".into());
    }
    if e.status != "applied" {
        return Err("这条改动不在已生效状态,无法撤销".into());
    }
    // 团队只读镜像 + 备注,不碰本地状态:撤销仅改 edit 自身状态(备注隐藏),不回写 cases。
    e.status = "reverted".into();
    write_edit(pool, &e).await
}

// ============================================================================
// 本人快照构建(从 cases 表,登记表粒度)
// ============================================================================

/// 重建本人快照并落库(seq 自增),返回信封。每次同步轮前调用,保证发出去的是最新。
pub async fn rebuild_own_snapshot(
    pool: &SqlitePool,
    identity: &TeamIdentity,
) -> Result<SnapshotEnvelope, String> {
    let cases = list_cases(pool)
        .await
        .map_err(|e| format!("读案件失败: {e}"))?;
    let payload = SnapshotPayload {
        cases: cases.iter().map(snapshot_case_from).collect(),
    };
    let seq = match get_state(pool, STATE_OWN_SEQ).await? {
        Some(s) => s.parse::<i64>().unwrap_or(0) + 1,
        None => 1,
    };
    set_state(pool, STATE_OWN_SEQ, &seq.to_string()).await?;
    let env = SnapshotEnvelope {
        member_id: identity.member_id.clone(),
        name: identity.my_name.clone(),
        seq,
        updated_at: chrono::Local::now().to_rfc3339(),
        payload: serde_json::to_string(&payload).map_err(|e| e.to_string())?,
    };
    merge_snapshots(pool, std::slice::from_ref(&env)).await?;
    Ok(env)
}

/// 单个案件 → 登记表粒度快照条目。**绝不**带文档原文/报告/路径/当事人联系方式。
fn snapshot_case_from(c: &Case) -> SnapshotCase {
    let parties = build_parties(c);
    let mut key_dates: Vec<SnapshotDate> = parse_key_dates(c.agg_key_dates.as_deref());
    if let (Some(at), Some(ty)) = (&c.next_milestone_at, &c.next_milestone_type) {
        key_dates.push(SnapshotDate {
            date: at.clone(),
            event: ty.clone(),
        });
    }
    key_dates.sort_by(|a, b| a.date.cmp(&b.date));
    key_dates.dedup();
    key_dates.truncate(20);
    // 「最新进展」= 时间轴里已发生(≤今天)的最后一件事(案件卡摘要位,老板需求);
    // 全是未来日期/没有时间轴 → None,卡片回退显示一句话概括。
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let latest_event = key_dates
        .iter()
        .rfind(|d| d.date.len() >= 10 && d.date[..10] <= today[..])
        .cloned();

    SnapshotCase {
        id: c.id.clone(),
        name: c.name.clone(),
        case_no: c.agg_case_no.clone().or_else(|| c.case_no.clone()),
        parties,
        case_type: Some(c.case_type.clone()),
        // bug F 修复:workflow_status 为空时**不要**降级取 agg_status_text —— 那是 LLM 的一句话
        // 长描述(可达 200+ 字),会把团队看板状态栏撑成一长句、卡片标题被挤竖排。只在 workflow_status
        // / stage(都是短状态)间取;都没有就 None(看板显示占位),由全局分析补出短状态。
        stage: c.workflow_status.clone().or_else(|| c.stage.clone()),
        status_detail: c.agg_resolution.clone(),
        claim_amount: c.agg_claim_amount,
        key_dates,
        last_activity: Some(format!(
            "更新于 {}",
            c.updated_at.chars().take(10).collect::<String>()
        )),
        summary: c.case_summary.clone(),
        latest_event,
        court: c.agg_court.clone().or_else(|| c.court.clone()),
        cause: c.agg_cause.clone().or_else(|| c.cause.clone()),
        filed_at: c.agg_filed_at.clone(),
        plaintiffs: parse_name_list(c.agg_plaintiffs.as_deref()),
        defendants: parse_name_list(c.agg_defendants.as_deref()),
        third_parties: parse_name_list(c.agg_third_parties.as_deref()),
        execution_total: c.execution_total,
        execution_received: c.execution_received,
        execution_remaining: c.execution_remaining,
    }
}

/// "原告甲、原告乙 vs 被告丙"。聚合字段是 JSON 数组(字符串或对象带 name)。
fn build_parties(c: &Case) -> Option<String> {
    let ps = parse_name_list(c.agg_plaintiffs.as_deref());
    let ds = parse_name_list(c.agg_defendants.as_deref());
    if ps.is_empty() && ds.is_empty() {
        return None;
    }
    Some(format!("{} vs {}", join_or_dash(&ps), join_or_dash(&ds)))
}

fn join_or_dash(v: &[String]) -> String {
    if v.is_empty() {
        "—".into()
    } else {
        v.join("、")
    }
}

fn parse_name_list(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|x| {
            x.as_str()
                .map(String::from)
                .or_else(|| x.get("name").and_then(|n| n.as_str()).map(String::from))
        })
        .filter(|s| !s.trim().is_empty())
        .take(4)
        .collect()
}

/// agg_key_dates: [{date, event, note}]。
fn parse_key_dates(raw: Option<&str>) -> Vec<SnapshotDate> {
    let Some(raw) = raw else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|x| {
            let date = x.get("date")?.as_str()?.trim();
            if date.is_empty() {
                return None;
            }
            let event = x
                .get("event")
                .and_then(|e| e.as_str())
                .unwrap_or("关键日期");
            Some(SnapshotDate {
                date: date.to_string(),
                event: event.to_string(),
            })
        })
        .collect()
}
