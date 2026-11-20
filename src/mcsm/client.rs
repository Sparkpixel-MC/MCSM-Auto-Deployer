//! MCSManager client.
//!
//! The file upload deliberately uses the daemon passport flow. The panel is
//! only used to sign a one-time upload passport; file bytes never pass through
//! the panel API.

use crate::config::{McsmConfig, RestartMode, TaskConfig};
use crate::mcsm::types::{
    ApiEnvelope, DeleteFilesRequest, FileEntry, FileListData, FileStatusData, InstanceDetail,
    InstanceOperationData, MoveFilesRequest, UploadOutcome, UploadPassport, UploadTarget,
    UrlDownloadRequest,
};
use reqwest::{Client, Method, Response, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum McsmError {
    #[error("MCSM 配置错误: {0}")]
    Config(String),
    #[error("MCSM 网络请求失败: {0}")]
    Network(reqwest::Error),
    #[error("MCSM API 返回 status={status}: {message}")]
    Api { status: i64, message: String },
    #[error("MCSM 响应解析失败: {0}")]
    Decode(serde_json::Error),
    #[error("MCSM 文件操作失败: {0}")]
    File(String),
    #[error("MCSM 上传失败 (HTTP {status}): {message}")]
    Upload { status: u16, message: String },
    #[error("实例在超时时间内仍未停止，已取消启动操作")]
    StopTimeout,
    #[error("上传凭证响应缺少 addr / uploadAddress 字段")]
    MissingUploadAddress,
}

/// Internal progress events emitted by the MCSM client. The engine uses these
/// to render the per-task deployment timeline without coupling the client to
/// the web layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McsmEventKind {
    Started,
    Finished,
    Failed,
    Retry,
    Info,
}

#[derive(Debug, Clone)]
pub struct McsmEvent {
    pub step: &'static str,
    pub kind: McsmEventKind,
    pub message: String,
    pub attempt: u32,
}

pub type McsmEventSink = Arc<dyn Fn(McsmEvent) + Send + Sync>;

#[derive(Clone)]
pub struct McsmClient {
    http: Client,
    base: Url,
    api_key: String,
    daemon_scheme: String,
    timeout: Duration,
    api_min_interval: Duration,
    api_retry_attempts: u32,
    events: Option<McsmEventSink>,
}

impl McsmClient {
    pub fn new(config: &McsmConfig) -> Result<Self, McsmError> {
        let key = config.api_key.trim();
        if key.is_empty() || key.starts_with("YOUR_") {
            return Err(McsmError::Config(
                "尚未配置有效的 MCSM API Key，请先在设置中填写".to_string(),
            ));
        }
        let base = Url::parse(&config.base_url)
            .map_err(|error| McsmError::Config(format!("base_url 无效: {error}")))?;
        if base.scheme() != "http" && base.scheme() != "https" {
            return Err(McsmError::Config("base_url 只支持 http/https".to_string()));
        }
        let daemon_scheme = config
            .daemon_scheme
            .clone()
            .unwrap_or_else(|| base.scheme().to_string());
        let timeout = Duration::from_secs(config.timeout_secs.clamp(10, 1_800));
        let api_min_interval = Duration::from_millis(config.api_min_interval_ms.min(5_000));
        let api_retry_attempts = config.api_retry_attempts.clamp(1, 10);
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(timeout)
            .danger_accept_invalid_certs(config.accept_invalid_certs)
            .build()
            .map_err(sanitize_network_error)?;
        Ok(Self {
            http,
            base,
            api_key: key.to_string(),
            daemon_scheme,
            timeout,
            api_min_interval,
            api_retry_attempts,
            events: None,
        })
    }

    pub fn with_events(mut self, sink: McsmEventSink) -> Self {
        self.events = Some(sink);
        self
    }

    fn emit(&self, step: &'static str, kind: McsmEventKind, message: impl Into<String>) {
        let message = message.into();
        tracing::debug!(step, ?kind, %message, "MCSM 操作事件");
        if let Some(sink) = &self.events {
            sink(McsmEvent {
                step,
                kind,
                message,
                attempt: 0,
            });
        }
    }

    fn emit_retry(&self, step: &'static str, attempt: u32, wait: Duration, message: String) {
        tracing::debug!(step, attempt, wait_ms = wait.as_millis(), %message, "MCSM 重试事件");
        if let Some(sink) = &self.events {
            sink(McsmEvent {
                step,
                kind: McsmEventKind::Retry,
                message,
                attempt,
            });
        }
    }

    fn panel_url(&self, path: &str, query: &[(&str, &str)]) -> Result<Url, McsmError> {
        let raw = format!(
            "{}/{}",
            self.base.as_str().trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut url = Url::parse(&raw)
            .map_err(|error| McsmError::Config(format!("无法构造 MCSM 请求地址: {error}")))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("apikey", &self.api_key);
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    fn panel_request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Content-Type", "application/json; charset=utf-8")
            .header("Accept", "application/json")
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<T, McsmError> {
        let url = self.panel_url(path, query)?;
        let attempts = self.api_retry_attempts.max(1);
        for attempt in 1..=attempts {
            wait_for_panel_slot(self.api_min_interval).await;
            let mut request = self.panel_request(method.clone(), url.clone());
            if let Some(body) = body.as_ref() {
                request = request.json(body);
            }
            tracing::debug!(operation = path, method = %method, attempt, "调用 MCSM 面板 API");
            let response = request.send().await.map_err(sanitize_network_error)?;
            match self.parse_envelope(response, path).await {
                Ok(value) => {
                    if attempt > 1 {
                        tracing::info!(operation = path, attempt, "MCSM 面板 API 重试成功");
                    }
                    return Ok(value);
                }
                Err(error) => {
                    if let Some(wait) = cooldown_delay(&error) {
                        if attempt < attempts {
                            let step = step_for_operation(path);
                            let message = format!(
                                "MCSM 冷却中，等待 {} ms 后重试（{attempt}/{attempts}）",
                                wait.as_millis()
                            );
                            self.emit_retry(step, attempt, wait, message);
                            tracing::warn!(
                                operation = path,
                                attempt,
                                max_attempts = attempts,
                                wait_ms = wait.as_millis(),
                                "MCSM API 冷却，等待后重试"
                            );
                            tokio::time::sleep(wait).await;
                            continue;
                        }
                    }
                    return Err(error);
                }
            }
        }
        Err(McsmError::Api {
            status: 0,
            message: format!("{path}: 重试次数已用尽"),
        })
    }

    async fn parse_envelope<T: DeserializeOwned>(
        &self,
        response: Response,
        action: &str,
    ) -> Result<T, McsmError> {
        let http_status = response.status();
        let text = response.text().await.map_err(sanitize_network_error)?;
        let envelope: ApiEnvelope<Value> =
            serde_json::from_str(&text).map_err(|error| McsmError::Api {
                status: http_status.as_u16() as i64,
                message: format!(
                    "{action}: 面板返回了非 JSON 响应 ({error}): {}",
                    truncate(&text, 240)
                ),
            })?;
        let envelope_status = envelope.status;
        if let Some(server_time) = envelope.time {
            tracing::trace!(server_time, action, "MCSM 响应时间戳");
        }
        if !http_status.is_success() || envelope_status != 200 {
            let data = envelope.data.unwrap_or(Value::Null);
            return Err(McsmError::Api {
                status: envelope_status,
                message: format!("{action}: {}", describe_payload(&data)),
            });
        }
        let data = envelope.data.ok_or_else(|| McsmError::Api {
            status: envelope_status,
            message: format!("{action}: 面板响应缺少 data 字段"),
        })?;
        serde_json::from_value(data).map_err(McsmError::Decode)
    }

    /// Compute the destination layout without touching the network. This is
    /// shared by the uploader and unit tests.
    pub fn resolve_target(task: &TaskConfig, asset_name: &str) -> Result<UploadTarget, McsmError> {
        let safe_name = safe_file_name(asset_name)?;
        let target = task.mcsm_target_path.trim();
        if target.is_empty() {
            return Err(McsmError::Config("mcsm_target_path 不能为空".to_string()));
        }
        if task.target_is_dir() {
            let dir = target.trim_end_matches('/');
            let dir = if dir.is_empty() { "/" } else { dir };
            let uploaded_path = join_posix(dir, &safe_name);
            return Ok(UploadTarget {
                upload_dir: dir.to_string(),
                final_path: uploaded_path.clone(),
                uploaded_path,
                needs_move: false,
            });
        }
        if !target.starts_with('/') {
            return Err(McsmError::Config(
                "文件目标的 mcsm_target_path 必须是绝对路径".to_string(),
            ));
        }
        let parent = match target.rfind('/') {
            Some(0) => "/",
            Some(index) => &target[..index],
            None => "/",
        };
        let uploaded_path = join_posix(parent, &safe_name);
        Ok(UploadTarget {
            upload_dir: parent.to_string(),
            needs_move: uploaded_path != target,
            final_path: target.to_string(),
            uploaded_path,
        })
    }

    async fn fetch_upload_passport(
        &self,
        target: &UploadTarget,
        daemon_id: &str,
        instance_uuid: &str,
    ) -> Result<UploadPassport, McsmError> {
        // ────────────────────────────────────────────────────────────────────
        // MCSM 两步上传 · 边界 1/2
        // 面板只负责签发一次性传输凭证，不接收文件字节。查询参数和
        // X-Requested-With 头都是 MCSM 文档要求的认证组成部分。
        // ────────────────────────────────────────────────────────────────────
        self.emit(
            "credential",
            McsmEventKind::Started,
            format!("向面板申请上传凭证 → {}", target.upload_dir),
        );
        let result = self
            .request_json::<UploadPassport>(
                Method::POST,
                "api/files/upload",
                &[
                    ("upload_dir", &target.upload_dir),
                    ("daemonId", daemon_id),
                    ("uuid", instance_uuid),
                ],
                None,
            )
            .await;
        match &result {
            Ok(_) => self.emit(
                "credential",
                McsmEventKind::Finished,
                "上传凭证已获取，准备直传 Daemon",
            ),
            Err(error) => self.emit(
                "credential",
                McsmEventKind::Failed,
                format!("申请上传凭证失败: {error}"),
            ),
        }
        result
    }

    /// Build the direct daemon endpoint from either modern (addr/password) or
    /// legacy (uploadAddress/password) passports.
    fn daemon_upload_url(&self, passport: &UploadPassport) -> Result<Url, McsmError> {
        let raw = if let Some(address) = passport
            .upload_address
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            address.to_string()
        } else if let Some(addr) = passport
            .addr
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            addr.to_string()
        } else {
            return Err(McsmError::MissingUploadAddress);
        };

        // MCSM stores the daemon address as a WebSocket origin (ws:// or
        // wss://). File transfer itself is still an HTTP(S) endpoint, and the
        // official panel frontend converts wss -> https and ws -> http before
        // POSTing to /upload/{password}. Mirror that conversion here, keeping
        // any daemon URL prefix intact.
        let absolute = normalize_daemon_http_origin(&raw, &self.daemon_scheme)?;
        let mut url = Url::parse(&absolute)
            .map_err(|error| McsmError::Config(format!("Daemon 上传地址无效: {error}")))?;
        let password = passport.password.as_str();
        let mut path = url.path().trim_end_matches('/').to_string();
        if path.contains("{password}") {
            path = path.replace("{password}", password);
        } else if !path.contains(password) {
            if path.is_empty() || path == "/" {
                path = format!("/upload/{password}");
            } else if path.ends_with("/upload") {
                path = format!("{path}/{password}");
            } else {
                path = format!("{path}/upload/{password}");
            }
        }
        url.set_path(&path);
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("overwrite", "true");
            // The strict requirement is that every request carries both the
            // XMLHttpRequest marker and an apikey parameter. The daemon ignores
            // the panel key, but it is harmless and keeps the audit uniform.
            pairs.append_pair("apikey", &self.api_key);
        }
        Ok(url)
    }

    // ────────────────────────────────────────────────────────────────────────
    // MCSM 两步上传 · 边界 2/2
    // 这一部分必须直接 POST 到 Daemon 的 /upload/{password}，绝不能改成
    // 面板的 /api/files/upload。multipart 中只有文件字段；目标目录已在第
    // 一步通过 upload_dir 绑定到一次性凭证。
    // ────────────────────────────────────────────────────────────────────────
    async fn upload_bytes_to_daemon(
        &self,
        url: &Url,
        local_path: &Path,
        file_name: &str,
    ) -> Result<u64, McsmError> {
        let file = tokio::fs::File::open(local_path).await.map_err(|error| {
            McsmError::File(format!(
                "无法打开待上传文件 {}: {error}",
                local_path.display()
            ))
        })?;
        let size = file
            .metadata()
            .await
            .map_err(|error| McsmError::File(format!("无法读取待上传文件大小: {error}")))?
            .len();
        let part = reqwest::multipart::Part::stream_with_length(file, size)
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")
            .map_err(|error| McsmError::File(format!("无法构造 multipart 文件字段: {error}")))?;
        let form = reqwest::multipart::Form::new().part("file", part);

        self.emit(
            "daemon_upload",
            McsmEventKind::Started,
            format!("开始直传 Daemon（{}，{}）", file_name, format_bytes(size)),
        );
        tracing::info!(
            file_name = %file_name,
            bytes = size,
            "文件直传 MCSM Daemon（不经过面板 API）"
        );
        let response = match self
            .http
            .post(url.clone())
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "*/*")
            .multipart(form)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let error = sanitize_network_error(error);
                self.emit(
                    "daemon_upload",
                    McsmEventKind::Failed,
                    format!("Daemon 直传失败: {error}"),
                );
                return Err(error);
            }
        };
        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                let error = sanitize_network_error(error);
                self.emit(
                    "daemon_upload",
                    McsmEventKind::Failed,
                    format!("读取 Daemon 响应失败: {error}"),
                );
                return Err(error);
            }
        };
        if !status.is_success() {
            let error = McsmError::Upload {
                status: status.as_u16(),
                message: truncate(&body, 300),
            };
            self.emit(
                "daemon_upload",
                McsmEventKind::Failed,
                format!("Daemon 直传失败: {error}"),
            );
            return Err(error);
        }
        // Some daemon versions answer with a JSON envelope even on 200.
        if body.trim_start().starts_with('{') {
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                if let Some(inner_status) = value.get("status").and_then(Value::as_i64) {
                    if inner_status != 200 {
                        let error = McsmError::Upload {
                            status: status.as_u16(),
                            message: truncate(&body, 300),
                        };
                        self.emit(
                            "daemon_upload",
                            McsmEventKind::Failed,
                            format!("Daemon 直传失败: {error}"),
                        );
                        return Err(error);
                    }
                }
            }
        }
        self.emit(
            "daemon_upload",
            McsmEventKind::Finished,
            format!("Daemon 直传完成（{}）", format_bytes(size)),
        );
        Ok(size)
    }

    /// Ask the daemon to download a public URL directly into the instance.
    /// This bypasses the panel/direct upload data plane and therefore the
    /// 8 MiB piece forwarding limit that can produce HTTP 502 responses.
    pub async fn download_from_url_and_wait<F>(
        &self,
        task: &TaskConfig,
        url: &str,
        file_name: &str,
        expected_size: u64,
        mut progress: F,
    ) -> Result<u64, McsmError>
    where
        F: FnMut(u64, Option<u64>),
    {
        self.emit(
            "url_download",
            McsmEventKind::Started,
            format!("请求 Daemon 从外部 URL 下载到 {file_name}"),
        );
        let body = serde_json::to_value(UrlDownloadRequest { url, file_name })
            .map_err(McsmError::Decode)?;
        let result = self
            .request_json::<Value>(
                Method::POST,
                "api/files/download_from_url",
                &[
                    ("daemonId", &task.mcsm_daemon_id),
                    ("uuid", &task.mcsm_instance_uuid),
                ],
                Some(body),
            )
            .await;
        if let Err(error) = result {
            self.emit(
                "url_download",
                McsmEventKind::Failed,
                format!("启动 URL 下载失败: {error}"),
            );
            return Err(error);
        }

        let deadline = tokio::time::Instant::now() + self.timeout.max(Duration::from_secs(1_800));
        let target = file_name.trim_start_matches('/');
        let mut seen = false;
        let mut last_progress = 0_u64;
        loop {
            let status: FileStatusData = self
                .request_json(
                    Method::GET,
                    "api/files/status",
                    &[
                        ("daemonId", &task.mcsm_daemon_id),
                        ("uuid", &task.mcsm_instance_uuid),
                    ],
                    None,
                )
                .await?;
            if let Some(download) = status
                .download_tasks
                .iter()
                .find(|download| download.path.trim_start_matches('/') == target)
            {
                if !seen {
                    tracing::info!(
                        task_id = %download.task_id,
                        path = %download.path,
                        "已找到 MCSM URL 下载任务"
                    );
                }
                seen = true;
                let total = if download.total > 0 {
                    download.total
                } else {
                    expected_size
                };
                match download.status {
                    0 => {
                        if download.current != last_progress {
                            last_progress = download.current;
                            progress(download.current, Some(total));
                            tracing::debug!(
                                path = %file_name,
                                current = download.current,
                                total,
                                "MCSM URL 下载进度"
                            );
                        }
                    }
                    1 => {
                        progress(total, Some(total));
                        self.emit(
                            "url_download",
                            McsmEventKind::Finished,
                            format!("URL 下载完成（{total} bytes）"),
                        );
                        return Ok(total);
                    }
                    2 => {
                        let detail = download
                            .error
                            .clone()
                            .unwrap_or_else(|| "MCSM 未返回错误详情".to_string());
                        self.emit(
                            "url_download",
                            McsmEventKind::Failed,
                            format!("URL 下载失败: {detail}"),
                        );
                        return Err(McsmError::File(format!("MCSM URL 下载失败: {detail}")));
                    }
                    other => {
                        tracing::debug!(status = other, "MCSM URL 下载状态未知");
                    }
                }
            } else if seen {
                // The daemon keeps completed/error tasks for about one second,
                // so an immediately-disappearing task is ambiguous. Keep
                // polling until the deadline instead of assuming success.
                tracing::debug!(path = %file_name, "MCSM URL 下载任务暂时不在列表中");
            }
            if tokio::time::Instant::now() >= deadline {
                self.emit(
                    "url_download",
                    McsmEventKind::Failed,
                    "URL 下载等待超时".to_string(),
                );
                return Err(McsmError::File(
                    "MCSM URL 下载等待超时，请检查 Daemon 到来源的连通性".to_string(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn move_file(
        &self,
        daemon_id: &str,
        instance_uuid: &str,
        source: &str,
        target: &str,
    ) -> Result<(), McsmError> {
        let request = MoveFilesRequest {
            targets: vec![(source, target)],
        };
        self.emit(
            "rename",
            McsmEventKind::Started,
            format!("移动上传文件 → {target}"),
        );
        let result = self
            .request_json::<Value>(
                Method::PUT,
                "api/files/move",
                &[("daemonId", daemon_id), ("uuid", instance_uuid)],
                Some(serde_json::to_value(request).map_err(McsmError::Decode)?),
            )
            .await;
        match &result {
            Ok(_) => self.emit(
                "rename",
                McsmEventKind::Finished,
                format!("文件已移动到 {target}"),
            ),
            Err(error) => self.emit(
                "rename",
                McsmEventKind::Failed,
                format!("移动文件失败: {error}"),
            ),
        }
        result.map(|_| ())
    }

    /// Full upload flow: passport → direct daemon upload → optional atomic
    /// rename so a core target such as /server.jar receives the right name.
    ///
    /// A fresh passport is requested for every attempt because the daemon may
    /// consume a password even when an upstream proxy returns 502 after
    /// receiving a partial body.
    pub async fn upload_and_place(
        &self,
        task: &TaskConfig,
        local_path: &Path,
        asset_name: &str,
    ) -> Result<UploadOutcome, McsmError> {
        let target = Self::resolve_target(task, asset_name)?;
        let safe_name = safe_file_name(asset_name)?;
        let attempts = self.api_retry_attempts.max(1);
        let mut attempt = 1_u32;
        loop {
            let passport = match self
                .fetch_upload_passport(&target, &task.mcsm_daemon_id, &task.mcsm_instance_uuid)
                .await
            {
                Ok(passport) => passport,
                Err(error) if attempt < attempts && is_retryable_upload_error(&error) => {
                    let wait = upload_retry_delay(attempt);
                    self.emit_retry(
                        "credential",
                        attempt,
                        wait,
                        format!(
                            "申请上传凭证失败，{} ms 后重新申请（{attempt}/{attempts}）: {error}",
                            wait.as_millis()
                        ),
                    );
                    tracing::warn!(
                        attempt,
                        max_attempts = attempts,
                        wait_ms = wait.as_millis(),
                        error = %error,
                        "申请上传凭证失败，稍后重试"
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let upload_url = self.daemon_upload_url(&passport)?;
            match self
                .upload_bytes_to_daemon(&upload_url, local_path, &safe_name)
                .await
            {
                Ok(bytes) => {
                    if target.needs_move {
                        tracing::info!(
                            from = %target.uploaded_path,
                            to = %target.final_path,
                            "重命名上传文件到任务目标路径"
                        );
                        self.move_file(
                            &task.mcsm_daemon_id,
                            &task.mcsm_instance_uuid,
                            &target.uploaded_path,
                            &target.final_path,
                        )
                        .await?;
                    }
                    return Ok(UploadOutcome {
                        upload_dir: target.upload_dir,
                        uploaded_path: target.uploaded_path,
                        final_path: target.final_path,
                        bytes,
                    });
                }
                Err(error) if attempt < attempts && is_retryable_upload_error(&error) => {
                    let wait = upload_retry_delay(attempt);
                    self.emit_retry(
                        "daemon_upload",
                        attempt,
                        wait,
                        format!(
                            "Daemon 直传失败，{} ms 后重新申请凭证并重试（{attempt}/{attempts}）: {error}",
                            wait.as_millis()
                        ),
                    );
                    tracing::warn!(
                        attempt,
                        max_attempts = attempts,
                        wait_ms = wait.as_millis(),
                        error = %error,
                        "Daemon 直传失败，将重新申请上传凭证并重试"
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn instance_status(
        &self,
        daemon_id: &str,
        instance_uuid: &str,
    ) -> Result<Option<i64>, McsmError> {
        let detail: InstanceDetail = self
            .request_json(
                Method::GET,
                "api/instance",
                &[("daemonId", daemon_id), ("uuid", instance_uuid)],
                None,
            )
            .await?;
        Ok(detail.status)
    }

    async fn instance_action(
        &self,
        action: &str,
        daemon_id: &str,
        instance_uuid: &str,
    ) -> Result<(), McsmError> {
        let path = format!("api/protected_instance/{action}");
        let data: InstanceOperationData = self
            .request_json(
                Method::GET,
                &path,
                &[("daemonId", daemon_id), ("uuid", instance_uuid)],
                None,
            )
            .await?;
        tracing::debug!(
            action = %action,
            instance = %data.instance_uuid.as_deref().unwrap_or(instance_uuid),
            "MCSM 实例操作已受理"
        );
        Ok(())
    }

    pub async fn restart_instance(
        &self,
        task: &TaskConfig,
        mode: RestartMode,
    ) -> Result<(), McsmError> {
        self.restart_instance_by(&task.mcsm_daemon_id, &task.mcsm_instance_uuid, mode)
            .await
    }

    /// Restart an instance by identifiers so scheduled restarts do not need a
    /// synthetic TaskConfig.
    pub async fn restart_instance_by(
        &self,
        daemon_id: &str,
        instance_uuid: &str,
        mode: RestartMode,
    ) -> Result<(), McsmError> {
        match mode {
            RestartMode::Restart => {
                self.emit("restart", McsmEventKind::Started, "调用 MCSM 重启实例");
                tracing::info!("调用 MCSM 重启实例");
                let result = self
                    .instance_action("restart", daemon_id, instance_uuid)
                    .await;
                match &result {
                    Ok(()) => self.emit("restart", McsmEventKind::Finished, "MCSM 已受理重启指令"),
                    Err(error) => self.emit(
                        "restart",
                        McsmEventKind::Failed,
                        format!("重启实例失败: {error}"),
                    ),
                }
                result
            }
            RestartMode::StopThenOpen => {
                self.emit(
                    "restart",
                    McsmEventKind::Started,
                    "调用 MCSM 停止实例，等待停止后重新启动",
                );
                tracing::info!("调用 MCSM 停止实例，等待停止后重新启动");
                if let Err(error) = self.instance_action("stop", daemon_id, instance_uuid).await {
                    self.emit(
                        "restart",
                        McsmEventKind::Failed,
                        format!("停止实例失败: {error}"),
                    );
                    return Err(error);
                }
                let deadline =
                    tokio::time::Instant::now() + self.timeout.min(Duration::from_secs(90));
                loop {
                    match self.instance_status(daemon_id, instance_uuid).await {
                        Ok(Some(0)) => break,
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(error = %error, "查询实例状态失败，将继续等待");
                        }
                    }
                    if tokio::time::Instant::now() >= deadline {
                        self.emit("restart", McsmEventKind::Failed, "实例在超时时间内仍未停止");
                        return Err(McsmError::StopTimeout);
                    }
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                }
                tracing::info!("实例已停止，调用 MCSM 启动实例");
                self.emit(
                    "restart",
                    McsmEventKind::Info,
                    "实例已停止，调用 MCSM 启动实例",
                );
                let result = self.instance_action("open", daemon_id, instance_uuid).await;
                match &result {
                    Ok(()) => self.emit("restart", McsmEventKind::Finished, "实例已重新启动"),
                    Err(error) => self.emit(
                        "restart",
                        McsmEventKind::Failed,
                        format!("启动实例失败: {error}"),
                    ),
                }
                result
            }
        }
    }

    /// List all entries of one remote directory (paginated, max 100 per page).
    pub async fn list_directory(
        &self,
        daemon_id: &str,
        instance_uuid: &str,
        dir: &str,
    ) -> Result<Vec<FileEntry>, McsmError> {
        let mut entries = Vec::new();
        let mut page = 0_u64;
        loop {
            let page_string = page.to_string();
            let data: FileListData = self
                .request_json(
                    Method::GET,
                    "api/files/list",
                    &[
                        ("daemonId", daemon_id),
                        ("uuid", instance_uuid),
                        ("target", dir),
                        ("page", &page_string),
                        ("page_size", "100"),
                    ],
                    None,
                )
                .await?;
            let total = data.total;
            let count = data.items.len();
            entries.extend(data.items);
            if count == 0 || entries.len() as u64 >= total || page >= 10_000 {
                break;
            }
            page = page.saturating_add(1);
        }
        Ok(entries)
    }

    pub async fn delete_files(
        &self,
        daemon_id: &str,
        instance_uuid: &str,
        targets: &[String],
    ) -> Result<(), McsmError> {
        if targets.is_empty() {
            return Ok(());
        }
        let request = DeleteFilesRequest {
            targets: targets.to_vec(),
        };
        let _: Value = self
            .request_json(
                Method::DELETE,
                "api/files",
                &[("daemonId", daemon_id), ("uuid", instance_uuid)],
                Some(serde_json::to_value(request).map_err(McsmError::Decode)?),
            )
            .await?;
        Ok(())
    }
}

/// MCSM's file routes use per-operation cooldowns. All panel calls share a
/// single pacing gate inside this process so concurrent tasks cannot burst the
/// same endpoint at once.
static PANEL_GATE: LazyLock<tokio::sync::Mutex<Option<Instant>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

async fn wait_for_panel_slot(interval: Duration) {
    let mut last = PANEL_GATE.lock().await;
    if let Some(previous) = *last {
        let elapsed = previous.elapsed();
        if elapsed < interval {
            let wait = interval - elapsed;
            tracing::debug!(
                wait_ms = wait.as_millis(),
                "MCSM 面板 API 节流等待，避免触发操作冷却"
            );
            tokio::time::sleep(wait).await;
        }
    }
    *last = Some(Instant::now());
}

fn is_retryable_upload_error(error: &McsmError) -> bool {
    match error {
        McsmError::Upload { status, .. } => {
            matches!(*status, 408 | 429 | 500 | 502 | 503 | 504)
        }
        McsmError::Network(error) => error.is_timeout() || error.is_connect() || error.is_request(),
        _ => false,
    }
}

fn upload_retry_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(3);
    Duration::from_millis(1_000_u64 << exponent)
}

fn cooldown_delay(error: &McsmError) -> Option<Duration> {
    let McsmError::Api { status, message } = error else {
        return None;
    };
    if *status != 500 && *status != 429 {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    if !message.contains("冷却") && !lower.contains("cooldown") && !message.contains("请稍后")
    {
        return None;
    }
    let seconds = cooldown_seconds(message).unwrap_or(1);
    Some(Duration::from_millis((seconds.saturating_add(1)) * 1_000))
}

fn cooldown_seconds(message: &str) -> Option<u64> {
    let after = message.split('约').nth(1)?;
    let mut digits = String::new();
    let mut started = false;
    for character in after.chars() {
        if character.is_ascii_digit() {
            started = true;
            digits.push(character);
        } else if started {
            if character == '秒' {
                return digits.parse().ok();
            }
            if !character.is_whitespace() {
                return None;
            }
        }
    }
    None
}

fn step_for_operation(path: &str) -> &'static str {
    if path.starts_with("api/files/upload") {
        "credential"
    } else if path.starts_with("api/files/move") {
        "rename"
    } else if path.starts_with("api/files") {
        "cleanup"
    } else if path.contains("protected_instance") || path.starts_with("api/instance") {
        "restart"
    } else {
        "api"
    }
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

fn normalize_daemon_http_origin(raw: &str, fallback_scheme: &str) -> Result<String, McsmError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(McsmError::MissingUploadAddress);
    }
    let lower = value.to_ascii_lowercase();
    let normalized = if lower.starts_with("wss://") {
        format!("https://{}", &value[6..])
    } else if lower.starts_with("ws://") {
        format!("http://{}", &value[5..])
    } else if lower.starts_with("https://") || lower.starts_with("http://") {
        value.to_string()
    } else {
        format!("{}://{}", fallback_scheme, value)
    };
    Ok(normalized)
}

fn sanitize_network_error(error: reqwest::Error) -> McsmError {
    // Strip the URL so a query-string API key can never leak into logs.
    McsmError::Network(error.without_url())
}

fn describe_payload(value: &Value) -> String {
    match value {
        Value::String(text) => truncate(text, 300),
        Value::Null => "无错误详情".to_string(),
        other => truncate(&other.to_string(), 300),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let shortened: String = text.chars().take(max).collect();
    format!("{shortened}…")
}

fn safe_file_name(name: &str) -> Result<String, McsmError> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    if last.is_empty() || last == "." || last == ".." {
        return Err(McsmError::Config(format!("无效的资产文件名: {name}")));
    }
    let sanitized: String = last
        .chars()
        .map(|character| match character {
            '\0' | '/' | '\\' => '_',
            other => other,
        })
        .collect();
    Ok(sanitized)
}

pub(crate) fn join_posix(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> McsmConfig {
        McsmConfig {
            base_url: "https://panel.example.com".to_string(),
            api_key: "test-key".to_string(),
            timeout_secs: 60,
            restart_mode: RestartMode::Restart,
            daemon_scheme: Some("http".to_string()),
            accept_invalid_certs: false,
            api_min_interval_ms: 0,
            api_retry_attempts: 4,
            url_download_threshold_mb: 8,
        }
    }

    #[test]
    fn directory_target_keeps_asset_name() {
        let task = TaskConfig {
            mcsm_target_path: "/plugins/".to_string(),
            ..TaskConfig::default()
        };
        match McsmClient::resolve_target(&task, "Example-1.2.jar") {
            Ok(target) => {
                assert_eq!(target.upload_dir, "/plugins");
                assert_eq!(target.uploaded_path, "/plugins/Example-1.2.jar");
                assert!(!target.needs_move);
            }
            Err(error) => assert!(false, "resolve failed: {error}"),
        }
    }

    #[test]
    fn file_target_is_renamed_after_upload() {
        let task = TaskConfig {
            mcsm_target_path: "/server.jar".to_string(),
            ..TaskConfig::default()
        };
        match McsmClient::resolve_target(&task, "lophine-42.jar") {
            Ok(target) => {
                assert_eq!(target.upload_dir, "/");
                assert_eq!(target.uploaded_path, "/lophine-42.jar");
                assert_eq!(target.final_path, "/server.jar");
                assert!(target.needs_move);
            }
            Err(error) => assert!(false, "resolve failed: {error}"),
        }
    }

    #[test]
    fn legacy_and_modern_passports_build_daemon_urls() {
        let Ok(client) = McsmClient::new(&test_config()) else {
            assert!(false, "test client construction failed");
            return;
        };
        let modern = UploadPassport {
            password: "abc123".to_string(),
            addr: Some("10.0.0.5:24444".to_string()),
            upload_address: None,
            remote_mappings: None,
        };
        match client.daemon_upload_url(&modern) {
            Ok(url) => {
                assert_eq!(url.path(), "/upload/abc123");
                assert_eq!(url.host_str(), Some("10.0.0.5"));
            }
            Err(error) => assert!(false, "modern URL failed: {error}"),
        }

        let legacy = UploadPassport {
            password: "secret".to_string(),
            addr: None,
            upload_address: Some("https://daemon.example.com/upload".to_string()),
            remote_mappings: None,
        };
        match client.daemon_upload_url(&legacy) {
            Ok(url) => assert_eq!(url.path(), "/upload/secret"),
            Err(error) => assert!(false, "legacy URL failed: {error}"),
        }

        let websocket_tls = UploadPassport {
            password: "wsspass".to_string(),
            addr: Some("wss://node.example.com:443".to_string()),
            upload_address: None,
            remote_mappings: None,
        };
        match client.daemon_upload_url(&websocket_tls) {
            Ok(url) => {
                assert_eq!(url.scheme(), "https");
                assert_eq!(url.host_str(), Some("node.example.com"));
                assert_eq!(url.port_or_known_default(), Some(443));
                assert_eq!(url.path(), "/upload/wsspass");
            }
            Err(error) => assert!(false, "wss URL failed: {error}"),
        }
    }

    #[tokio::test]
    async fn url_download_waits_for_daemon_task_completion() {
        use axum::{
            extract::State,
            routing::{get, post},
            Json, Router,
        };
        use serde_json::json;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct MockState {
            posts: Arc<AtomicUsize>,
            statuses: Arc<AtomicUsize>,
        }

        async fn start(State(state): State<MockState>, _body: axum::body::Bytes) -> Json<Value> {
            state.posts.fetch_add(1, Ordering::SeqCst);
            Json(json!({ "status": 200, "data": "task-1" }))
        }

        async fn status(State(state): State<MockState>) -> Json<Value> {
            let number = state.statuses.fetch_add(1, Ordering::SeqCst);
            let (current, task_status) = if number == 0 { (25, 0) } else { (100, 1) };
            Json(json!({
                "status": 200,
                "data": {
                    "downloadTasks": [{
                        "taskId": "task-1",
                        "path": "/plugins/url.jar",
                        "total": 100,
                        "current": current,
                        "status": task_status
                    }]
                }
            }))
        }

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => {
                assert!(false, "bind URL mock failed: {error}");
                return;
            }
        };
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => {
                assert!(false, "mock address failed: {error}");
                return;
            }
        };
        let posts = Arc::new(AtomicUsize::new(0));
        let statuses = Arc::new(AtomicUsize::new(0));
        let state = MockState {
            posts: Arc::clone(&posts),
            statuses: Arc::clone(&statuses),
        };
        let app = Router::new()
            .route("/api/files/download_from_url", post(start))
            .route("/api/files/status", get(status))
            .with_state(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut config = test_config();
        config.base_url = format!("http://{address}");
        config.api_min_interval_ms = 0;
        let client = match McsmClient::new(&config) {
            Ok(client) => client,
            Err(error) => {
                assert!(false, "test client construction failed: {error}");
                return;
            }
        };
        let task = TaskConfig {
            mcsm_daemon_id: "daemon".to_string(),
            mcsm_instance_uuid: "instance".to_string(),
            ..TaskConfig::default()
        };
        let mut last = (0_u64, None);
        let result = client
            .download_from_url_and_wait(
                &task,
                "https://example.com/url.jar",
                "/plugins/url.jar",
                100,
                |current, total| last = (current, total),
            )
            .await;
        match result {
            Ok(bytes) => assert_eq!(bytes, 100),
            Err(error) => {
                assert!(false, "URL download should complete: {error}");
                return;
            }
        }
        assert_eq!(last, (100, Some(100)));
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        assert_eq!(statuses.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn classifies_retryable_upload_failures() {
        assert!(is_retryable_upload_error(&McsmError::Upload {
            status: 502,
            message: "piece at 8388608 failed on all targets".to_string(),
        }));
        assert!(is_retryable_upload_error(&McsmError::Upload {
            status: 504,
            message: "gateway timeout".to_string(),
        }));
        assert!(!is_retryable_upload_error(&McsmError::Upload {
            status: 403,
            message: "forbidden".to_string(),
        }));
        assert_eq!(upload_retry_delay(1), Duration::from_secs(1));
        assert_eq!(upload_retry_delay(3), Duration::from_secs(4));
    }

    #[tokio::test]
    async fn daemon_upload_retries_with_a_fresh_passport() {
        use axum::{
            extract::State,
            response::{IntoResponse, Response},
            routing::post,
            Json, Router,
        };
        use serde_json::json;
        use std::io::Write as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct MockState {
            addr: String,
            passports: Arc<AtomicUsize>,
            uploads: Arc<AtomicUsize>,
        }

        async fn passport(State(state): State<MockState>) -> Json<Value> {
            let number = state.passports.fetch_add(1, Ordering::SeqCst) + 1;
            Json(json!({
                "status": 200,
                "data": {
                    "password": format!("p{number}"),
                    "addr": format!("http://{}", state.addr)
                }
            }))
        }

        async fn upload(State(state): State<MockState>, _body: axum::body::Bytes) -> Response {
            let number = state.uploads.fetch_add(1, Ordering::SeqCst) + 1;
            if number == 1 {
                (
                    axum::http::StatusCode::BAD_GATEWAY,
                    "piece at 8388608 failed on all targets",
                )
                    .into_response()
            } else {
                Json(json!({ "status": 200 })).into_response()
            }
        }

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => {
                assert!(false, "bind upload mock failed: {error}");
                return;
            }
        };
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => {
                assert!(false, "mock address failed: {error}");
                return;
            }
        };
        let state = MockState {
            addr: address.to_string(),
            passports: Arc::new(AtomicUsize::new(0)),
            uploads: Arc::new(AtomicUsize::new(0)),
        };
        let passport_count = Arc::clone(&state.passports);
        let upload_count = Arc::clone(&state.uploads);
        let app = Router::new()
            .route("/api/files/upload", post(passport))
            .route("/upload/{password}", post(upload))
            .with_state(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut config = test_config();
        config.base_url = format!("http://{address}");
        config.api_min_interval_ms = 0;
        config.api_retry_attempts = 3;
        let client = match McsmClient::new(&config) {
            Ok(client) => client,
            Err(error) => {
                assert!(false, "test client construction failed: {error}");
                return;
            }
        };
        let mut temp = match tempfile::NamedTempFile::new() {
            Ok(file) => file,
            Err(error) => {
                assert!(false, "temp file failed: {error}");
                return;
            }
        };
        if let Err(error) = temp.write_all(b"test artifact") {
            assert!(false, "temp write failed: {error}");
            return;
        }
        if let Err(error) = temp.flush() {
            assert!(false, "temp flush failed: {error}");
            return;
        }
        let task = TaskConfig {
            mcsm_daemon_id: "daemon".to_string(),
            mcsm_instance_uuid: "instance".to_string(),
            mcsm_target_path: "/plugins/".to_string(),
            ..TaskConfig::default()
        };
        let result = client
            .upload_and_place(&task, temp.path(), "retry-test.jar")
            .await;
        assert!(result.is_ok(), "upload retry should succeed: {result:?}");
        assert_eq!(passport_count.load(Ordering::SeqCst), 2);
        assert_eq!(upload_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cooldown_error_waits_until_operation_is_available() {
        let error = McsmError::Api {
            status: 500,
            message: "api/files/move: 此操作冷却中，约 0 秒后可继续操作！".to_string(),
        };
        assert_eq!(cooldown_delay(&error), Some(Duration::from_secs(1)));
        assert_eq!(cooldown_seconds("约 3 秒后可继续操作"), Some(3));
    }

    #[tokio::test]
    async fn panel_request_retries_after_cooldown_response() {
        use axum::{extract::State, routing::put, Json, Router};
        use serde_json::json;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let attempts = Arc::new(AtomicUsize::new(0));
        let state = Arc::clone(&attempts);
        let app = Router::new()
            .route(
                "/api/files/move",
                put(|State(attempts): State<Arc<AtomicUsize>>| async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Json(json!({
                            "status": 500,
                            "data": "api/files/move: 此操作冷却中，约 0 秒后可继续操作！"
                        }))
                    } else {
                        Json(json!({ "status": 200, "data": { "ok": true } }))
                    }
                }),
            )
            .with_state(state);
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => {
                assert!(false, "bind test listener failed: {error}");
                return;
            }
        };
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => {
                assert!(false, "listener address failed: {error}");
                return;
            }
        };
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut config = test_config();
        config.base_url = format!("http://{address}");
        config.api_min_interval_ms = 0;
        let client = match McsmClient::new(&config) {
            Ok(client) => client,
            Err(error) => {
                assert!(false, "test client construction failed: {error}");
                return;
            }
        };
        let result = client
            .request_json::<Value>(
                Method::PUT,
                "api/files/move",
                &[],
                Some(json!({ "targets": [] })),
            )
            .await;
        assert!(result.is_ok(), "retry should succeed: {result:?}");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
