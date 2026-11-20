//! CurseForge for Studios API: https://docs.curseforge.com/rest-api/

use crate::config::{ProvidersConfig, SourceType, TaskConfig};
use crate::sources::{get_json, ResolvedArtifact};
use anyhow::{bail, Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseResponse {
    #[serde(default)]
    data: Vec<CurseFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseFile {
    id: u64,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    release_type: u8,
    #[serde(default)]
    file_date: Option<String>,
    #[serde(default)]
    file_length: u64,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
}

pub async fn resolve(
    client: &Client,
    config: &ProvidersConfig,
    task: &TaskConfig,
) -> Result<ResolvedArtifact> {
    if config.curseforge_api_key.trim().is_empty() || config.curseforge_api_key.starts_with("YOUR_")
    {
        bail!("CurseForge 需要 API Key，请先在设置中配置 providers.curseforge_api_key");
    }
    let project = task.source_id();
    let mod_id: u64 = project.parse().context("CurseForge 项目 ID 必须是数字")?;
    let base = config.curseforge_base.trim_end_matches('/');
    let mut url = Url::parse(&format!("{base}/mods/{mod_id}/files"))
        .with_context(|| format!("CurseForge API 地址无效: {base}"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("pageSize", "50");
        query.append_pair("index", "0");
        if let Some(game_version) = task.game_versions.first() {
            query.append_pair("gameVersion", game_version);
        }
        if let Some(loader) = task.loaders.first() {
            let loader_type = loader_type(loader)
                .with_context(|| format!("不支持的 CurseForge 加载器: {loader}"))?;
            query.append_pair("modLoaderType", &loader_type.to_string());
        }
    }
    let response: CurseResponse = get_json(
        client,
        url,
        &config.user_agent,
        "CurseForge",
        &[("x-api-key", config.curseforge_api_key.trim())],
    )
    .await?;
    let regex = task.compiled_asset_regex()?;
    let mut files = response.data;
    files.sort_by(|left, right| right.file_date.cmp(&left.file_date));
    let mut inspected = 0_usize;
    let mut skipped_prerelease = 0_usize;
    for file in files {
        if !task.include_prerelease && file.release_type != 1 {
            skipped_prerelease = skipped_prerelease.saturating_add(1);
            continue;
        }
        if !game_versions_match(task, &file) {
            continue;
        }
        inspected = inspected.saturating_add(1);
        if !regex.is_match(&file.file_name) {
            continue;
        }
        let Some(download_url) = file
            .download_url
            .clone()
            .filter(|url| !url.trim().is_empty())
        else {
            bail!(
                "CurseForge 文件 {}（id={}）未提供 downloadUrl，作者可能禁用了第三方分发",
                file.file_name,
                file.id
            );
        };
        return Ok(ResolvedArtifact {
            provider: SourceType::Curseforge,
            version: if file.display_name.trim().is_empty() {
                file.file_name.clone()
            } else {
                file.display_name.clone()
            },
            release_name: if file.display_name.trim().is_empty() {
                file.file_name.clone()
            } else {
                file.display_name.clone()
            },
            release_url: Some(format!("https://www.curseforge.com/projects/{mod_id}")),
            published_at: file.file_date.clone(),
            asset_name: file.file_name,
            url_download: Some(download_url.clone()),
            download_url,
            size: file.file_length,
            expected_sha256: None,
            from_cache: false,
            rate_limit_remaining: None,
        });
    }
    if inspected == 0 && skipped_prerelease > 0 {
        bail!(
            "没有匹配的 CurseForge 正式版本（已跳过 {skipped_prerelease} 个 beta/alpha 文件）。可在任务中开启预发布",
        );
    }
    bail!(
        "未找到匹配 {} 的 CurseForge 文件（项目 {mod_id}），请检查游戏版本、加载器和预发布选项",
        regex.as_str()
    )
}

fn game_versions_match(task: &TaskConfig, file: &CurseFile) -> bool {
    if task.game_versions.is_empty() {
        return true;
    }
    task.game_versions.iter().any(|wanted| {
        file.game_versions
            .iter()
            .any(|actual| actual.eq_ignore_ascii_case(wanted))
    })
}

fn loader_type(loader: &str) -> Option<u8> {
    match loader.trim().to_ascii_lowercase().as_str() {
        "forge" => Some(1),
        "liteloader" => Some(3),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_loaders() {
        assert_eq!(loader_type("fabric"), Some(4));
        assert_eq!(loader_type("NeoForge"), Some(6));
        assert_eq!(loader_type("unknown"), None);
    }
}
