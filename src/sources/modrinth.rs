//! Modrinth v2: https://docs.modrinth.com/api/

use crate::config::{ProvidersConfig, TaskConfig};
use crate::sources::{get_json, ResolvedArtifact};
use anyhow::{bail, Context, Result};
use regex::Regex;
use reqwest::{Client, Url};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct ModrinthVersion {
    id: String,
    name: String,
    version_number: String,
    version_type: String,
    #[serde(default)]
    date_published: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    files: Vec<ModrinthFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthFile {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    size: u64,
}

pub async fn resolve(
    client: &Client,
    config: &ProvidersConfig,
    task: &TaskConfig,
) -> Result<ResolvedArtifact> {
    let project = task.source_id();
    let base = config.modrinth_base.trim_end_matches('/');
    let mut url = Url::parse(&format!("{base}/project/{project}/version"))
        .with_context(|| format!("Modrinth API 地址无效: {base}"))?;
    {
        let mut query = url.query_pairs_mut();
        if !task.game_versions.is_empty() {
            query.append_pair(
                "game_versions",
                &serde_json::to_string(&task.game_versions)
                    .context("无法编码 Modrinth game_versions")?,
            );
        }
        if !task.loaders.is_empty() {
            query.append_pair(
                "loaders",
                &serde_json::to_string(&task.loaders).context("无法编码 Modrinth loaders")?,
            );
        }
    }
    let mut versions: Vec<ModrinthVersion> =
        get_json(client, url, &config.user_agent, "Modrinth", &[]).await?;
    versions.sort_by(|left, right| right.date_published.cmp(&left.date_published));
    let regex = task.compiled_asset_regex()?;
    let mut inspected = 0_usize;
    let mut skipped_prerelease = 0_usize;
    for version in &versions {
        if !task.include_prerelease && version.version_type != "release" {
            skipped_prerelease = skipped_prerelease.saturating_add(1);
            continue;
        }
        if !versions_match_filters(task, version) {
            continue;
        }
        inspected = inspected.saturating_add(1);
        if let Some(file) = choose_file(version, &regex) {
            if file.url.trim().is_empty() {
                bail!(
                    "Modrinth 版本 {} 的文件缺少下载地址",
                    version.version_number
                );
            }
            return Ok(ResolvedArtifact {
                provider: crate::config::SourceType::Modrinth,
                version: version.version_number.clone(),
                release_name: if version.name.trim().is_empty() {
                    version.version_number.clone()
                } else {
                    version.name.clone()
                },
                release_url: Some(format!(
                    "https://modrinth.com/project/{project}/version/{}",
                    version.id
                )),
                published_at: version.date_published.clone(),
                asset_name: file.filename.clone(),
                url_download: Some(file.url.clone()),
                download_url: file.url.clone(),
                size: file.size,
                expected_sha256: None,
                from_cache: false,
                rate_limit_remaining: None,
            });
        }
    }
    if inspected == 0 && skipped_prerelease > 0 {
        bail!(
            "没有匹配的 Modrinth 正式版本（已跳过 {skipped_prerelease} 个 beta/alpha 版本）。可在任务中开启预发布",
        );
    }
    bail!(
        "在 {} 个 Modrinth 版本中未找到匹配 {} 的资产（项目 {project}）",
        inspected,
        regex.as_str()
    )
}

fn versions_match_filters(task: &TaskConfig, version: &ModrinthVersion) -> bool {
    let game_ok = task.game_versions.is_empty()
        || task.game_versions.iter().any(|wanted| {
            version
                .game_versions
                .iter()
                .any(|actual| actual.eq_ignore_ascii_case(wanted))
        });
    let loader_ok = task.loaders.is_empty()
        || task.loaders.iter().any(|wanted| {
            version
                .loaders
                .iter()
                .any(|actual| actual.eq_ignore_ascii_case(wanted))
        });
    game_ok && loader_ok
}

fn choose_file<'a>(version: &'a ModrinthVersion, regex: &Regex) -> Option<&'a ModrinthFile> {
    let matching: Vec<&ModrinthFile> = version
        .files
        .iter()
        .filter(|file| regex.is_match(&file.filename))
        .collect();
    matching
        .iter()
        .copied()
        .find(|file| file.primary)
        .or_else(|| matching.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_release_and_loader() {
        let task = TaskConfig {
            game_versions: vec!["1.21.1".to_string()],
            loaders: vec!["fabric".to_string()],
            ..TaskConfig::default()
        };
        let version = ModrinthVersion {
            id: "x".to_string(),
            name: "v".to_string(),
            version_number: "1.0".to_string(),
            version_type: "release".to_string(),
            date_published: None,
            game_versions: vec!["1.21.1".to_string()],
            loaders: vec!["fabric".to_string()],
            files: vec![],
        };
        assert!(versions_match_filters(&task, &version));
    }
}
