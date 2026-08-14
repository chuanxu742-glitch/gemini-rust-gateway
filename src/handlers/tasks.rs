use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{domain::task::PublicTaskRecord, error::Result, middleware::auth::authorize};

use super::AppState;

#[derive(Debug, Deserialize)]
pub struct TaskListQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    before: Option<u64>,
    #[serde(default)]
    status: Option<String>,
}

fn default_limit() -> usize {
    20
}

pub async fn list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TaskListQuery>,
) -> Result<Json<Value>> {
    authorize(&state.config, &headers)?;
    if let Some(status) = &query.status
        && !crate::domain::task::TERMINAL_STATES.contains(&status.as_str())
        && !matches!(status.as_str(), "queued" | "running")
    {
        return Err(crate::error::GatewayError::InvalidRequest(
            "invalid task status filter".into(),
        ));
    }
    let tasks = state
        .tasks
        .list(query.limit, query.before, query.status)
        .await?;
    let next_before = tasks.last().map(|task| task.created_at);
    Ok(Json(serde_json::json!({
        "object": "list",
        "data": tasks,
        "next_before": next_before,
    })))
}

pub async fn get_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<Value>> {
    authorize(&state.config, &headers)?;
    let record: PublicTaskRecord = state.tasks.get(&task_id).await?;
    Ok(Json(serde_json::to_value(record)?))
}

pub async fn cancel_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<Value>> {
    authorize(&state.config, &headers)?;
    let record: PublicTaskRecord = state.tasks.cancel(&task_id).await?;
    Ok(Json(serde_json::to_value(record)?))
}

#[allow(dead_code)]
fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[allow(dead_code)]
const _: Duration = Duration::from_secs(1);
