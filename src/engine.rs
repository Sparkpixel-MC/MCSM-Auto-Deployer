//! Deployment engine: check → download → verify → clean → upload → restart.
//!
//! All long-running operations are spawned by the API layer so that closing the
//! browser cannot cancel a deployment halfway through. The engine serialises
//! operations for the same task with a per-task async mutex.

use crate::config::{RestartMode, TaskConfig};
use crate::logs::now_ms;
use crate::mcsm::{McsmClient, McsmEvent, McsmEventKind, McsmEventSink};
use crate::sources::ResolvedArtifact;
use crate::state::{AppState, DownloadProgress, TaskPhase};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

pub struct Engine {
    state: Arc<AppState>,
}

impl Engine {
    pub fn new(state: Arc<AppState>) -> Arc<Self> {
        Arc::new(Self { state })
    }

    /// Spawn a one-off operation. Lifecycle changes are visible through
    /// /api/status; errors are logged to the SSE console.
    pub fn spawn_task(self: &Arc<Self>, id: String, deploy: bool) {
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            let _ = engine.run_task(&id, deploy).await;
        });
    }

    /// Start the unified restart queue watcher. It runs independently from the
    /// source polling timer so a scheduled restart happens even if no check is
    /// due.
    pub fn spawn_restart_scheduler(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            tracing::info!("统一重启调度器已启动");
            loop {
                sleep(Duration::from_secs(2)).await;
                engine.process_due_restarts().await;
            }
        });
    }

    /// Start the scheduled provider check loop. Config edits reset the timer.
    pub fn spawn_scheduler(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            let state = Arc::clone(&engine.state);
            let mut config_rx = state.config_changed();
            let mut first_run = true;
            tracing::info!("后台轮询调度器已启动");
            loop {
                let config = state.config_snapshot();
                let interval = Duration::from_secs(config.server.poll_interval_secs.max(30));
                let sleep_for = if first_run {
                    Duration::from_secs(4)
                } else {
                    interval
                };
                *state.next_check_at.write() =
                    Some(now_ms().saturating_add(sleep_for.as_millis() as u64));
                tracing::debug!(
                    seconds = sleep_for.as_secs(),
                    "调度器进入等待，下一次自动检查已排期"
                );

                tokio::select! {
                    _ = sleep(sleep_for) => {
                        first_run = false;
                        engine.run_all_checks().await;
                    }
                    result = config_rx.changed() => {
                        if result.is_err() {
                            tracing::warn!("配置监听通道已关闭，调度器停止");
                            break;
                        }
                        first_run = false;
                        tracing::info!("配置已热更新，轮询计时器已重置");
                    }
                }
            }
        });
    }

    /// Spawn a check of every enabled task. Returns false when a global check
    /// is already in progress.
    pub fn spawn_check_all(self: &Arc<Self>) -> bool {
        if self
            .state
            .check_all_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            engine.run_all_checks_inner().await;
        });
        true
    }

    pub async fn run_all_checks(&self) {
        if self
            .state
            .check_all_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::warn!("全局检查已在进行中，本次调度跳过");
            return;
        }
        self.run_all_checks_inner().await;
    }

    async fn run_all_checks_inner(&self) {
        let started = Instant::now();
        *self.state.last_check_at.write() = Some(now_ms());
        tracing::info!("开始全局版本检查");
        let tasks: Vec<TaskConfig> = self
            .state
            .config_snapshot()
            .tasks
            .into_iter()
            .filter(|task| task.enabled)
            .collect();
        for task in tasks {
            if self.state.is_task_busy(&task.id) {
                tracing::warn!(task_id = %task.id, "任务正在执行，跳过本轮自动检查");
                continue;
            }
            if let Err(error) = self.run_task(&task.id, false).await {
                tracing::warn!(
                    task_id = %task.id,
                    error = %format!("{error:#}"),
                    "自动检查失败"
                );
            }
        }
        self.state.check_all_active.store(false, Ordering::SeqCst);
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "全局版本检查完成"
        );
    }

    pub async fn run_task(&self, id: &str, deploy: bool) -> Result<()> {
        self.run_task_with_options(id, deploy, false).await
    }

    /// Run a group of updates without restarting after each task. Once all
    /// tasks have finished (or failed), one scheduled restart is registered per
    /// affected instance. The restart is never scheduled before the requested
    /// time, and is delayed slightly if the batch is still running.
    pub fn spawn_batch(
        self: &Arc<Self>,
        task_ids: Vec<String>,
        restart_at: Option<u64>,
        restart_mode: RestartMode,
    ) -> bool {
        if self
            .state
            .batch_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            engine
                .run_batch_inner(task_ids, restart_at, restart_mode)
                .await;
        });
        true
    }

    async fn run_batch_inner(
        &self,
        task_ids: Vec<String>,
        restart_at: Option<u64>,
        restart_mode: RestartMode,
    ) {
        let started = Instant::now();
        tracing::info!(tasks = task_ids.len(), "批量部署开始");
        let mut affected: BTreeMap<(String, String), (String, Vec<String>, Vec<String>)> =
            BTreeMap::new();
        let mut succeeded = 0_usize;
        let mut failed = 0_usize;
        for task_id in &task_ids {
            let Some(task) = self.state.task_config(task_id) else {
                tracing::warn!(task_id = %task_id, "批量部署跳过不存在的任务");
                failed = failed.saturating_add(1);
                continue;
            };
            if !task.enabled {
                tracing::warn!(task_id = %task_id, "批量部署跳过已停用任务");
                continue;
            }
            match self.run_task_with_options(task_id, true, true).await {
                Ok(()) => {
                    succeeded = succeeded.saturating_add(1);
                    let key = (task.mcsm_daemon_id.clone(), task.mcsm_instance_uuid.clone());
                    let entry = affected
                        .entry(key)
                        .or_insert_with(|| (task.name.clone(), Vec::new(), Vec::new()));
                    entry.1.push(task.id.clone());
                    entry.2.push(task.name.clone());
                }
                Err(error) => {
                    failed = failed.saturating_add(1);
                    tracing::error!(
                        task_id = %task_id,
                        error = %format!("{error:#}"),
                        "批量部署中的任务失败"
                    );
                }
            }
        }

        if let Some(requested_at) = restart_at {
            let effective_at = requested_at.max(now_ms().saturating_add(5_000));
            if effective_at > requested_at {
                tracing::warn!(
                    requested_at,
                    effective_at,
                    "批量部署结束时已接近/超过计划重启时间，重启时间顺延 5 秒"
                );
            }
            for ((daemon_id, instance_uuid), (label, ids, names)) in affected {
                let label = if names.len() > 1 {
                    format!("{} 等 {} 个任务", names[0], names.len())
                } else {
                    label
                };
                match self.state.schedule_restart(
                    &daemon_id,
                    &instance_uuid,
                    &label,
                    effective_at,
                    restart_mode,
                    ids,
                    names,
                ) {
                    Ok(restart) => {
                        tracing::info!(
                            restart_id = %restart.id,
                            instance = %instance_uuid,
                            at = effective_at,
                            "批量部署完成，已登记统一重启"
                        );
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "登记统一重启失败");
                    }
                }
            }
            if let Err(error) = self.state.persist().await {
                tracing::warn!(error = %error, "统一重启计划持久化失败");
            }
        }

        self.state.batch_active.store(false, Ordering::SeqCst);
        tracing::info!(
            succeeded,
            failed,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "批量部署结束"
        );
    }

    async fn process_due_restarts(&self) {
        let due = self.state.take_due_restarts(now_ms());
        if due.is_empty() {
            return;
        }
        let config = self.state.config_snapshot();
        let mcsm = match McsmClient::new(&config.mcsm) {
            Ok(client) => client,
            Err(error) => {
                for restart in &due {
                    tracing::error!(
                        restart_id = %restart.id,
                        error = %error,
                        "统一重启失败：MCSM 客户端初始化失败"
                    );
                    self.state
                        .finish_restart(&restart.id, Some(format!("{error:#}")));
                }
                let _ = self.state.persist().await;
                return;
            }
        };
        for restart in due {
            tracing::info!(
                restart_id = %restart.id,
                instance = %restart.instance_uuid,
                mode = restart.restart_mode.as_str(),
                "执行统一重启"
            );
            match mcsm
                .restart_instance_by(
                    &restart.daemon_id,
                    &restart.instance_uuid,
                    restart.restart_mode,
                )
                .await
            {
                Ok(()) => {
                    tracing::info!(restart_id = %restart.id, "统一重启指令已发送");
                    self.state.finish_restart(&restart.id, None);
                }
                Err(error) => {
                    tracing::error!(
                        restart_id = %restart.id,
                        error = %format!("{error:#}"),
                        "统一重启失败"
                    );
                    self.state
                        .finish_restart(&restart.id, Some(format!("{error:#}")));
                }
            }
        }
        if let Err(error) = self.state.persist().await {
            tracing::warn!(error = %error, "统一重启状态持久化失败");
        }
    }

    /// suppress_restart is used by batch deployments so that one scheduled
    /// instance restart can happen after all files are in place.
    pub async fn run_task_with_options(
        &self,
        id: &str,
        deploy: bool,
        suppress_restart: bool,
    ) -> Result<()> {
        let task = self
            .state
            .task_config(id)
            .ok_or_else(|| anyhow!("任务不存在: {id}"))?;
        if !task.enabled {
            self.state.update_runtime(id, |runtime| {
                runtime.phase = TaskPhase::Disabled;
            });
            bail!("任务「{}」已被停用", task.name);
        }
        let lock = self.state.task_lock(id);
        let _guard = lock
            .try_lock()
            .map_err(|_| anyhow!("任务「{}」正在执行其他操作", task.name))?;

        let result = self.execute_task(&task, deploy, suppress_restart).await;
        if let Err(error) = &result {
            let detail = format!("{error:#}");
            self.state.update_runtime(id, |runtime| {
                runtime.phase = TaskPhase::Error;
                runtime.last_error = Some(detail.clone());
                runtime.progress = None;
                runtime.fail_active_step(detail.clone());
            });
            tracing::error!(
                task_id = %id,
                task = %task.name,
                error = %detail,
                "任务执行失败"
            );
        }
        result
    }

    /// Convert low-level MCSM client events into task runtime steps and SSE
    /// log lines. The client never needs to know about AppState or task IDs.
    fn mcsm_event_sink(&self, task_id: &str) -> McsmEventSink {
        let state = Arc::clone(&self.state);
        let id = task_id.to_string();
        Arc::new(move |event: McsmEvent| {
            let detail = event.message.clone();
            state.update_runtime(&id, |runtime| match event.kind {
                McsmEventKind::Started => runtime.start_step(event.step, detail.clone()),
                McsmEventKind::Finished => runtime.finish_step(event.step, detail.clone()),
                McsmEventKind::Failed => runtime.fail_step(event.step, detail.clone()),
                McsmEventKind::Retry => {
                    runtime.retry_step(event.step, detail.clone(), event.attempt)
                }
                McsmEventKind::Info => runtime.current_detail = Some(detail.clone()),
            });
            match event.kind {
                McsmEventKind::Failed => tracing::error!(
                    task_id = %id,
                    step = event.step,
                    "{}",
                    event.message
                ),
                McsmEventKind::Retry => tracing::warn!(
                    task_id = %id,
                    step = event.step,
                    attempt = event.attempt,
                    "{}",
                    event.message
                ),
                _ => tracing::info!(
                    task_id = %id,
                    step = event.step,
                    "{}",
                    event.message
                ),
            }
        })
    }

    async fn execute_task(
        &self,
        task: &TaskConfig,
        deploy: bool,
        suppress_restart: bool,
    ) -> Result<()> {
        let config = self.state.config_snapshot();

        self.state.update_runtime(&task.id, |runtime| {
            runtime.phase = TaskPhase::Checking;
            runtime.last_error = None;
            runtime.progress = None;
            runtime.sha256 = None;
            runtime.prepare_steps();
            runtime.start_step("check", "正在解析版本来源");
        });
        tracing::info!(
            task_id = %task.id,
            provider = task.source_type.label(),
            source = task.source_id(),
            deploy,
            "开始检查版本来源"
        );

        let artifact = self
            .state
            .sources
            .resolve(task, &config)
            .await
            .with_context(|| {
                format!(
                    "解析 {} 来源「{}」失败",
                    task.source_type.label(),
                    task.source_id()
                )
            })?;
        let latest_version = artifact.version.clone();
        let latest_asset = artifact.asset_name.clone();
        let update_available = self
            .state
            .runtime_snapshot(&task.id)
            .and_then(|runtime| runtime.current_version)
            .as_deref()
            != Some(latest_version.as_str());

        self.state.update_runtime(&task.id, |runtime| {
            runtime.latest_version = Some(latest_version.clone());
            runtime.latest_asset = Some(latest_asset.clone());
            runtime.latest_release_name = Some(artifact.release_name.clone());
            runtime.latest_release_url = artifact.release_url.clone();
            runtime.latest_published_at = artifact.published_at.clone();
            runtime.last_checked_at = Some(now_ms());
            runtime.update_available = update_available;
            runtime.rate_limit_remaining = artifact.rate_limit_remaining;
            runtime.used_cache = artifact.from_cache;
            runtime.last_error = None;
            runtime.finish_step(
                "check",
                format!("已解析 {} · {}", latest_version, latest_asset),
            );
        });
        tracing::info!(
            task_id = %task.id,
            provider = task.source_type.label(),
            version = %latest_version,
            asset = %latest_asset,
            from_cache = artifact.from_cache,
            update_available,
            "版本检查完成"
        );

        if !deploy {
            self.state.update_runtime(&task.id, |runtime| {
                runtime.phase = if runtime.update_available {
                    TaskPhase::UpdateAvailable
                } else {
                    TaskPhase::UpToDate
                };
            });
            return Ok(());
        }

        let mcsm = McsmClient::new(&config.mcsm)
            .context("MCSM 客户端初始化失败")?
            .with_events(self.mcsm_event_sink(&task.id));
        let temp_dir = tempfile::Builder::new()
            .prefix("mcsm-auto-deployer-")
            .tempdir()
            .context("无法创建下载临时目录")?;
        let download_path = temp_dir.path().join("asset.bin");
        let artifact_for_download = artifact.clone();

        self.state.update_runtime(&task.id, |runtime| {
            runtime.phase = TaskPhase::Downloading;
            runtime.start_step("download", format!("正在下载 {}", artifact.asset_name));
            runtime.progress = Some(DownloadProgress {
                downloaded: 0,
                total: if artifact.size > 0 {
                    Some(artifact.size)
                } else {
                    None
                },
                percent: 0.0,
            });
        });
        tracing::info!(
            task_id = %task.id,
            provider = task.source_type.label(),
            asset = %artifact.asset_name,
            bytes = artifact.size,
            "开始流式下载资产"
        );

        let state = Arc::clone(&self.state);
        let progress_task_id = task.id.clone();
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let download = self
            .state
            .sources
            .download(
                &artifact_for_download,
                &config,
                &download_path,
                move |downloaded, total| {
                    let finished = total.map(|total| downloaded >= total).unwrap_or(false);
                    if !finished && last_emit.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                    last_emit = Instant::now();
                    let percent = total
                        .filter(|total| *total > 0)
                        .map(|total| (downloaded as f64 / total as f64 * 100.0).min(100.0))
                        .unwrap_or(0.0);
                    state.update_runtime(&progress_task_id, |runtime| {
                        runtime.progress = Some(DownloadProgress {
                            downloaded,
                            total,
                            percent,
                        });
                    });
                },
            )
            .await
            .context("下载资产失败")?;

        self.state.update_runtime(&task.id, |runtime| {
            runtime.phase = TaskPhase::Verifying;
            runtime.finish_step(
                "download",
                format!("下载完成 · {}", format_bytes(download.bytes)),
            );
            runtime.start_step("verify", "正在校验 SHA-256");
            runtime.progress = Some(DownloadProgress {
                downloaded: download.bytes,
                total: Some(download.bytes),
                percent: 100.0,
            });
        });
        self.state.update_runtime(&task.id, |runtime| {
            runtime.finish_step(
                "verify",
                format!("SHA-256 校验通过 · {}", short_hash(&download.sha256)),
            );
        });
        tracing::info!(
            task_id = %task.id,
            bytes = download.bytes,
            sha256 = %download.sha256,
            "SHA-256 校验完成"
        );

        if task.cleanup_old {
            self.state.update_runtime(&task.id, |runtime| {
                runtime.phase = TaskPhase::Cleaning;
                runtime.start_step("cleanup", "列出目标目录并匹配旧文件");
                runtime.progress = None;
            });
            let previous = self.state.runtime_snapshot(&task.id).unwrap_or_default();
            let tracked = previous.deployed_remote_path.clone().or_else(|| {
                previous.deployed_asset.as_ref().map(|asset| {
                    let dir = task.mcsm_target_path.trim_end_matches('/');
                    let dir = if dir.is_empty() { "/" } else { dir };
                    join_posix(dir, asset)
                })
            });
            let deleted = self
                .cleanup_old_remote_files(&mcsm, task, &artifact, tracked.as_deref())
                .await
                .context("清理旧版远程文件失败")?;
            self.state.update_runtime(&task.id, |runtime| {
                if deleted.is_empty() {
                    runtime.finish_step("cleanup", "没有需要清理的旧文件");
                } else {
                    runtime.finish_step("cleanup", format!("已删除 {} 个旧文件", deleted.len()));
                }
            });
            tracing::info!(
                task_id = %task.id,
                deleted = deleted.len(),
                "旧版远程文件清理完成"
            );
        } else {
            self.state.update_runtime(&task.id, |runtime| {
                runtime.skip_step("cleanup", "未启用旧版清理");
            });
        }

        self.state.update_runtime(&task.id, |runtime| {
            runtime.phase = TaskPhase::Uploading;
            runtime.start_step("credential", "准备申请上传凭证");
            runtime.progress = None;
        });
        tracing::info!(
            task_id = %task.id,
            target = %task.mcsm_target_path,
            "开始上传到 MCSM 实例"
        );
        let upload = mcsm
            .upload_and_place(task, &download_path, &artifact.asset_name)
            .await
            .context("MCSM 上传流程失败")?;
        tracing::info!(
            task_id = %task.id,
            bytes = upload.bytes,
            upload_dir = %upload.upload_dir,
            uploaded_path = %upload.uploaded_path,
            final_path = %upload.final_path,
            "文件上传完成"
        );

        self.state.update_runtime(&task.id, |runtime| {
            if upload.uploaded_path == upload.final_path {
                runtime.skip_step("rename", "目标保留上传文件名，无需重命名");
            }
            runtime.current_version = Some(latest_version.clone());
            runtime.deployed_asset = Some(latest_asset.clone());
            runtime.deployed_remote_path = Some(upload.final_path.clone());
            runtime.last_updated_at = Some(now_ms());
            runtime.update_available = false;
            runtime.sha256 = Some(download.sha256.clone());
            runtime.last_error = None;
            runtime.restart_deferred = false;
        });
        if let Err(error) = self.state.persist().await {
            tracing::warn!(task_id = %task.id, error = %error, "部署成功但状态持久化失败");
        }

        if task.restart && !suppress_restart {
            self.state.update_runtime(&task.id, |runtime| {
                runtime.phase = TaskPhase::Restarting;
                runtime.start_step("restart", "准备调用 MCSM 重启实例");
            });
            let mode = task.restart_mode(config.mcsm.restart_mode);
            tracing::info!(
                task_id = %task.id,
                mode = mode.as_str(),
                "触发 MCSM 实例重启"
            );
            match mcsm.restart_instance(task, mode).await {
                Ok(()) => {
                    tracing::info!(task_id = %task.id, "实例重启指令已发送");
                }
                Err(error) => {
                    tracing::error!(
                        task_id = %task.id,
                        error = %format!("{error:#}"),
                        "文件已部署，但实例重启失败"
                    );
                    return Err(anyhow!("文件已部署，但实例重启失败: {error}"));
                }
            }
        } else if suppress_restart {
            self.state.update_runtime(&task.id, |runtime| {
                runtime.restart_deferred = true;
                runtime.skip_step("restart", "已跳过逐任务重启，等待统一重启");
            });
            tracing::info!(
                task_id = %task.id,
                "文件更新完成，已跳过逐任务重启，等待统一重启"
            );
        } else {
            self.state.update_runtime(&task.id, |runtime| {
                runtime.skip_step("restart", "任务未启用自动重启");
            });
            tracing::info!(task_id = %task.id, "任务已关闭自动重启，文件更新完成");
        }

        self.state.update_runtime(&task.id, |runtime| {
            runtime.phase = TaskPhase::Success;
            runtime.progress = None;
            runtime.current_detail = Some("部署完成".to_string());
        });
        tracing::info!(
            task_id = %task.id,
            version = %latest_version,
            "任务完成：版本已部署"
        );
        Ok(())
    }

    /// Build the set of old remote files to delete. Both a tracked path and a
    /// user-supplied remote filename regex are checked; the regex is what makes
    /// cleanup safe when an administrator renamed the deployed file.
    async fn cleanup_old_remote_files(
        &self,
        mcsm: &McsmClient,
        task: &TaskConfig,
        artifact: &ResolvedArtifact,
        tracked_path: Option<&str>,
    ) -> Result<Vec<String>> {
        let target = McsmClient::resolve_target(task, &artifact.asset_name)?;
        if !task.target_is_dir() {
            return Ok(Vec::new());
        }
        let dir = target.upload_dir.clone();
        let new_name = file_name(&target.uploaded_path).to_string();
        let files = mcsm
            .list_directory(&task.mcsm_daemon_id, &task.mcsm_instance_uuid, &dir)
            .await?;
        let existing: HashSet<&str> = files
            .iter()
            .filter(|file| file.kind == 1)
            .map(|file| file.name.as_str())
            .collect();
        let regex = task.compiled_cleanup_regex()?;
        let mut to_delete = Vec::new();
        let mut seen = HashSet::new();

        if let Some(regex) = &regex {
            for file in files.iter().filter(|file| file.kind == 1) {
                if file.name == new_name || !regex.is_match(&file.name) {
                    continue;
                }
                let path = join_posix(&dir, &file.name);
                if seen.insert(path.clone()) {
                    to_delete.push(path);
                }
            }
        }

        if let Some(tracked) = tracked_path {
            if tracked != target.final_path && tracked != target.uploaded_path {
                if let Some(name) = tracked.rsplit('/').next() {
                    if existing.contains(name) && seen.insert(tracked.to_string()) {
                        to_delete.push(tracked.to_string());
                    }
                }
            }
        }

        if to_delete.is_empty() {
            tracing::info!(
                task_id = %task.id,
                dir = %dir,
                "没有发现需要清理的旧版远程文件"
            );
            return Ok(Vec::new());
        }
        tracing::info!(
            task_id = %task.id,
            dir = %dir,
            files = ?to_delete,
            "删除旧版远程文件后再上传新版"
        );
        mcsm.delete_files(&task.mcsm_daemon_id, &task.mcsm_instance_uuid, &to_delete)
            .await?;
        Ok(to_delete)
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn short_hash(hash: &str) -> &str {
    let end = hash.len().min(12);
    &hash[..end]
}

fn join_posix(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}
