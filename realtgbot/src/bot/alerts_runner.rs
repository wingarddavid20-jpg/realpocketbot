//! Background worker that polls Real.xyz mark prices and triggers user price alerts.

use std::{collections::HashMap, sync::Arc, time::Duration};
use teloxide::{
    prelude::*,
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, ParseMode},
};
use tracing::{error, info};

use super::state::BotState;

/// Background task checking active alerts every 10 seconds and refreshing dynamic markets every 3 minutes.
pub async fn run_alert_checker(bot: Bot, state: Arc<BotState>) {
    info!("Starting background price alert checker and market refresher");

    let mut tick_count: u64 = 0;

    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        tick_count = tick_count.wrapping_add(1);

        // Every 3 minutes (18 * 10s = 180s), refresh dynamic market list from indexer
        if tick_count % 18 == 0 {
            let refreshed = state.trading.get_markets().await;
            tracing::debug!(count = refreshed.len(), "Background market list refreshed");
        }

        let active_alerts = state.alerts.get_active_alerts().await;
        if active_alerts.is_empty() {
            continue;
        }

        // Cache prices for checked markets in this tick
        let mut prices: HashMap<String, f64> = HashMap::new();

        for alert in &active_alerts {
            let market_id = &alert.market_id;
            if !prices.contains_key(market_id) {
                if let Ok(price) = state.trading.get_market_mark_price(market_id).await {
                    prices.insert(market_id.clone(), price);
                }
            }

            let current_price = match prices.get(market_id) {
                Some(&p) => p,
                None => continue,
            };

            let condition_met = if alert.trigger_above {
                current_price >= alert.target_price
            } else {
                current_price <= alert.target_price
            };

            if condition_met {
                info!(
                    user_id = alert.telegram_id,
                    symbol = %alert.symbol,
                    target = alert.target_price,
                    current = current_price,
                    "Price alert triggered!"
                );

                if let Err(e) = state.alerts.mark_triggered(&alert.id).await {
                    error!(error = %e, "Failed to mark alert triggered");
                }

                let sym_lower = alert.symbol.to_lowercase();
                let keyboard = InlineKeyboardMarkup::new(vec![
                    vec![
                        InlineKeyboardButton::callback(
                            format!("📈 Long {}", alert.symbol),
                            format!("trade:long:{sym_lower}"),
                        ),
                        InlineKeyboardButton::callback(
                            format!("📉 Short {}", alert.symbol),
                            format!("trade:short:{sym_lower}"),
                        ),
                    ],
                    vec![
                        InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
                    ],
                ]);

                let arrow = if alert.trigger_above { "🔺" } else { "🔻" };
                let msg = format!(
                    "🔔 *PRICE ALERT TRIGGERED!* {arrow}\n\n\
                     • Market: *{}*\n\
                     • Target Price: *${:.2}*\n\
                     • Current Price: *${:.2}*\n\n\
                     Take action below:",
                    alert.symbol,
                    alert.target_price,
                    current_price
                );

                let _ = bot
                    .send_message(ChatId(alert.chat_id), msg)
                    .reply_markup(keyboard)
                    .parse_mode(ParseMode::Markdown)
                    .await;
            }
        }
    }
}
