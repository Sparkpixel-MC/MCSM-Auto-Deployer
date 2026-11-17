//! MCSManager client.
//!
//! The file upload deliberately uses the daemon passport flow. The panel is
//! only used to sign a one-time upload passport; file bytes never pass through
//! the panel API.

use crate::config::{McsmConfig, RestartMode, TaskConfig};
use crate::mcsm::types::{
    ApiEnvelope, DeleteFilesRequest, FileEntry, FileListData, InstanceDetail,
    InstanceOperationData, MoveFilesRequest, UploadOutcome, UploadPassport, UploadTarget,
};
use reqwest::{Client, Method, Response, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
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

#[derive(Debug, Clone)]
pub struct McsmClient {
    http: Client,
    base: Url,
    api_key: String,
    daemon_scheme: String,
    timeout: Duration,
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
        })
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
        let mut request = self.panel_request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(sanitize_network_error)?;
        self.parse_envelope(response, path).await
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
        let url = self.panel_url(
            "api/files/upload",
            &[
                ("upload_dir", &target.upload_dir),
                ("daemonId", daemon_id),
                ("uuid", instance_uuid),
            ],
        )?;
        tracing::debug!(
            upload_dir = %target.upload_dir,
            "向 MCSM 面板申请一次性上传凭证"
        );
        let response = self
            .panel_request(Method::POST, url)
            .send()
            .await
            .map_err(sanitize_network_error)?;
        self.parse_envelope::<UploadPassport>(response, "api/files/upload")
            .await
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
            format!(
                "{}://{}/upload/{}",
                self.daemon_scheme, addr, passport.password
            )
        } else {
            return Err(McsmError::MissingUploadAddress);
        };

        let absolute = if raw.contains("://") {
            raw
        } else {
            format!("{}://{}", self.daemon_scheme, raw)
        };
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

        tracing::info!(
            file_name = %file_name,
            bytes = size,
            "文件直传 MCSM Daemon（不经过面板 API）"
        );
        let response = self
            .http
            .post(url.clone())
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "*/*")
            .multipart(form)
            .send()
            .await
            .map_err(sanitize_network_error)?;
        let status = response.status();
        let body = response.text().await.map_err(sanitize_network_error)?;
        if !status.is_success() {
            return Err(McsmError::Upload {
                status: status.as_u16(),
                message: truncate(&body, 300),
            });
        }
        // Some daemon versions answer with a JSON envelope even on 200.
        if body.trim_start().starts_with('{') {
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                if let Some(inner_status) = value.get("status").and_then(Value::as_i64) {
                    if inner_status != 200 {
                        return Err(McsmError::Upload {
                            status: status.as_u16(),
                            message: truncate(&body, 300),
                        });
                    }
                }
            }
        }
        Ok(size)
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
        let _: Value = self
            .request_json(
                Method::PUT,
                "api/files/move",
                &[("daemonId", daemon_id), ("uuid", instance_uuid)],
                Some(serde_json::to_value(request).map_err(McsmError::Decode)?),
            )
            .await?;
        Ok(())
    }

    /// Full upload flow: passport → direct daemon upload → optional atomic
    /// rename so a core target such as /server.jar receives the right name.
    pub async fn upload_and_place(
        &self,
        task: &TaskConfig,
        local_path: &Path,
        asset_name: &str,
    ) -> Result<UploadOutcome, McsmError> {
        let target = Self::resolve_target(task, asset_name)?;
        let passport = self
            .fetch_upload_passport(&target, &task.mcsm_daemon_id, &task.mcsm_instance_uuid)
            .await?;
        let upload_url = self.daemon_upload_url(&passport)?;
        let bytes = self
            .upload_bytes_to_daemon(&upload_url, local_path, &safe_file_name(asset_name)?)
            .await?;
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
        Ok(UploadOutcome {
            upload_dir: target.upload_dir,
            uploaded_path: target.uploaded_path,
            final_path: target.final_path,
            bytes,
        })
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
                tracing::info!("调用 MCSM 重启实例");
                self.instance_action("restart", daemon_id, instance_uuid)
                    .await
            }
            RestartMode::StopThenOpen => {
                tracing::info!("调用 MCSM 停止实例，等待停止后重新启动");
                self.instance_action("stop", daemon_id, instance_uuid)
                    .await?;
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
                        return Err(McsmError::StopTimeout);
                    }
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                }
                tracing::info!("实例已停止，调用 MCSM 启动实例");
                self.instance_action("open", daemon_id, instance_uuid).await
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
    }
}
