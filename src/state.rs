//! Shared application state. All API handlers and background jobs receive an
//! Arc<AppState>; mutable sections are protected by RwLocks.

use crate::config::{Config, RestartMode, TaskConfig};
use crate::logs::{now_ms, LogBus};
use crate::sources::SourceManager;
use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Verifying,
    Cleaning,
    Uploading,
    Restarting,
    UpToDate,
    UpdateAvailable,
    Success,
    Error,
    Disabled,
}

impl TaskPhase {
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading
                | Self::Verifying
                | Self::Cleaning
                | Self::Uploading
                | Self::Restarting
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadProgress {
    pub downloaded: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStepStatus {
    #[default]
    Pending,
    Active,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    pub key: String,
    pub label: String,
    pub status: TaskStepStatus,
    pub detail: Option<String>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub attempts: u32,
}

impl TaskStep {
    fn new(key: &str, label: &str) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            status: TaskStepStatus::Pending,
            detail: None,
            started_at: None,
            finished_at: None,
            attempts: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct TaskRuntime {
    pub phase: TaskPhase,
    pub current_version: Option<String>,
    pub deployed_asset: Option<String>,
    pub deployed_remote_path: Option<String>,
    pub latest_version: Option<String>,
    pub latest_asset: Option<String>,
    pub latest_release_name: Option<String>,
    pub latest_release_url: Option<String>,
    pub latest_published_at: Option<String>,
    pub update_available: bool,
    pub last_checked_at: Option<u64>,
    pub last_updated_at: Option<u64>,
    pub last_error: Option<String>,
    pub progress: Option<DownloadProgress>,
    pub rate_limit_remaining: Option<u64>,
    pub used_cache: bool,
    pub sha256: Option<String>,
    /// Set when a batch deployment intentionally deferred the instance restart.
    pub restart_deferred: bool,
    /// Human-readable text for the currently executing sub-operation.
    pub current_detail: Option<String>,
    /// Number of MCSM cooldown retries seen during the current deployment.
    pub retry_count: u32,
    /// Internal deployment timeline shown by the task process dialog.
    pub steps: Vec<TaskStep>,
}

impl TaskRuntime {
    /// Reset the timeline at the beginning of a deployment.
    pub fn prepare_steps(&mut self) {
        self.steps = vec![
            TaskStep::new("check", "检查来源"),
            TaskStep::new("download", "下载资产"),
            TaskStep::new("verify", "SHA-256 校验"),
            TaskStep::new("cleanup", "清理旧版"),
            TaskStep::new("credential", "申请上传凭证"),
            TaskStep::new("daemon_upload", "直传 Daemon"),
            TaskStep::new("rename", "重命名文件"),
            TaskStep::new("restart", "重启实例"),
        ];
        self.current_detail = Some("准备开始部署".to_string());
        self.retry_count = 0;
    }

    pub fn start_step(&mut self, key: &str, detail: impl Into<String>) {
        let detail = detail.into();
        self.current_detail = Some(detail.clone());
        if let Some(step) = self.steps.iter_mut().find(|step| step.key == key) {
            if step.status != TaskStepStatus::Skipped {
                step.status = TaskStepStatus::Active;
                if step.started_at.is_none() {
                    step.started_at = Some(now_ms());
                }
            }
            step.detail = Some(detail);
        }
    }

    pub fn finish_step(&mut self, key: &str, detail: impl Into<String>) {
        let detail = detail.into();
        self.current_detail = Some(detail.clone());
        if let Some(step) = self.steps.iter_mut().find(|step| step.key == key) {
            step.status = TaskStepStatus::Done;
            if step.started_at.is_none() {
                step.started_at = Some(now_ms());
            }
            step.finished_at = Some(now_ms());
            step.detail = Some(detail);
        }
    }

    pub fn fail_step(&mut self, key: &str, detail: impl Into<String>) {
        let detail = detail.into();
        self.current_detail = Some(detail.clone());
        if let Some(step) = self.steps.iter_mut().find(|step| step.key == key) {
            step.status = TaskStepStatus::Failed;
            if step.started_at.is_none() {
                step.started_at = Some(now_ms());
            }
            step.finished_at = Some(now_ms());
            step.detail = Some(detail);
        }
    }

    pub fn skip_step(&mut self, key: &str, detail: impl Into<String>) {
        let detail = detail.into();
        self.current_detail = Some(detail.clone());
        if let Some(step) = self.steps.iter_mut().find(|step| step.key == key) {
            step.status = TaskStepStatus::Skipped;
            step.finished_at = Some(now_ms());
            step.detail = Some(detail);
        }
    }

    pub fn retry_step(&mut self, key: &str, detail: impl Into<String>, attempt: u32) {
        let detail = detail.into();
        self.current_detail = Some(detail.clone());
        self.retry_count = self.retry_count.saturating_add(1);
        if let Some(step) = self.steps.iter_mut().find(|step| step.key == key) {
            if step.status == TaskStepStatus::Pending {
                step.status = TaskStepStatus::Active;
                step.started_at = Some(now_ms());
            }
            step.attempts = step.attempts.max(attempt);
            step.detail = Some(detail);
        }
    }

    /// Mark whichever step is currently active as failed. Used by the outer
    /// error handler so a failure is always visible in the process timeline.
    pub fn fail_active_step(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        self.current_detail = Some(detail.clone());
        if let Some(step) = self
            .steps
            .iter_mut()
            .find(|step| step.status == TaskStepStatus::Active)
        {
            step.status = TaskStepStatus::Failed;
            step.finished_at = Some(now_ms());
            step.detail = Some(detail);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: String,
    pub name: String,
    pub category: String,
    pub category_label: String,
    pub enabled: bool,
    pub source_type: String,
    pub source: String,
    pub source_label: String,
    pub asset_regex: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub cleanup_old: bool,
    pub cleanup_regex: String,
    pub mcsm_daemon_id: String,
    pub mcsm_instance_uuid: String,
    pub mcsm_target_path: String,
    pub restart: bool,
    #[serde(flatten)]
    pub runtime: TaskRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledRestartStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledRestart {
    pub id: String,
    pub daemon_id: String,
    pub instance_uuid: String,
    pub instance_label: String,
    pub restart_at: u64,
    pub restart_mode: RestartMode,
    pub created_at: u64,
    pub status: ScheduledRestartStatus,
    #[serde(default)]
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub task_names: Vec<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub executed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemStatus {
    pub version: &'static str,
    pub started_at: u64,
    pub uptime_secs: u64,
    pub now: u64,
    pub poll_interval_secs: u64,
    pub next_check_at: Option<u64>,
    pub last_check_at: Option<u64>,
    pub checking_all: bool,
    pub batch_active: bool,
    pub scheduled_restart_count: usize,
    pub task_count: usize,
    pub enabled_count: usize,
    pub update_count: usize,
    pub busy_count: usize,
    pub mcsm_configured: bool,
    pub github_authenticated: bool,
    pub config_path: String,
    pub state_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusResponse {
    pub system: SystemStatus,
    pub tasks: Vec<TaskView>,
    pub scheduled_restarts: Vec<ScheduledRestart>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    #[serde(default)]
    tasks: HashMap<String, PersistedTask>,
    #[serde(default)]
    restarts: Vec<ScheduledRestart>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedTask {
    current_version: Option<String>,
    deployed_asset: Option<String>,
    #[serde(default)]
    deployed_remote_path: Option<String>,
    updated_at: Option<u64>,
}

pub struct AppState {
    pub config: RwLock<Config>,
    pub runtime: RwLock<HashMap<String, TaskRuntime>>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    pub logs: Arc<LogBus>,
    pub sources: SourceManager,
    pub started_at: Instant,
    pub started_at_ms: u64,
    pub next_check_at: RwLock<Option<u64>>,
    pub last_check_at: RwLock<Option<u64>>,
    pub check_all_active: AtomicBool,
    pub batch_active: AtomicBool,
    pub scheduled_restarts: RwLock<Vec<ScheduledRestart>>,
    restart_sequence: AtomicU64,
    config_changed: watch::Sender<u64>,
    pub config_save_lock: Mutex<()>,
    state_save_lock: Mutex<()>,
    task_locks: parking_lot::Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl AppState {
    pub fn new(config: Config, config_path: PathBuf, logs: Arc<LogBus>) -> Result<Arc<Self>> {
        let state_path = PathBuf::from(&config.server.state_file);
        if let Some(parent) = state_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("无法创建状态文件目录 {}", parent.display()))?;
            }
        }
        let persisted = load_persisted(&state_path);
        let runtime = build_runtime_map(&config, &persisted);
        let scheduled_restarts = build_restart_list(&persisted);
        let (config_changed, _) = watch::channel(0_u64);
        let sources = SourceManager::new().context("无法初始化内容平台客户端")?;
        let now = now_ms();
        Ok(Arc::new(Self {
            config: RwLock::new(config),
            runtime: RwLock::new(runtime),
            config_path,
            state_path,
            logs,
            sources,
            started_at: Instant::now(),
            started_at_ms: now,
            next_check_at: RwLock::new(None),
            last_check_at: RwLock::new(None),
            check_all_active: AtomicBool::new(false),
            batch_active: AtomicBool::new(false),
            scheduled_restarts: RwLock::new(scheduled_restarts),
            restart_sequence: AtomicU64::new(1),
            config_changed,
            config_save_lock: Mutex::new(()),
            state_save_lock: Mutex::new(()),
            task_locks: parking_lot::Mutex::new(HashMap::new()),
        }))
    }

    pub fn config_snapshot(&self) -> Config {
        self.config.read().clone()
    }

    pub fn task_config(&self, id: &str) -> Option<TaskConfig> {
        self.config
            .read()
            .tasks
            .iter()
            .find(|task| task.id == id)
            .cloned()
    }

    pub fn task_lock(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.task_locks.lock();
        locks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub fn update_runtime<F>(&self, id: &str, update: F)
    where
        F: FnOnce(&mut TaskRuntime),
    {
        let mut runtimes = self.runtime.write();
        let runtime = runtimes.entry(id.to_string()).or_default();
        update(runtime);
    }

    pub fn runtime_snapshot(&self, id: &str) -> Option<TaskRuntime> {
        self.runtime.read().get(id).cloned()
    }

    pub fn is_task_busy(&self, id: &str) -> bool {
        self.runtime
            .read()
            .get(id)
            .map(|runtime| runtime.phase.is_busy())
            .unwrap_or(false)
    }

    pub fn config_changed(&self) -> watch::Receiver<u64> {
        self.config_changed.subscribe()
    }

    pub fn notify_config_changed(&self) {
        let _ = self.config_changed.send(now_ms());
    }

    /// Add runtimes for new tasks and drop removed ones, preserving deployment
    /// history across hot reloads.
    pub fn sync_runtime_with_config(&self) {
        let config = self.config.read().clone();
        let mut runtimes = self.runtime.write();
        let mut seen = std::collections::HashSet::new();
        for task in &config.tasks {
            seen.insert(task.id.clone());
            let runtime = runtimes.entry(task.id.clone()).or_default();
            if !task.enabled && !runtime.phase.is_busy() {
                runtime.phase = TaskPhase::Disabled;
            } else if task.enabled && runtime.phase == TaskPhase::Disabled {
                runtime.phase = TaskPhase::Idle;
            }
        }
        runtimes.retain(|id, _| seen.contains(id));
    }

    pub fn build_status(&self) -> StatusResponse {
        let config = self.config.read().clone();
        let runtimes = self.runtime.read().clone();
        let now = now_ms();
        let mut tasks = Vec::with_capacity(config.tasks.len());
        let mut update_count = 0_usize;
        let mut enabled_count = 0_usize;
        let mut busy_count = 0_usize;

        for task in &config.tasks {
            let runtime = runtimes.get(&task.id).cloned().unwrap_or_default();
            if task.enabled {
                enabled_count = enabled_count.saturating_add(1);
            }
            if runtime.update_available && task.enabled {
                update_count = update_count.saturating_add(1);
            }
            if runtime.phase.is_busy() {
                busy_count = busy_count.saturating_add(1);
            }
            let category = task.effective_category();
            tasks.push(TaskView {
                id: task.id.clone(),
                name: task.name.clone(),
                category: category.as_str().to_string(),
                category_label: category.label().to_string(),
                enabled: task.enabled,
                source_type: task.source_type.as_str().to_string(),
                source: task.source_id().to_string(),
                source_label: task.source_type.label().to_string(),
                asset_regex: task.asset_regex.clone(),
                game_versions: task.game_versions.clone(),
                loaders: task.loaders.clone(),
                cleanup_old: task.cleanup_old,
                cleanup_regex: task.cleanup_regex.clone(),
                mcsm_daemon_id: task.mcsm_daemon_id.clone(),
                mcsm_instance_uuid: task.mcsm_instance_uuid.clone(),
                mcsm_target_path: task.mcsm_target_path.clone(),
                restart: task.restart,
                runtime,
            });
        }

        let scheduled = self.scheduled_restarts.read().clone();
        let scheduled_restart_count = scheduled
            .iter()
            .filter(|restart| restart.status == ScheduledRestartStatus::Pending)
            .count();
        let status = SystemStatus {
            version: env!("CARGO_PKG_VERSION"),
            started_at: self.started_at_ms,
            uptime_secs: self.started_at.elapsed().as_secs(),
            now,
            poll_interval_secs: config.server.poll_interval_secs,
            next_check_at: *self.next_check_at.read(),
            last_check_at: *self.last_check_at.read(),
            checking_all: self.check_all_active.load(Ordering::Relaxed),
            batch_active: self.batch_active.load(Ordering::Relaxed),
            scheduled_restart_count,
            task_count: config.tasks.len(),
            enabled_count,
            update_count,
            busy_count,
            mcsm_configured: is_real_secret(&config.mcsm.api_key),
            github_authenticated: is_real_secret(&config.github.token),
            config_path: self.config_path.display().to_string(),
            state_path: self.state_path.display().to_string(),
        };

        StatusResponse {
            system: status,
            tasks,
            scheduled_restarts: scheduled,
        }
    }

    pub fn schedule_restart(
        &self,
        daemon_id: &str,
        instance_uuid: &str,
        instance_label: &str,
        restart_at: u64,
        restart_mode: RestartMode,
        task_ids: Vec<String>,
        task_names: Vec<String>,
    ) -> Result<ScheduledRestart> {
        if daemon_id.trim().is_empty() || instance_uuid.trim().is_empty() {
            return Err(anyhow::anyhow!("daemon_id 和 instance_uuid 不能为空"));
        }
        if restart_at <= now_ms().saturating_add(2_000) {
            return Err(anyhow::anyhow!("重启时间必须至少晚于当前时间 2 秒"));
        }
        let now = now_ms();
        let seq = self.restart_sequence.fetch_add(1, Ordering::Relaxed);
        let restart = ScheduledRestart {
            id: format!("rst-{now}-{seq}"),
            daemon_id: daemon_id.trim().to_string(),
            instance_uuid: instance_uuid.trim().to_string(),
            instance_label: if instance_label.trim().is_empty() {
                instance_uuid.trim().to_string()
            } else {
                instance_label.trim().to_string()
            },
            restart_at,
            restart_mode,
            created_at: now,
            status: ScheduledRestartStatus::Pending,
            task_ids,
            task_names,
            last_error: None,
            executed_at: None,
        };
        {
            let mut restarts = self.scheduled_restarts.write();
            // One pending restart per instance: repeated batch deployments or
            // manual scheduling coalesce into a single unified restart.
            restarts.retain(|existing| {
                !(existing.status == ScheduledRestartStatus::Pending
                    && existing.daemon_id == restart.daemon_id
                    && existing.instance_uuid == restart.instance_uuid)
            });
            restarts.push(restart.clone());
            prune_restart_history(&mut restarts);
        }
        Ok(restart)
    }

    pub fn cancel_restart(&self, id: &str) -> bool {
        let mut restarts = self.scheduled_restarts.write();
        for restart in restarts.iter_mut() {
            if restart.id == id && restart.status == ScheduledRestartStatus::Pending {
                restart.status = ScheduledRestartStatus::Cancelled;
                restart.executed_at = Some(now_ms());
                return true;
            }
        }
        false
    }

    pub fn restarts_snapshot(&self) -> Vec<ScheduledRestart> {
        self.scheduled_restarts.read().clone()
    }

    /// Mark due restarts as Running atomically and return them. This prevents a
    /// second scheduler tick from executing the same restart twice.
    pub fn take_due_restarts(&self, now: u64) -> Vec<ScheduledRestart> {
        let mut restarts = self.scheduled_restarts.write();
        let mut due = Vec::new();
        for restart in restarts.iter_mut() {
            if restart.status == ScheduledRestartStatus::Pending && restart.restart_at <= now {
                restart.status = ScheduledRestartStatus::Running;
                due.push(restart.clone());
            }
        }
        due
    }

    pub fn finish_restart(&self, id: &str, error: Option<String>) {
        let mut task_ids = Vec::new();
        let success = error.is_none();
        {
            let mut restarts = self.scheduled_restarts.write();
            for restart in restarts.iter_mut() {
                if restart.id != id {
                    continue;
                }
                restart.executed_at = Some(now_ms());
                match &error {
                    Some(message) => {
                        restart.status = ScheduledRestartStatus::Failed;
                        restart.last_error = Some(message.clone());
                    }
                    None => {
                        restart.status = ScheduledRestartStatus::Done;
                        restart.last_error = None;
                        task_ids = restart.task_ids.clone();
                    }
                }
                break;
            }
        }
        if success {
            for task_id in task_ids {
                self.update_runtime(&task_id, |runtime| {
                    runtime.restart_deferred = false;
                });
            }
        }
    }

    pub async fn persist(&self) -> Result<()> {
        let _guard = self.state_save_lock.lock().await;
        let runtimes = self.runtime.read().clone();
        let mut persisted = PersistedState {
            restarts: self.scheduled_restarts.read().clone(),
            ..PersistedState::default()
        };
        for (id, runtime) in runtimes {
            if runtime.current_version.is_some() {
                persisted.tasks.insert(
                    id,
                    PersistedTask {
                        current_version: runtime.current_version,
                        deployed_asset: runtime.deployed_asset,
                        deployed_remote_path: runtime.deployed_remote_path,
                        updated_at: runtime.last_updated_at,
                    },
                );
            }
        }
        let serialized = serde_json::to_vec_pretty(&persisted).context("无法序列化部署状态")?;
        if let Some(parent) = self.state_path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("无法创建状态文件目录 {}", parent.display()))?;
            }
        }
        let temp_path = self.state_path.with_extension("json.tmp");
        tokio::fs::write(&temp_path, serialized)
            .await
            .with_context(|| format!("无法写入状态文件 {}", temp_path.display()))?;
        if let Err(error) = tokio::fs::rename(&temp_path, &self.state_path).await {
            // Windows cannot rename over an existing file. Remove and retry.
            if self.state_path.exists() {
                let _ = tokio::fs::remove_file(&self.state_path).await;
                tokio::fs::rename(&temp_path, &self.state_path)
                    .await
                    .with_context(|| format!("无法替换状态文件 {}", self.state_path.display()))?;
            } else {
                return Err(error)
                    .with_context(|| format!("无法写入状态文件 {}", self.state_path.display()));
            }
        }
        Ok(())
    }
}

fn build_restart_list(persisted: &PersistedState) -> Vec<ScheduledRestart> {
    let mut restarts = persisted.restarts.clone();
    // A crash may have left a restart in Running state. Put it back in the
    // queue so it is retried by the restart scheduler.
    for restart in &mut restarts {
        if restart.status == ScheduledRestartStatus::Running {
            restart.status = ScheduledRestartStatus::Pending;
        }
    }
    prune_restart_history(&mut restarts);
    restarts
}

fn prune_restart_history(restarts: &mut Vec<ScheduledRestart>) {
    let terminal = restarts
        .iter()
        .filter(|restart| {
            !matches!(
                restart.status,
                ScheduledRestartStatus::Pending | ScheduledRestartStatus::Running
            )
        })
        .count();
    if terminal <= 30 {
        return;
    }
    let mut to_remove = terminal.saturating_sub(30);
    let mut index = 0_usize;
    while index < restarts.len() && to_remove > 0 {
        if matches!(
            restarts[index].status,
            ScheduledRestartStatus::Pending | ScheduledRestartStatus::Running
        ) {
            index += 1;
            continue;
        }
        restarts.remove(index);
        to_remove -= 1;
    }
}

fn load_persisted(path: &Path) -> PersistedState {
    let Ok(raw) = std::fs::read(path) else {
        return PersistedState::default();
    };
    serde_json::from_slice(&raw).unwrap_or_else(|error| {
        tracing::warn!(file = %path.display(), error = %error, "部署状态文件无法解析，已使用空状态");
        PersistedState::default()
    })
}

fn build_runtime_map(config: &Config, persisted: &PersistedState) -> HashMap<String, TaskRuntime> {
    let mut map = HashMap::new();
    for task in &config.tasks {
        let mut runtime = TaskRuntime::default();
        if let Some(saved) = persisted.tasks.get(&task.id) {
            runtime.current_version = saved.current_version.clone();
            runtime.deployed_asset = saved.deployed_asset.clone();
            runtime.deployed_remote_path = saved.deployed_remote_path.clone();
            runtime.last_updated_at = saved.updated_at;
        }
        if !task.enabled {
            runtime.phase = TaskPhase::Disabled;
        }
        map.insert(task.id.clone(), runtime);
    }
    map
}

pub fn is_real_secret(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && !trimmed.starts_with("YOUR_")
}
