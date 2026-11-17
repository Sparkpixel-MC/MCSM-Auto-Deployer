//! GitHub Releases client with conditional requests (ETag) and asset selection.
//! Streaming downloads live in crate::sources so every provider shares them.

use crate::config::GithubConfig;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use regex::Regex;
use reqwest::{header, Client, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAsset {
    #[serde(default)]
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub browser_download_url: String,
    #[serde(default)]
    pub url: String,
    /// GitHub may expose a digest such as "sha256:...". We still compute our
    /// own digest and cross-check when this field is present.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubRelease {
    #[serde(default)]
    pub id: u64,
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    etag: Option<String>,
    releases: Vec<GitHubRelease>,
}

#[derive(Debug, Clone)]
pub struct PollResult {
    pub releases: Vec<GitHubRelease>,
    pub from_cache: bool,
    pub rate_limit_remaining: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SelectedRelease<'a> {
    pub release: &'a GitHubRelease,
    pub asset: &'a GitHubAsset,
}

pub struct GithubClient {
    api: Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl GithubClient {
    pub fn new() -> Result<Self> {
        let api = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(60))
            .build()
            .context("构建 GitHub API HTTP 客户端失败")?;
        Ok(Self {
            api,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Poll releases with If-None-Match when an ETag is cached. A 304 response
    /// reuses the previous body and does not consume additional API payload.
    pub async fn fetch_releases(&self, repo: &str, config: &GithubConfig) -> Result<PollResult> {
        let cache_key = format!("{}|{repo}", config.api_base);
        let cached = self.cache.lock().get(&cache_key).cloned();
        let base = config.api_base.trim_end_matches('/');
        let mut url = Url::parse(&format!("{base}/repos/{repo}/releases"))
            .with_context(|| format!("GitHub API 地址无效: {base}"))?;
        url.query_pairs_mut().append_pair("per_page", "10");

        let mut request = self
            .api
            .get(url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header(header::USER_AGENT, &config.user_agent)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if !config.token.trim().is_empty() {
            request = request.bearer_auth(config.token.trim());
        }
        if let Some(entry) = &cached {
            if let Some(etag) = &entry.etag {
                request = request.header(header::IF_NONE_MATCH, etag);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|error| anyhow!("GitHub 请求失败: {}", error.without_url()))?;
        let status = response.status();
        let remaining = header_u64(&response, "x-ratelimit-remaining");

        if status == StatusCode::NOT_MODIFIED {
            if let Some(entry) = cached {
                tracing::debug!(repo = %repo, "GitHub Releases ETag 命中 (304)");
                return Ok(PollResult {
                    releases: entry.releases,
                    from_cache: true,
                    rate_limit_remaining: remaining,
                });
            }
            bail!("GitHub 返回 304 但本地没有可用的 ETag 缓存");
        }

        if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
            let reset = header_u64(&response, "x-ratelimit-reset");
            let body = response.text().await.unwrap_or_default();
            let hint = if config.token.trim().is_empty() {
                "请在设置中配置 GitHub Token 以提高限额"
            } else {
                "当前 Token 限额可能已用尽"
            };
            bail!(
                "GitHub API 限流 (HTTP {status})，剩余 {remaining:?} 次，窗口重置时间戳 {reset:?}。{hint}。{}",
                truncate(&body, 200)
            );
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let hint = match status {
                StatusCode::NOT_FOUND => "仓库不存在，或 Token 无权访问",
                StatusCode::UNAUTHORIZED => "GitHub Token 无效",
                _ => "请检查网络和 GitHub API 配置",
            };
            bail!(
                "GitHub API 返回 HTTP {status}（{hint}）: {}",
                truncate(&body, 300)
            );
        }

        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .text()
            .await
            .context("读取 GitHub Releases 响应失败")?;
        let releases: Vec<GitHubRelease> =
            serde_json::from_str(&body).context("解析 GitHub Releases JSON 失败")?;
        self.cache.lock().insert(
            cache_key,
            CacheEntry {
                etag,
                releases: releases.clone(),
            },
        );
        tracing::debug!(
            repo = %repo,
            releases = releases.len(),
            rate_limit_remaining = ?remaining,
            "GitHub Releases 已刷新"
        );
        Ok(PollResult {
            releases,
            from_cache: false,
            rate_limit_remaining: remaining,
        })
    }
}

pub fn select_asset<'a>(
    releases: &'a [GitHubRelease],
    pattern: &Regex,
    include_prerelease: bool,
) -> Result<SelectedRelease<'a>> {
    let mut inspected = 0_usize;
    let mut prereleases_skipped = 0_usize;
    for release in releases {
        if release.draft {
            continue;
        }
        if release.prerelease && !include_prerelease {
            prereleases_skipped = prereleases_skipped.saturating_add(1);
            continue;
        }
        inspected = inspected.saturating_add(1);
        for asset in &release.assets {
            if pattern.is_match(&asset.name) {
                return Ok(SelectedRelease { release, asset });
            }
        }
    }
    if inspected == 0 && prereleases_skipped > 0 {
        bail!(
            "没有匹配的正式 Release（已跳过 {} 个预发布版本）。如需使用预发布，请开启任务选项",
            prereleases_skipped
        );
    }
    bail!(
        "在 {} 个 Release 中未找到匹配 {} 的资产",
        inspected,
        pattern.as_str()
    )
}

fn header_u64(response: &Response, name: &str) -> Option<u64> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let shortened: String = text.chars().take(max).collect();
    format!("{shortened}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> GitHubRelease {
        GitHubRelease {
            id: 1,
            tag_name: tag.to_string(),
            name: Some(tag.to_string()),
            body: None,
            draft: false,
            prerelease,
            html_url: format!("https://github.com/example/repo/releases/tag/{tag}"),
            published_at: None,
            assets: assets
                .iter()
                .map(|name| GitHubAsset {
                    id: 1,
                    name: (*name).to_string(),
                    size: 10,
                    content_type: "application/java-archive".to_string(),
                    browser_download_url: "https://example.com/file.jar".to_string(),
                    url: String::new(),
                    digest: None,
                })
                .collect(),
        }
    }

    #[test]
    fn selects_newest_matching_asset() {
        let releases = vec![
            release("v2", false, &["readme.md", "server-v2.jar"]),
            release("v1", false, &["server-v1.jar"]),
        ];
        let Ok(regex) = Regex::new(r"^server-.*\.jar$") else {
            assert!(false, "test regex failed to compile");
            return;
        };
        match select_asset(&releases, &regex, false) {
            Ok(selected) => {
                assert_eq!(selected.release.tag_name, "v2");
                assert_eq!(selected.asset.name, "server-v2.jar");
            }
            Err(error) => assert!(false, "selection failed: {error}"),
        }
    }

    #[test]
    fn skips_prerelease_by_default() {
        let releases = vec![
            release("v3-beta", true, &["server-v3.jar"]),
            release("v2", false, &["server-v2.jar"]),
        ];
        let Ok(regex) = Regex::new(r"^server-.*\.jar$") else {
            assert!(false, "test regex failed to compile");
            return;
        };
        match select_asset(&releases, &regex, false) {
            Ok(selected) => assert_eq!(selected.release.tag_name, "v2"),
            Err(error) => assert!(false, "selection failed: {error}"),
        }
    }
}
