//! 案件待办清单(2026-06-13 · case_todos 表)
//!
//! 反馈人「胡彬律师」:每个案件一个手动输入的待办清单,案件详情页增删 + 打钩完成,
//! 首页"待办汇总"跨案件显示未完成项。仿滴答清单 —— 打钩 = done=1(默认隐藏,**不删行**)。

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Todo {
    pub id: String,
    pub case_id: String,
    pub title: String,
    pub done: i64, // 0=未完成 1=已完成
    pub done_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewTodo {
    pub case_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTodo {
    pub title: Option<String>,
    pub done: Option<i64>,
}

/// 跨案件未完成待办(首页汇总用)—— 扁平结构带 case_name,
/// 不用 (Todo, String) tuple(sqlx tuple 按序号读、Todo 按列名读,二者不兼容)。
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct OpenTodoRow {
    pub id: String,
    pub case_id: String,
    pub case_name: String,
    pub title: String,
    pub created_at: String,
}

pub async fn add(pool: &SqlitePool, t: NewTodo) -> Result<Todo, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO case_todos (id, case_id, title) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(&t.case_id)
        .bind(&t.title)
        .execute(pool)
        .await?;

    sqlx::query_as::<_, Todo>("SELECT * FROM case_todos WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await
}

/// 列出某案件全部待办(未完成在前,各自按创建倒序)。
pub async fn list_by_case(pool: &SqlitePool, case_id: &str) -> Result<Vec<Todo>, sqlx::Error> {
    sqlx::query_as::<_, Todo>(
        "SELECT * FROM case_todos WHERE case_id = ? ORDER BY done ASC, created_at DESC",
    )
    .bind(case_id)
    .fetch_all(pool)
    .await
}

/// 跨案件所有未完成待办(首页汇总),按案件分组、组内创建倒序。
pub async fn list_open(pool: &SqlitePool) -> Result<Vec<OpenTodoRow>, sqlx::Error> {
    sqlx::query_as::<_, OpenTodoRow>(
        "SELECT t.id, t.case_id, c.name AS case_name, t.title, t.created_at \
         FROM case_todos t JOIN cases c ON t.case_id = c.id \
         WHERE t.done = 0 \
         ORDER BY c.name ASC, t.created_at DESC",
    )
    .fetch_all(pool)
    .await
}

/// 更新待办:改标题 / 打钩或取消打钩。done=1 时写 done_at,取消时清空。
pub async fn update(pool: &SqlitePool, id: &str, upd: &UpdateTodo) -> Result<u64, sqlx::Error> {
    let r = sqlx::query(
        "UPDATE case_todos SET \
         title = COALESCE(?, title), \
         done = COALESCE(?, done), \
         done_at = CASE \
             WHEN ? = 1 THEN datetime('now') \
             WHEN ? = 0 THEN NULL \
             ELSE done_at END, \
         updated_at = datetime('now') \
         WHERE id = ?",
    )
    .bind(&upd.title)
    .bind(upd.done)
    .bind(upd.done)
    .bind(upd.done)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<u64, sqlx::Error> {
    let r = sqlx::query("DELETE FROM case_todos WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}
