use std::sync::Arc;

use clap::Parser;
use gemini_rust_gateway::{
    config::Config, error::GatewayError, gemini::GeminiClient, handlers,
    observability::metrics::Metrics, tasks::TaskQueue,
};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

mod commands;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = commands::Cli::parse();
    match cli.command {
        Some(commands::Command::Init {
            force,
            non_interactive,
        }) => return commands::init(force, non_interactive),
        Some(commands::Command::Doctor { online }) => return commands::doctor(online).await,
        Some(commands::Command::Serve) | None => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .compact()
        .init();

    let config = Arc::new(Config::load()?);
    config.validate().map_err(|error| match error {
        GatewayError::InvalidRequest(message) => {
            anyhow::anyhow!("config validation failed: {message}")
        }
        other => anyhow::anyhow!("config validation failed: {other}"),
    })?;
    print_banner(&config);
    let metrics = Metrics::new();
    let gemini = match GeminiClient::initialize(config.clone()).await {
        Ok(client) => Arc::new(client),
        Err(error) if config.cookie_sync_enabled => {
            tracing::warn!(
                error = %error,
                "Gemini session initialization failed; starting in degraded cookie-sync mode"
            );
            Arc::new(GeminiClient::degraded(config.clone())?)
        }
        Err(error) => return Err(error.into()),
    };
    gemini.start_health_monitor().await;
    let tasks = TaskQueue::start(config.clone(), gemini.clone()).await?;
    let router = handlers::router(config.clone(), gemini.clone(), tasks.clone(), metrics);
    let listener = TcpListener::bind((config.host.as_str(), config.port)).await?;

    tracing::info!(address = %listener.local_addr()?, "Gemini Rust gateway listening");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    tracing::info!("HTTP server stopped, draining task queue");
    tasks.shutdown().await;
    gemini.shutdown().await;
    tracing::info!("shutdown complete");
    Ok(())
}

fn print_banner(config: &Config) {
    tracing::info!(
        host = %config.host,
        port = config.port,
        default_model = %config.default_model,
        advanced_model = %config.advanced_model,
        enabled_generations = ?config.enabled_generations,
        max_concurrency = config.max_concurrency,
        task_workers = config.task_workers,
        "starting Gemini Rust gateway"
    );
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("termination handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl-C, starting graceful shutdown"),
        _ = terminate => tracing::info!("received SIGTERM, starting graceful shutdown"),
    }
}
