mod config;
mod http;
// Opened at startup and held in `AppState`; most of the API is called only by the
// OCI routes that come next (THL-39, THL-41, THL-53).
#[cfg_attr(not(test), allow(dead_code, unused_imports))]
mod oci;
#[cfg_attr(not(test), allow(dead_code, unused_imports))]
mod store;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use config::{Config, LogFormat};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser)]
#[command(name = "tiny-repo", version, about)]
struct Cli {
    /// Path to the TOML config file. A missing file is fine: defaults and env apply.
    #[arg(
        long,
        global = true,
        env = "TINY_REPO_CONFIG",
        default_value = "tiny-repo.toml"
    )]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server.
    Serve,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config = Config::load(&cli.config).context("invalid configuration")?;

    init_tracing(&config);

    match cli.command {
        Command::Serve => serve(config).await,
    }
}

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_new(&config.log.level).unwrap_or(EnvFilter::new("info"));

    let builder = fmt().with_env_filter(filter);

    match config.log.format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Text => builder.init(),
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let storage = store::open(&config.storage, &config.meta, &config.uploads)
        .await
        .context("open storage")?;
    let state = Arc::new(http::AppState { storage });

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind {}", config.listen))?;

    tracing::info!(addr = %config.listen, "listening");

    axum::serve(listener, http::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "ctrl-c handler failed");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => tracing::error!(%err, "SIGTERM handler failed"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutting down");
}
