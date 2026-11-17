//! SpigotMC resources via the community Spiget API.
//!
//! Spigot has no official public API. Spiget can redirect to files hosted on
//! Spigot's CDN; externally hosted resources only expose a page URL, which we
//! deliberately refuse instead of downloading HTML.
//!
//! Important limitation: Spiget's download redirect may require cookies or
//! Cloudflare clearance for some resources. In that case use the GitHub or
//! Modrinth source for the same project.

use crate::config::{ProvidersConfig, SourceType, TaskConfig};
use crate::sources::{get_json, safe_asset_name, ResolvedArtifact};
use anyhow::{bail, Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpigotResource {
    #[serde(default)]
    name: String,
    #[serde(default)]
    external: bool,
    #[serde(default)]
    file: Option<SpigotFile>,
    #[serde(default)]
    premium: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpigotFile {
    #[serde(default)]
    external: bool,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    external_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpigotVersion {
    #[serde(default)]
    name: String,
}

pub async fn resolve(
    client: &Client,
    config: &ProvidersConfig,
    task: &TaskConfig,
) -> Result<ResolvedArtifact> {
    let resource_id: u64 = task
        .source_id()
        .parse()
        .context("Spigot 资源 ID 必须是数字")?;
    let base = config.spiget_base.trim_end_matches('/');
    let resource_url = Url::parse(&format!("{base}/resources/{resource_id}"))
        .with_context(|| format!("Spiget API 地址无效: {base}"))?;
    let resource: SpigotResource =
        get_json(client, resource_url, &config.user_agent, "Spiget", &[]).await?;
    if resource.premium {
        bail!("Spigot 付费资源无法通过 Spiget 自动下载");
    }
    let latest_url = Url::parse(&format!("{base}/resources/{resource_id}/versions/latest"))
        .with_context(|| format!("Spiget API 地址无效: {base}"))?;
    let latest: SpigotVersion =
        get_json(client, latest_url, &config.user_agent, "Spiget", &[]).await?;
    let file = resource.file.as_ref();
    let external = resource.external || file.map(|value| value.external).unwrap_or(false);
    let download_url = if external {
        let external_url = file
            .and_then(|value| value.external_url.as_deref())
            .unwrap_or("")
            .trim();
        if !looks_like_archive(external_url) {
            bail!(
                "Spigot 资源 {} 是外部托管，Spiget 只提供外部页面 URL，无法可靠直链下载。请改用该项目的 GitHub/Modrinth 来源",
                resource_id
            );
        }
        external_url.to_string()
    } else {
        format!("{base}/resources/{resource_id}/versions/latest/download")
    };
    let asset_name = safe_asset_name(&resource.name);
    Ok(ResolvedArtifact {
        provider: SourceType::Spigot,
        version: if latest.name.trim().is_empty() {
            format!("v{resource_id}")
        } else {
            latest.name.clone()
        },
        release_name: if latest.name.trim().is_empty() {
            resource.name.clone()
        } else {
            format!("{} {}", resource.name, latest.name)
        },
        release_url: Some(format!("https://www.spigotmc.org/resources/{resource_id}/")),
        published_at: None,
        asset_name,
        download_url,
        size: file.map(|value| value.size).unwrap_or(0),
        expected_sha256: None,
        from_cache: false,
        rate_limit_remaining: None,
    })
}

fn looks_like_archive(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.ends_with(".jar")
        || lower.ends_with(".zip")
        || lower.contains(".jar?")
        || lower.contains(".zip?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_archive_detection() {
        assert!(looks_like_archive("https://example.com/plugin.jar"));
        assert!(looks_like_archive(
            "https://example.com/plugin.jar?download=1"
        ));
        assert!(!looks_like_archive(
            "https://github.com/example/plugin/releases/tag/v1"
        ));
    }
}
