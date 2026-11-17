//! Axum JSON API and the SSE log stream.

use crate::config::{Config, RestartMode};
use crate::engine::Engine;
use crate::logs::{now_ms, LogEntry};
use crate::state::{AppState, ScheduledRestart};
use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::fmt;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/config", get(get_config).post(post_config))
        .route("/check-all", post(check_all))
        .route("/batch/update", post(batch_update))
        .route("/restarts", get(list_restarts).post(schedule_restart))
        .route("/restarts/{id}", delete(cancel_restart))
        .route("/tasks/{id}/check", post(check_task))
        .route("/tasks/{id}/update", post(update_task))
        .route("/logs/stream", get(log_stream))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.into(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}): {}",
            self.code, self.status, self.message
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
                "code": self.code,
            })),
        )
            .into_response()
    }
}

#[derive(Serialize)]
struct ActionResponse {
    accepted: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_count: Option<usize>,
}

#[derive(Serialize)]
struct ConfigMeta {
    config_path: String,
    state_path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    restart_required_for: Vec<&'static str>,
}

#[derive(Serialize)]
struct ConfigResponse {
    config: Value,
    meta: ConfigMeta,
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "MCSM-Auto-Deployer",
        "time": now_ms(),
    }))
}

async fn status(State(state): State<Arc<AppState>>) -> Json<crate::state::StatusResponse> {
    Json(state.build_status())
}

async fn get_config(State(state): State<Arc<AppState>>) -> Result<Json<ConfigResponse>, ApiError> {
    let config = state.config_snapshot();
    build_config_response(&state, &config).map(Json)
}

async fn post_config(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Config>,
) -> Result<Json<ConfigResponse>, ApiError> {
    let _guard = state.config_save_lock.lock().await;
    let old = state.config_snapshot();
    let mut updated = payload;

    // Empty secret fields mean "keep the existing secret". The browser never
    // receives actual keys, so it cannot round-trip them accidentally.
    if updated.mcsm.api_key.trim().is_empty() {
        updated.mcsm.api_key = old.mcsm.api_key.clone();
    }
    if updated.github.token.trim().is_empty() {
        updated.github.token = old.github.token.clone();
    }
    if updated.providers.curseforge_api_key.trim().is_empty() {
        updated.providers.curseforge_api_key = old.providers.curseforge_api_key.clone();
    }
    updated.normalize();
    updated
        .validate()
        .map_err(|error| ApiError::bad_request(format!("配置校验失败: {error}")))?;

    let toml = updated
        .to_toml()
        .map_err(|error| ApiError::internal(format!("无法序列化配置: {error}")))?;
    write_config_file(&state.config_path, toml.as_bytes())
        .await
        .map_err(|error| ApiError::internal(format!("无法保存配置: {error}")))?;

    let restart_required_for = if updated.server.bind != old.server.bind {
        vec!["server.bind"]
    } else {
        Vec::new()
    };
    let task_count = updated.tasks.len();
    *state.config.write() = updated.clone();
    state.sync_runtime_with_config();
    state.notify_config_changed();
    tracing::info!(
        tasks = task_count,
        interval_secs = updated.server.poll_interval_secs,
        "配置已通过 Web 控制台更新"
    );

    let mut response = build_config_response(&state, &updated)
        .map_err(|error| ApiError::internal(error.message))?;
    response.meta.restart_required_for = restart_required_for;
    Ok(Json(response))
}

fn build_config_response(state: &AppState, config: &Config) -> Result<ConfigResponse, ApiError> {
    let mut value = serde_json::to_value(config)
        .map_err(|error| ApiError::internal(format!("无法序列化配置: {error}")))?;
    if let Some(mcsm) = value.get_mut("mcsm").and_then(Value::as_object_mut) {
        let configured = !mcsm
            .get("api_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty();
        mcsm.insert("api_key_set".to_string(), Value::Bool(configured));
        mcsm.insert("api_key".to_string(), Value::String(String::new()));
    }
    if let Some(github) = value.get_mut("github").and_then(Value::as_object_mut) {
        let configured = !github
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty();
        github.insert("token_set".to_string(), Value::Bool(configured));
        github.insert("token".to_string(), Value::String(String::new()));
    }
    if let Some(providers) = value.get_mut("providers").and_then(Value::as_object_mut) {
        let configured = !providers
            .get("curseforge_api_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty();
        providers.insert(
            "curseforge_api_key_set".to_string(),
            Value::Bool(configured),
        );
        providers.insert(
            "curseforge_api_key".to_string(),
            Value::String(String::new()),
        );
    }
    Ok(ConfigResponse {
        config: value,
        meta: ConfigMeta {
            config_path: state.config_path.display().to_string(),
            state_path: state.state_path.display().to_string(),
            restart_required_for: Vec::new(),
        },
    })
}

async fn write_config_file(path: &FsPath, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("无法创建配置目录 {}", parent.display()))?;
        }
    }
    let temp_path = path.with_extension("toml.tmp");
    tokio::fs::write(&temp_path, bytes)
        .await
        .with_context(|| format!("无法写入临时配置 {}", temp_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        if let Err(error) = std::fs::set_permissions(&temp_path, permissions) {
            tracing::warn!(error = %error, "无法将配置文件权限设置为 0600");
        }
    }
    if let Err(error) = tokio::fs::rename(&temp_path, path).await {
        if path.exists() {
            let _ = tokio::fs::remove_file(path).await;
            tokio::fs::rename(&temp_path, path)
                .await
                .with_context(|| format!("无法替换配置文件 {}", path.display()))?;
        } else {
            return Err(error).with_context(|| format!("无法替换配置文件 {}", path.display()));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct BatchUpdateRequest {
    task_ids: Vec<String>,
    restart_at: u64,
    #[serde(default)]
    restart_mode: Option<RestartMode>,
}

async fn batch_update(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BatchUpdateRequest>,
) -> Result<Json<ActionResponse>, ApiError> {
    let mut seen = std::collections::HashSet::new();
    let task_ids: Vec<String> = payload
        .task_ids
        .into_iter()
        .filter(|id| !id.trim().is_empty() && seen.insert(id.clone()))
        .collect();
    if task_ids.is_empty() {
        return Err(ApiError::bad_request("至少选择一个任务"));
    }
    let mut valid = Vec::new();
    for id in &task_ids {
        match state.task_config(id) {
            Some(task) if task.enabled => valid.push(id.clone()),
            Some(_) => {
                return Err(ApiError::conflict(format!("任务「{id}」已停用")));
            }
            None => return Err(ApiError::not_found(format!("任务不存在: {id}"))),
        }
    }
    let mode = payload
        .restart_mode
        .unwrap_or(state.config_snapshot().mcsm.restart_mode);
    let engine = Engine::new(Arc::clone(&state));
    if !engine.spawn_batch(valid.clone(), Some(payload.restart_at), mode) {
        return Err(ApiError::conflict("已有批量部署正在进行"));
    }
    Ok(Json(ActionResponse {
        accepted: true,
        message: format!("已开始批量部署 {} 个任务", valid.len()),
        task_id: None,
        task_count: Some(valid.len()),
    }))
}

#[derive(Debug, Deserialize)]
struct ScheduleRestartRequest {
    daemon_id: String,
    instance_uuid: String,
    #[serde(default)]
    instance_label: Option<String>,
    restart_at: u64,
    #[serde(default)]
    restart_mode: Option<RestartMode>,
    #[serde(default)]
    task_ids: Vec<String>,
}

async fn list_restarts(State(state): State<Arc<AppState>>) -> Json<Vec<ScheduledRestart>> {
    Json(state.restarts_snapshot())
}

async fn schedule_restart(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ScheduleRestartRequest>,
) -> Result<Json<ScheduledRestart>, ApiError> {
    let config = state.config_snapshot();
    let mode = payload.restart_mode.unwrap_or(config.mcsm.restart_mode);
    let mut task_names = Vec::new();
    for id in &payload.task_ids {
        if let Some(task) = state.task_config(id) {
            task_names.push(task.name);
        }
    }
    let restart = state
        .schedule_restart(
            &payload.daemon_id,
            &payload.instance_uuid,
            payload.instance_label.as_deref().unwrap_or_default(),
            payload.restart_at,
            mode,
            payload.task_ids,
            task_names,
        )
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .persist()
        .await
        .map_err(|error| ApiError::internal(format!("保存重启计划失败: {error}")))?;
    tracing::info!(
        restart_id = %restart.id,
        instance = %restart.instance_uuid,
        at = restart.restart_at,
        "已登记统一重启"
    );
    Ok(Json(restart))
}

async fn cancel_restart(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, ApiError> {
    if !state.cancel_restart(&id) {
        return Err(ApiError::not_found("未找到可取消的待重启计划"));
    }
    state
        .persist()
        .await
        .map_err(|error| ApiError::internal(format!("保存重启计划失败: {error}")))?;
    tracing::info!(restart_id = %id, "已取消统一重启");
    Ok(Json(ActionResponse {
        accepted: true,
        message: "已取消统一重启".to_string(),
        task_id: None,
        task_count: None,
    }))
}

async fn check_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, ApiError> {
    let task = state
        .task_config(&id)
        .ok_or_else(|| ApiError::not_found("任务不存在"))?;
    if !task.enabled {
        return Err(ApiError::conflict("任务已停用，请先启用"));
    }
    if state.is_task_busy(&id) {
        return Err(ApiError::conflict("任务正在执行，请稍后再试"));
    }
    let engine = Engine::new(Arc::clone(&state));
    engine.spawn_task(id.clone(), false);
    Ok(Json(ActionResponse {
        accepted: true,
        message: format!("已开始检查「{}」", task.name),
        task_id: Some(id),
        task_count: None,
    }))
}

async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, ApiError> {
    let task = state
        .task_config(&id)
        .ok_or_else(|| ApiError::not_found("任务不存在"))?;
    if !task.enabled {
        return Err(ApiError::conflict("任务已停用，请先启用"));
    }
    if state.is_task_busy(&id) {
        return Err(ApiError::conflict("任务正在执行，请稍后再试"));
    }
    let engine = Engine::new(Arc::clone(&state));
    engine.spawn_task(id.clone(), true);
    Ok(Json(ActionResponse {
        accepted: true,
        message: format!("已开始部署「{}」", task.name),
        task_id: Some(id),
        task_count: None,
    }))
}

async fn check_all(State(state): State<Arc<AppState>>) -> Result<Json<ActionResponse>, ApiError> {
    let engine = Engine::new(Arc::clone(&state));
    if !engine.spawn_check_all() {
        return Err(ApiError::conflict("全局检查已在进行中"));
    }
    let task_count = state
        .config_snapshot()
        .tasks
        .iter()
        .filter(|task| task.enabled)
        .count();
    Ok(Json(ActionResponse {
        accepted: true,
        message: format!("已开始检查 {task_count} 个启用任务"),
        task_id: None,
        task_count: Some(task_count),
    }))
}

async fn log_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = state.logs.subscribe();
    let history = state.logs.snapshot();
    let newest_seq = history.last().map(|entry| entry.seq).unwrap_or(0);
    let stream = async_stream::stream! {
        for entry in history {
            if let Some(event) = log_event(&entry) {
                yield Ok(event);
            }
        }
        loop {
            match receiver.recv().await {
                Ok(entry) if entry.seq > newest_seq => {
                    if let Some(event) = log_event(&entry) {
                        yield Ok(event);
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let entry = LogEntry {
                        seq: 0,
                        timestamp_ms: now_ms(),
                        level: "WARN".to_string(),
                        target: "mcsm_auto_deployer::logs".to_string(),
                        message: format!("日志流消费过慢，已跳过 {skipped} 条"),
                        task_id: None,
                    };
                    if let Some(event) = log_event(&entry) {
                        yield Ok(event);
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn log_event(entry: &LogEntry) -> Option<Event> {
    match Event::default().json_data(entry) {
        Ok(event) => Some(event),
        Err(error) => {
            tracing::warn!(error = %error, "无法序列化 SSE 日志事件");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn secrets_are_masked_in_config_payload() {
        let config = Config::default();
        let value = serde_json::to_value(&config).unwrap_or(Value::Null);
        assert!(value.is_object());
    }
}
