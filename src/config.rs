//! Configuration model, validation and normalisation.
//!
//! The whole application is configured by a single TOML document. The file is
//! deliberately plain data so it can be edited by hand or through the web UI.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;

fn default_user_agent() -> String {
    format!(
        "MCSM-Auto-Deployer/{} (+https://github.com/)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Update channel represented by a task. The category is optional in the
/// config; when omitted it is inferred from the destination path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    Core,
    Plugin,
    Mod,
    #[default]
    Other,
}

impl TaskCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Plugin => "plugin",
            Self::Mod => "mod",
            Self::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "服务端核心",
            Self::Plugin => "插件",
            Self::Mod => "模组",
            Self::Other => "其他",
        }
    }
}

/// Where a task obtains its artifacts. GitHub remains the default so existing
/// configurations keep working; the other providers are opt-in per task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    #[default]
    Github,
    Modrinth,
    Curseforge,
    Spigot,
}

impl SourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Modrinth => "modrinth",
            Self::Curseforge => "curseforge",
            Self::Spigot => "spigot",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Github => "GitHub Releases",
            Self::Modrinth => "Modrinth",
            Self::Curseforge => "CurseForge",
            Self::Spigot => "SpigotMC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RestartMode {
    /// Call the MCSManager restart endpoint directly.
    #[default]
    Restart,
    /// Stop the instance, wait until it is stopped, then start it again.
    StopThenOpen,
}

impl RestartMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::StopThenOpen => "stop_then_open",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub poll_interval_secs: u64,
    pub state_file: String,
    pub log_history: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".to_string(),
            poll_interval_secs: 600,
            state_file: "data/state.json".to_string(),
            log_history: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McsmConfig {
    pub base_url: String,
    pub api_key: String,
    pub timeout_secs: u64,
    pub restart_mode: RestartMode,
    /// Optional override for the daemon scheme. When unset the scheme of
    /// base_url is used. Useful when the panel is behind HTTPS but the
    /// daemon is reached over plain HTTP on a private network.
    pub daemon_scheme: Option<String>,
    pub accept_invalid_certs: bool,
}

impl Default for McsmConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:23333".to_string(),
            api_key: String::new(),
            timeout_secs: 180,
            restart_mode: RestartMode::Restart,
            daemon_scheme: None,
            accept_invalid_certs: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    pub api_base: String,
    pub token: String,
    pub user_agent: String,
}

/// Settings shared by the non-GitHub artifact providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    pub modrinth_base: String,
    pub curseforge_base: String,
    pub curseforge_api_key: String,
    pub spiget_base: String,
    pub user_agent: String,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            modrinth_base: "https://api.modrinth.com/v2".to_string(),
            curseforge_base: "https://api.curseforge.com/v1".to_string(),
            curseforge_api_key: String::new(),
            spiget_base: "https://api.spiget.org/v2".to_string(),
            user_agent: default_user_agent(),
        }
    }
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.github.com".to_string(),
            token: String::new(),
            user_agent: default_user_agent(),
        }
    }
}

/// A single auto-deployment task. The id is normally generated from the name;
/// it is persisted so that API calls and deployment history stay stable when
/// the display name changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskConfig {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<TaskCategory>,
    pub enabled: bool,
    /// github | modrinth | curseforge | spigot
    #[serde(default)]
    pub source_type: SourceType,
    /// GitHub owner/repo, Modrinth slug/ID, CurseForge mod ID or Spigot resource ID.
    #[serde(default, alias = "github_repo")]
    pub source: String,
    #[serde(default)]
    pub asset_regex: String,
    pub include_prerelease: bool,
    /// Optional platform filters (Modrinth / CurseForge).
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    pub mcsm_daemon_id: String,
    pub mcsm_instance_uuid: String,
    pub mcsm_target_path: String,
    pub restart: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_mode: Option<RestartMode>,
    /// Delete the previously deployed remote file before uploading the new one.
    #[serde(default)]
    pub cleanup_old: bool,
    /// Optional remote-filename regex. This is what makes cleanup work even
    /// after a file was renamed manually on the server.
    #[serde(default)]
    pub cleanup_regex: String,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            category: None,
            enabled: true,
            source_type: SourceType::Github,
            source: String::new(),
            asset_regex: String::new(),
            include_prerelease: false,
            game_versions: Vec::new(),
            loaders: Vec::new(),
            mcsm_daemon_id: String::new(),
            mcsm_instance_uuid: String::new(),
            mcsm_target_path: String::new(),
            restart: true,
            restart_mode: None,
            cleanup_old: false,
            cleanup_regex: String::new(),
        }
    }
}

impl TaskConfig {
    /// The explicit category, or a sensible guess based on the upload target.
    pub fn effective_category(&self) -> TaskCategory {
        if let Some(category) = self.category {
            return category;
        }
        let path = self.mcsm_target_path.to_ascii_lowercase();
        if path.ends_with(".jar") && !path.contains("/plugins/") && !path.contains("/mods/") {
            TaskCategory::Core
        } else if path.contains("/plugins/") || path.ends_with("/plugins") {
            TaskCategory::Plugin
        } else if path.contains("/mods/") || path.ends_with("/mods") {
            TaskCategory::Mod
        } else {
            TaskCategory::Other
        }
    }

    /// A target ending in /, or whose final segment has no extension, is a
    /// directory. MCSM stores instance paths with a leading slash.
    pub fn target_is_dir(&self) -> bool {
        if self.mcsm_target_path.ends_with('/') {
            return true;
        }
        let last = self.mcsm_target_path.rsplit('/').next().unwrap_or_default();
        !last.contains('.')
    }

    pub fn compiled_asset_regex(&self) -> Result<Regex> {
        let pattern = if self.asset_regex.trim().is_empty() {
            ".*"
        } else {
            self.asset_regex.trim()
        };
        Regex::new(pattern).with_context(|| format!("任务「{}」的 asset_regex 无效", self.name))
    }

    pub fn compiled_cleanup_regex(&self) -> Result<Option<Regex>> {
        let pattern = self.cleanup_regex.trim();
        if pattern.is_empty() {
            return Ok(None);
        }
        Regex::new(pattern)
            .map(Some)
            .with_context(|| format!("任务「{}」的 cleanup_regex 无效", self.name))
    }

    pub fn restart_mode(&self, fallback: RestartMode) -> RestartMode {
        self.restart_mode.unwrap_or(fallback)
    }

    pub fn source_id(&self) -> &str {
        self.source.trim()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub mcsm: McsmConfig,
    pub github: GithubConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    pub tasks: Vec<TaskConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            mcsm: McsmConfig::default(),
            github: GithubConfig::default(),
            providers: ProvidersConfig::default(),
            tasks: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件 {}", path.display()))?;
        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("配置文件 {} 的 TOML 格式无效", path.display()))?;
        config.normalize();
        config.validate()?;
        Ok(config)
    }

    /// Trim user input, slugify ids and normalise destination paths. This is
    /// idempotent and safe to call after every web edit.
    pub fn normalize(&mut self) {
        self.server.bind = self.server.bind.trim().to_string();
        if self.server.poll_interval_secs == 0 {
            self.server.poll_interval_secs = 600;
        }
        if self.server.log_history == 0 {
            self.server.log_history = 500;
        }
        self.server.state_file = self.server.state_file.trim().to_string();
        if self.server.state_file.is_empty() {
            self.server.state_file = "data/state.json".to_string();
        }

        self.mcsm.base_url = self.mcsm.base_url.trim().trim_end_matches('/').to_string();
        self.mcsm.api_key = self.mcsm.api_key.trim().to_string();
        self.mcsm.daemon_scheme = self
            .mcsm
            .daemon_scheme
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches("://").to_ascii_lowercase());

        self.github.api_base = self
            .github
            .api_base
            .trim()
            .trim_end_matches('/')
            .to_string();
        if self.github.api_base.is_empty() {
            self.github.api_base = "https://api.github.com".to_string();
        }
        self.github.token = self.github.token.trim().to_string();
        if self.github.user_agent.trim().is_empty() {
            self.github.user_agent = default_user_agent();
        }

        self.providers.modrinth_base = self
            .providers
            .modrinth_base
            .trim()
            .trim_end_matches('/')
            .to_string();
        if self.providers.modrinth_base.is_empty() {
            self.providers.modrinth_base = "https://api.modrinth.com/v2".to_string();
        }
        self.providers.curseforge_base = self
            .providers
            .curseforge_base
            .trim()
            .trim_end_matches('/')
            .to_string();
        if self.providers.curseforge_base.is_empty() {
            self.providers.curseforge_base = "https://api.curseforge.com/v1".to_string();
        }
        self.providers.spiget_base = self
            .providers
            .spiget_base
            .trim()
            .trim_end_matches('/')
            .to_string();
        if self.providers.spiget_base.is_empty() {
            self.providers.spiget_base = "https://api.spiget.org/v2".to_string();
        }
        self.providers.curseforge_api_key = self.providers.curseforge_api_key.trim().to_string();
        if self.providers.user_agent.trim().is_empty() {
            self.providers.user_agent = default_user_agent();
        }

        let mut used_ids = std::collections::HashSet::new();
        for task in &mut self.tasks {
            task.name = task.name.trim().to_string();
            task.source = normalise_source(task.source_type, &task.source);
            task.asset_regex = task.asset_regex.trim().to_string();
            task.game_versions = normalise_list(&task.game_versions);
            task.loaders = normalise_list(&task.loaders);
            task.cleanup_regex = task.cleanup_regex.trim().to_string();
            task.mcsm_daemon_id = task.mcsm_daemon_id.trim().to_string();
            task.mcsm_instance_uuid = task.mcsm_instance_uuid.trim().to_string();
            task.mcsm_target_path = normalize_target_path(&task.mcsm_target_path);
            if task.id.trim().is_empty() {
                task.id = slugify(&task.name);
            } else {
                task.id = slugify(&task.id);
            }
            if task.id.is_empty() {
                task.id = "task".to_string();
            }
            let base = task.id.clone();
            let mut candidate = base.clone();
            let mut suffix = 2_u32;
            while used_ids.contains(&candidate) {
                candidate = format!("{base}-{suffix}");
                suffix = suffix.saturating_add(1);
            }
            task.id = candidate.clone();
            used_ids.insert(candidate);
        }
    }

    pub fn validate(&self) -> Result<()> {
        let _: SocketAddr = self
            .server
            .bind
            .parse()
            .with_context(|| format!("server.bind 不是有效的监听地址: {}", self.server.bind))?;
        if self.server.poll_interval_secs < 30 {
            bail!("server.poll_interval_secs 不能小于 30 秒，以免耗尽 GitHub API 限额");
        }
        if self.server.poll_interval_secs > 86_400 {
            bail!("server.poll_interval_secs 不能大于 86400 秒");
        }
        if self.mcsm.timeout_secs < 10 {
            bail!("mcsm.timeout_secs 不能小于 10 秒");
        }
        let base = reqwest::Url::parse(&self.mcsm.base_url)
            .with_context(|| format!("mcsm.base_url 无效: {}", self.mcsm.base_url))?;
        if base.scheme() != "http" && base.scheme() != "https" {
            bail!("mcsm.base_url 只支持 http/https");
        }
        if let Some(scheme) = &self.mcsm.daemon_scheme {
            if scheme != "http" && scheme != "https" {
                bail!("mcsm.daemon_scheme 只能是 http 或 https");
            }
        }
        let github_base = reqwest::Url::parse(&self.github.api_base)
            .with_context(|| format!("github.api_base 无效: {}", self.github.api_base))?;
        if github_base.scheme() != "http" && github_base.scheme() != "https" {
            bail!("github.api_base 只支持 http/https");
        }
        if self.github.user_agent.trim().is_empty() {
            bail!("github.user_agent 不能为空");
        }
        for (name, value) in [
            ("providers.modrinth_base", &self.providers.modrinth_base),
            ("providers.curseforge_base", &self.providers.curseforge_base),
            ("providers.spiget_base", &self.providers.spiget_base),
        ] {
            let parsed =
                reqwest::Url::parse(value).with_context(|| format!("{name} 无效: {value}"))?;
            if parsed.scheme() != "http" && parsed.scheme() != "https" {
                bail!("{name} 只支持 http/https");
            }
        }
        if self.providers.user_agent.trim().is_empty() {
            bail!("providers.user_agent 不能为空");
        }

        let mut ids = std::collections::HashSet::new();
        for (index, task) in self.tasks.iter().enumerate() {
            let label = if task.name.is_empty() {
                format!("#{}", index + 1)
            } else {
                task.name.clone()
            };
            if task.name.is_empty() {
                bail!("第 {} 个任务缺少 name", index + 1);
            }
            if task.id.is_empty() {
                bail!("任务「{label}」缺少 id");
            }
            if !ids.insert(task.id.clone()) {
                bail!("任务 id 重复: {}", task.id);
            }
            validate_source(task.source_type, &task.source)
                .with_context(|| format!("任务「{label}」的 source 无效"))?;
            if task.source_type == SourceType::Github && task.asset_regex.is_empty() {
                bail!("任务「{label}」使用 GitHub 时 asset_regex 不能为空");
            }
            task.compiled_asset_regex()?;
            task.compiled_cleanup_regex()?;
            if task.cleanup_old && !task.target_is_dir() {
                bail!("任务「{label}」启用旧文件清理时，mcsm_target_path 必须是目录（以 / 结尾）");
            }
            if task.mcsm_daemon_id.is_empty() {
                bail!("任务「{label}」缺少 mcsm_daemon_id");
            }
            if task.mcsm_instance_uuid.is_empty() {
                bail!("任务「{label}」缺少 mcsm_instance_uuid");
            }
            if task.mcsm_target_path.is_empty() || task.mcsm_target_path == "/" {
                bail!("任务「{label}」的 mcsm_target_path 不能为空");
            }
            if task.mcsm_target_path.contains("..") {
                bail!("任务「{label}」的 mcsm_target_path 不允许包含 ..");
            }
        }
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("配置无法序列化为 TOML")
    }
}

fn normalise_source(source_type: SourceType, source: &str) -> String {
    let value = source.trim().to_string();
    match source_type {
        SourceType::Github => normalise_repo(&value),
        _ => value.trim_end_matches('/').to_string(),
    }
}

fn normalise_list(values: &[String]) -> Vec<String> {
    let mut output = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty()
            && !output
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            output.push(trimmed.to_string());
        }
    }
    output
}

fn validate_source(source_type: SourceType, source: &str) -> Result<()> {
    let source = source.trim();
    if source.is_empty() {
        return Err(anyhow!("来源标识不能为空"));
    }
    match source_type {
        SourceType::Github => validate_repo(source),
        SourceType::Modrinth => {
            if source.chars().any(char::is_whitespace) || source.contains('/') {
                return Err(anyhow!("Modrinth 项目标识应为 slug 或 project ID"));
            }
            Ok(())
        }
        SourceType::Curseforge => {
            source
                .parse::<u64>()
                .map_err(|_| anyhow!("CurseForge 项目 ID 必须是数字"))?;
            Ok(())
        }
        SourceType::Spigot => {
            source
                .parse::<u64>()
                .map_err(|_| anyhow!("Spigot 资源 ID 必须是数字"))?;
            Ok(())
        }
    }
}

fn normalise_repo(repo: &str) -> String {
    let mut value = repo.trim().to_string();
    for prefix in [
        "https://github.com/",
        "http://github.com/",
        "https://www.github.com/",
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.trim_end_matches('/').to_string();
            break;
        }
    }
    value
}

fn validate_repo(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(anyhow!("应为 owner/repo 格式"));
    }
    if owner.chars().any(char::is_whitespace) || name.chars().any(char::is_whitespace) {
        return Err(anyhow!("仓库名不能包含空白字符"));
    }
    Ok(())
}

/// Convert a user supplied instance path to an absolute POSIX path. Both
/// plugins and /plugins/ refer to the same directory level; the trailing
/// slash is preserved because it distinguishes a directory from a file target.
pub fn normalize_target_path(input: &str) -> String {
    let trimmed = input.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return String::new();
    }
    let wants_dir = trimmed.ends_with('/');
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        segments.push(segment);
    }
    let joined = segments.join("/");
    let mut result = if joined.is_empty() {
        "/".to_string()
    } else {
        format!("/{joined}")
    };
    if wants_dir && result != "/" && !result.ends_with('/') {
        result.push('/');
    }
    result
}

pub fn slugify(input: &str) -> String {
    let lower = input.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_paths_are_normalised() {
        assert_eq!(normalize_target_path("plugins/"), "/plugins/");
        assert_eq!(normalize_target_path(" /mods "), "/mods");
        assert_eq!(normalize_target_path("server.jar"), "/server.jar");
        assert_eq!(normalize_target_path("/"), "/");
        assert_eq!(normalize_target_path("a//b/../c"), "/a/b/../c");
    }

    #[test]
    fn category_inference() {
        let mut task = TaskConfig::default();
        task.mcsm_target_path = "/server.jar".into();
        assert_eq!(task.effective_category(), TaskCategory::Core);
        task.mcsm_target_path = "/plugins/".into();
        assert_eq!(task.effective_category(), TaskCategory::Plugin);
        task.mcsm_target_path = "/mods/".into();
        assert_eq!(task.effective_category(), TaskCategory::Mod);
    }

    #[test]
    fn legacy_github_repo_is_migrated_to_source() {
        let raw = r#"
[[tasks]]
id = "legacy"
name = "Legacy"
enabled = true
github_repo = "Owner/Repo"
asset_regex = '^Legacy-.*\\.jar$'
mcsm_daemon_id = "daemon"
mcsm_instance_uuid = "instance"
mcsm_target_path = "/plugins/"
restart = true
"#;
        match toml::from_str::<Config>(raw) {
            Ok(mut config) => {
                config.normalize();
                assert_eq!(config.tasks[0].source, "Owner/Repo");
                assert_eq!(config.tasks[0].source_type, SourceType::Github);
                assert!(config.validate().is_ok());
            }
            Err(error) => assert!(false, "legacy config parse failed: {error}"),
        }
    }

    #[test]
    fn slugify_is_stable() {
        assert_eq!(slugify("EssentialsX · Plugins"), "essentialsx-plugins");
        assert_eq!(slugify("服务端 核心"), "");
        let mut config = Config {
            tasks: vec![TaskConfig {
                name: "服务端 核心".to_string(),
                ..TaskConfig::default()
            }],
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.tasks[0].id, "task");
    }
}
