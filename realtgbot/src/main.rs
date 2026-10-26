//! Pocket Real - Telegram Perp Trading Bot for Real.xyz
//!
//! A Telegram bot that interfaces with the Real Markets perp DEX on IOTA via
//! the official Real.xyz Rust SDK. Private key and object IDs are loaded from
//! environment variables; never hard-coded.

mod config;
mod error;
mod crypto;
mod users;
mod alerts;
mod trading;
mod bot;

use anyhow::Context;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

fn main() -> anyhow::Result<()> {
    // Spawn a dedicated thread with 16 MB stack to avoid Windows 1 MB stack limits
    std::thread::Builder::new()
        .name("bot-main".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(16 * 1024 * 1024)
                .build()
                .expect("Failed to build Tokio runtime");
            rt.block_on(run_bot())
        })?
        .join()
        .map_err(|e| anyhow::anyhow!("Bot thread panicked: {:?}", e))?
}

async fn run_bot() -> anyhow::Result<()> {
    // Load .env file if present (dev convenience; production uses real env vars)
    dotenvy::dotenv().ok();

    // Initialise structured logging; defaults to INFO if RUST_LOG is unset
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("realtgbot=info")),
        )
        .compact()
        .init();

    info!("Starting Pocket Real Telegram trading bot…");

    // Load and validate configuration from environment
    let cfg = config::Config::from_env().context("failed to load configuration")?;

    info!(
        network = %cfg.network,
        account = %cfg.account_id,
        "Configuration loaded"
    );

    // Build the Real.xyz signing client once; share it across all handlers
    let trading_client = trading::TradingClient::new(&cfg)
        .await
        .context("failed to initialise trading client")?;

    info!("Trading client connected");

    // Preload dynamic markets from Real.xyz indexer on startup
    let initial_markets = trading_client.get_markets().await;
    info!(count = initial_markets.len(), "Preloaded live markets from indexer");

    // Optional health check server for cloud container hosts (Hugging Face Spaces, Render, etc.)
    let port = std::env::var("PORT").unwrap_or_else(|_| "7860".to_string());
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        if let Ok(listener) = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await {
            info!(port = %port, "Cloud health check listener active");
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 15\r\n\r\nPocket Real OK\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                }
            }
        }
    });

    // Start the Telegram bot (blocks until shutdown)
    bot::run(cfg, trading_client).await
}
