//! MCSM-Auto-Deployer — single-binary Rust deployment service.

mod api;
mod config;
mod engine;
mod github;
mod logs;
mod mcsm;
mod sources;
mod state;
mod web;

use anyhow::{bail, Context, Result};
use axum::Router;
use config::Config;
use engine::Engine;
use logs::{LogBus, LogLayer};
use state::{is_real_secret, AppState};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = parse_config_path()?;
    let config = Config::load(&config_path)?;
    let log_bus = LogBus::new(config.server.log_history);
    init_logging(Arc::clone(&log_bus));

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %config_path.display(),
        "MCSM-Auto-Deployer 正在启动"
    );
    warn_about_placeholders(&config);

    let state = AppState::new(config.clone(), config_path.clone(), Arc::clone(&log_bus))
        .context("初始化应用状态失败")?;
    let engine = Engine::new(Arc::clone(&state));
    engine.spawn_scheduler();
    engine.spawn_restart_scheduler();

    let app = Router::new()
        .nest("/api", api::router())
        .fallback(web::static_handler)
        .with_state(Arc::clone(&state));

    let listener = TcpListener::bind(&config.server.bind)
        .await
        .with_context(|| format!("无法监听 {}", config.server.bind))?;
    tracing::info!(
        bind = %config.server.bind,
        url = %format!("http://{}", config.server.bind),
        "Web 控制台已就绪"
    );
    tracing::info!(
        tasks = config.tasks.len(),
        interval_secs = config.server.poll_interval_secs,
        "已加载自动部署任务"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Axum 服务异常退出")?;
    tracing::info!("服务已停止");
    Ok(())
}

fn init_logging(log_bus: Arc<LogBus>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,mcsm_auto_deployer=debug"));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(LogLayer::new(log_bus))
        .with(tracing_subscriber::fmt::layer().with_target(false));
    if let Err(error) = registry.try_init() {
        eprintln!("日志订阅器初始化失败: {error}");
    }
}

fn warn_about_placeholders(config: &Config) {
    if !is_real_secret(&config.mcsm.api_key) {
        tracing::warn!("MCSM API Key 尚未配置，上传与重启操作将无法执行");
    }
    if config.github.token.trim().is_empty() {
        tracing::warn!("未配置 GitHub Token；匿名 API 限额为每小时 60 次，建议在设置中补充");
    }
    if config.tasks.is_empty() {
        tracing::warn!("当前没有任何更新任务，请点击右下角 + 新建任务");
    }
    let placeholder_tasks = config
        .tasks
        .iter()
        .filter(|task| {
            task.mcsm_daemon_id.starts_with("YOUR_") || task.mcsm_instance_uuid.starts_with("YOUR_")
        })
        .count();
    if placeholder_tasks > 0 {
        tracing::warn!(
            count = placeholder_tasks,
            "部分任务仍使用占位 daemon/instance UUID，请先完成配置"
        );
    }
}

fn parse_config_path() -> Result<PathBuf> {
    let mut args = std::env::args().skip(1);
    let mut path = std::env::var("MCSM_CONFIG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" | "-c" => {
                let value = args.next().context("--config 需要一个文件路径参数")?;
                path = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!(
                    "MCSM-Auto-Deployer {}\n\n用法: mcsm-auto-deployer [--config <config.toml>]\n\n环境变量:\n  MCSM_CONFIG   配置文件路径（默认 config.toml）\n  RUST_LOG      日志过滤级别（默认 info,mcsm_auto_deployer=debug）",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("MCSM-Auto-Deployer {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other if !other.starts_with('-') && path.is_none() => {
                path = Some(PathBuf::from(other));
            }
            other => {
                bail!("未知命令行参数: {other}（使用 --help 查看帮助）");
            }
        }
    }
    Ok(path.unwrap_or_else(|| PathBuf::from("config.toml")))
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => Some(signal),
                Err(error) => {
                    tracing::warn!(error = %error, "无法注册 SIGTERM 处理器，仅监听 Ctrl+C");
                    None
                }
            };
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::warn!(error = %error, "Ctrl+C 信号监听失败");
                }
            }
            _ = async {
                if let Some(signal) = terminate.as_mut() {
                    signal.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %error, "Ctrl+C 信号监听失败");
        }
    }
    tracing::info!("收到退出信号，正在优雅关闭");
}
