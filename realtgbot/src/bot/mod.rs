//! Telegram bot: command routing and handler dispatch.
//!
//! This module owns all `teloxide` types. `trading.rs` is never imported
//! directly by handlers — all calls go through `TradingClient`.

mod commands;
mod handlers;
mod alerts_runner;
pub mod state;

pub use state::BotState;

use crate::{config::Config, trading::TradingClient};
use teloxide::{
    dispatching::UpdateFilterExt,
    dptree,
    prelude::*,
    types::Message,
    utils::command::BotCommands,
};
use tracing::info;

/// Start the bot and block until a shutdown signal is received.
pub async fn run(cfg: Config, trading: TradingClient) -> anyhow::Result<()> {
    let bot = Bot::new(&cfg.telegram_token);

    // Register all commands with Telegram so they show in the menu
    bot.set_my_commands(commands::Command::bot_commands())
        .await?;

    info!("Bot started — listening for commands");

    // Persistent stores for encrypted keys and price alerts (supports configurable DATA_DIR e.g. /data)
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".".into());
    info!(data_dir = %data_dir, "Persistent storage directory configured");
    let users_path = format!("{data_dir}/users.json");
    let alerts_path = format!("{data_dir}/alerts.json");

    let users = std::sync::Arc::new(crate::users::UserStore::new(users_path));
    let alerts = std::sync::Arc::new(crate::alerts::AlertStore::new(alerts_path));

    // Shared application state threaded through every handler
    let state = std::sync::Arc::new(state::BotState::new(cfg, trading, users, alerts));

    // Spawn background price alert checker
    let alert_checker_bot = bot.clone();
    let alert_checker_state = state.clone();
    tokio::spawn(async move {
        alerts_runner::run_alert_checker(alert_checker_bot, alert_checker_state).await;
    });

    // Build the dispatcher: messages (commands & conversation) + button callbacks
    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<commands::Command>()
                        .endpoint(handlers::handle_command),
                )
                .branch(dptree::endpoint(handlers::handle_message)),
        )
        .branch(
            Update::filter_callback_query()
                .endpoint(handlers::handle_callback),
        );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
