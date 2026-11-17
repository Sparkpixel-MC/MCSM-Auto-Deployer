//! Artifact provider abstraction.
//!
//! GitHub, Modrinth, CurseForge and Spigot return different JSON shapes but
//! all ultimately resolve to one downloadable file. The deployment engine only
//! deals with ResolvedArtifact, so adding a provider does not change the
//! upload/cleanup/restart pipeline.

pub mod curseforge;
pub mod modrinth;
pub mod spigot;

use crate::config::{Config, SourceType, TaskConfig};
use crate::github::{select_asset, GithubClient};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use reqwest::{header, Client, StatusCode};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct ResolvedArtifact {
    pub provider: SourceType,
    pub version: String,
    pub release_name: String,
    pub release_url: Option<String>,
    pub published_at: Option<String>,
    pub asset_name: String,
    pub download_url: String,
    pub size: u64,
    pub expected_sha256: Option<String>,
    pub from_cache: bool,
    pub rate_limit_remaining: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DownloadOutcome {
    pub bytes: u64,
    pub sha256: String,
}

pub struct SourceManager {
    github: GithubClient,
    api: Client,
    download: Client,
}

impl SourceManager {
    pub fn new() -> Result<Self> {
        let api = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(60))
            .build()
            .context("构建内容平台 API HTTP 客户端失败")?;
        let download = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .context("构建资产下载 HTTP 客户端失败")?;
        Ok(Self {
            github: GithubClient::new().context("无法初始化 GitHub HTTP 客户端")?,
            api,
            download,
        })
    }

    pub async fn resolve(&self, task: &TaskConfig, config: &Config) -> Result<ResolvedArtifact> {
        match task.source_type {
            SourceType::Github => self.resolve_github(task, config).await,
            SourceType::Modrinth => modrinth::resolve(&self.api, &config.providers, task).await,
            SourceType::Curseforge => curseforge::resolve(&self.api, &config.providers, task).await,
            SourceType::Spigot => spigot::resolve(&self.api, &config.providers, task).await,
        }
    }

    async fn resolve_github(&self, task: &TaskConfig, config: &Config) -> Result<ResolvedArtifact> {
        let repo = task.source_id();
        let poll = self
            .github
            .fetch_releases(repo, &config.github)
            .await
            .with_context(|| format!("轮询 GitHub 仓库 {repo} 失败"))?;
        let regex = task.compiled_asset_regex()?;
        let selected = select_asset(&poll.releases, &regex, task.include_prerelease)
            .with_context(|| format!("为任务「{}」选择 GitHub Release 资产失败", task.name))?;
        let release_url = if selected.release.html_url.trim().is_empty() {
            Some(format!(
                "https://github.com/{repo}/releases/tag/{}",
                selected.release.tag_name
            ))
        } else {
            Some(selected.release.html_url.clone())
        };
        let download_url = if selected.asset.url.trim().is_empty() {
            selected.asset.browser_download_url.clone()
        } else {
            selected.asset.url.clone()
        };
        if download_url.is_empty() {
            bail!("GitHub 资产没有可用的下载地址");
        }
        Ok(ResolvedArtifact {
            provider: SourceType::Github,
            version: selected.release.tag_name.clone(),
            release_name: selected
                .release
                .name
                .clone()
                .unwrap_or_else(|| selected.release.tag_name.clone()),
            release_url,
            published_at: selected.release.published_at.clone(),
            asset_name: selected.asset.name.clone(),
            download_url,
            size: selected.asset.size,
            expected_sha256: selected
                .asset
                .digest
                .as_deref()
                .and_then(|digest| digest.strip_prefix("sha256:"))
                .map(str::to_string),
            from_cache: poll.from_cache,
            rate_limit_remaining: poll.rate_limit_remaining,
        })
    }

    /// Stream a provider artifact to disk while computing SHA-256. GitHub
    /// tokens are attached only for GitHub downloads; direct CDN downloads from
    /// Modrinth/CurseForge/Spiget stay token-free.
    pub async fn download<F>(
        &self,
        artifact: &ResolvedArtifact,
        config: &Config,
        destination: &Path,
        mut progress: F,
    ) -> Result<DownloadOutcome>
    where
        F: FnMut(u64, Option<u64>),
    {
        let user_agent = match artifact.provider {
            SourceType::Github => &config.github.user_agent,
            _ => &config.providers.user_agent,
        };
        let mut request = self
            .download
            .get(&artifact.download_url)
            .header(header::ACCEPT, "application/octet-stream")
            .header(header::USER_AGENT, user_agent);
        if artifact.provider == SourceType::Github && !config.github.token.trim().is_empty() {
            request = request.bearer_auth(config.github.token.trim());
        }
        if artifact.provider == SourceType::Spigot {
            request = request.header(header::REFERER, "https://www.spigotmc.org/");
        }
        let response = request.send().await.map_err(|error| {
            anyhow!(
                "下载 {} 资产失败: {}",
                artifact.provider.label(),
                error.without_url()
            )
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!(
                "下载 {} 资产失败 (HTTP {status}): {}",
                artifact.provider.label(),
                truncate(&body, 260)
            );
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if content_type.contains("text/html") {
            bail!(
                "{} 返回了 HTML 页面而不是文件，可能需要登录、作者禁用了下载，或该 Spigot 资源为外部托管",
                artifact.provider.label()
            );
        }
        let total = response.content_length().or({
            if artifact.size > 0 {
                Some(artifact.size)
            } else {
                None
            }
        });
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("无法创建下载目录 {}", parent.display()))?;
        }
        let mut file = tokio::fs::File::create(destination)
            .await
            .with_context(|| format!("无法创建临时文件 {}", destination.display()))?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha256::new();
        let mut downloaded = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| anyhow!("下载流中断: {}", error.without_url()))?;
            file.write_all(&chunk)
                .await
                .with_context(|| format!("写入临时文件 {} 失败", destination.display()))?;
            hasher.update(&chunk);
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            progress(downloaded, total);
        }
        file.flush().await.context("刷新临时文件失败")?;
        file.sync_all().await.context("同步临时文件失败")?;
        if let Some(expected) = total {
            if downloaded != expected {
                let _ = tokio::fs::remove_file(destination).await;
                bail!("下载不完整: 期望 {expected} 字节，实际 {downloaded} 字节");
            }
        }
        let digest = hex_digest(hasher.finalize().as_slice());
        if let Some(expected) = &artifact.expected_sha256 {
            if !digest.eq_ignore_ascii_case(expected) {
                let _ = tokio::fs::remove_file(destination).await;
                bail!("服务端声明的 SHA-256 与下载内容不一致");
            }
        }
        Ok(DownloadOutcome {
            bytes: downloaded,
            sha256: digest,
        })
    }
}

pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: reqwest::Url,
    user_agent: &str,
    provider: &str,
    headers: &[(&str, &str)],
) -> Result<T> {
    let mut request = client
        .get(url)
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, user_agent);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request
        .send()
        .await
        .map_err(|error| anyhow!("{provider} API 请求失败: {}", error.without_url()))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        bail!("{provider} 未找到对应项目或版本 (HTTP 404)");
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        bail!("{provider} 拒绝访问 (HTTP {status})，请检查 API Key 或请求限制");
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!(
            "{provider} API 返回 HTTP {status}: {}",
            truncate(&body, 300)
        );
    }
    let body = response
        .text()
        .await
        .with_context(|| format!("读取 {provider} 响应失败"))?;
    serde_json::from_str(&body)
        .with_context(|| format!("解析 {provider} JSON 响应失败: {}", truncate(&body, 200)))
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let shortened: String = text.chars().take(max).collect();
    format!("{shortened}…")
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub(crate) fn safe_asset_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("artifact").trim();
    let base = if base.is_empty() { "artifact" } else { base };
    let sanitized: String = base
        .chars()
        .map(|character| match character {
            '\0' | '/' | '\\' => '_',
            other => other,
        })
        .collect();
    if sanitized.ends_with(".jar") || sanitized.ends_with(".zip") {
        sanitized
    } else {
        format!("{sanitized}.jar")
    }
}
