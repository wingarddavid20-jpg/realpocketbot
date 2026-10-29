//! Command handlers and interactive inline button menu flow.
//!
//! Handlers call `TradingClient` methods and format results for Telegram.
//! No SDK types are used here; all trading logic lives in `trading.rs`.

use std::{sync::Arc, time::Duration};
use teloxide::{
    prelude::*,
    types::{CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode},
};
use tracing::{error, info, warn};

use super::{
    commands::Command,
    state::{BotState, PendingTrade, TradeDirection, TradeStep},
};
use crate::error::BotError;

// ─── Inline Keyboards ────────────────────────────────────────────────────────

pub fn main_menu_keyboard(is_connected: bool) -> InlineKeyboardMarkup {
    if is_connected {
        InlineKeyboardMarkup::new(vec![
            vec![
                InlineKeyboardButton::callback("📍 Positions", "action:positions"),
                InlineKeyboardButton::callback("📋 Orders", "action:orders"),
            ],
            vec![
                InlineKeyboardButton::callback("📊 Markets", "menu:markets"),
                InlineKeyboardButton::callback("💰 Balance", "action:balance"),
            ],
            vec![
                InlineKeyboardButton::callback("📈 Volume", "action:volume"),
                InlineKeyboardButton::callback("🔔 Alerts", "menu:alerts"),
            ],
            vec![
                InlineKeyboardButton::callback("📜 History", "action:history"),
                InlineKeyboardButton::callback("🔌 Disconnect Wallet", "action:disconnect"),
            ],
        ])
    } else {
        InlineKeyboardMarkup::new(vec![
            vec![
                InlineKeyboardButton::callback("🔗 Connect Wallet", "action:connect"),
            ],
            vec![
                InlineKeyboardButton::callback("📊 Markets", "menu:markets"),
                InlineKeyboardButton::callback("🔔 Alerts", "menu:alerts"),
            ],
        ])
    }
}

pub fn connect_prompt_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
    ]])
}

pub fn unconnected_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("🔗 Connect Wallet", "action:connect"),
        ],
        vec![
            InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
        ],
    ])
}

pub fn asset_icon(symbol: &str) -> &'static str {
    match symbol.to_uppercase().as_str() {
        "BTC" => "₿",
        "ETH" => "⟠",
        "SOL" => "◎",
        "HYPE" => "⚡",
        "SUI" => "💧",
        "IOTA" => "🪙",
        "DOGE" => "🐕",
        "XRP" => "✕",
        _ => "📊",
    }
}

pub fn markets_keyboard(markets: &[crate::trading::MarketInfo]) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();
    for m in markets {
        let icon = asset_icon(&m.base_asset);
        let change_str = match m.price_change_24h {
            Some(pc) => {
                let pc_val = if pc.abs() < 1.0 { pc * 100.0 } else { pc };
                let sign = if pc_val >= 0.0 { "+" } else { "" };
                format!(" ({sign}{pc_val:.1}%)")
            }
            None => String::new(),
        };
        let label = if m.mark_price > 0.0 {
            format!("{icon} {} • ${}{change_str}", m.base_asset, format_usd(m.mark_price))
        } else {
            format!("{icon} {}", m.symbol)
        };
        rows.push(vec![
            InlineKeyboardButton::callback(label, format!("market:{}", m.short_name)),
        ]);
    }
    rows.push(vec![
        InlineKeyboardButton::callback("⬅️ Back", "menu:main"),
    ]);
    InlineKeyboardMarkup::new(rows)
}

pub fn market_actions_keyboard(symbol: &str) -> InlineKeyboardMarkup {
    let sym_lower = symbol.to_lowercase();
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(format!("📈 Long {symbol}"), format!("trade:long:{sym_lower}")),
            InlineKeyboardButton::callback(format!("📉 Short {symbol}"), format!("trade:short:{sym_lower}")),
        ],
        vec![
            InlineKeyboardButton::callback(format!("❌ Close {symbol}"), format!("menu:close:{sym_lower}")),
            InlineKeyboardButton::callback("📋 Orders", "action:orders"),
        ],
        vec![
            InlineKeyboardButton::callback(format!("🔔 Alert {symbol}"), format!("alert:market:{sym_lower}")),
            InlineKeyboardButton::callback("⬅️ Back", "menu:markets"),
        ],
    ])
}

pub fn close_options_keyboard(symbol: &str) -> InlineKeyboardMarkup {
    let sym_lower = symbol.to_lowercase();
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("25%", format!("close:{sym_lower}:25")),
            InlineKeyboardButton::callback("50%", format!("close:{sym_lower}:50")),
            InlineKeyboardButton::callback("100%", format!("close:{sym_lower}:100")),
        ],
        vec![
            InlineKeyboardButton::callback("✏️ Custom", format!("close_custom:{sym_lower}")),
        ],
        vec![
            InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
        ],
    ])
}

pub fn order_size_keyboard(back_callback: Option<&str>) -> InlineKeyboardMarkup {
    let mut rows = vec![
        vec![
            InlineKeyboardButton::callback("25%", "size_pct:25"),
            InlineKeyboardButton::callback("50%", "size_pct:50"),
            InlineKeyboardButton::callback("100%", "size_pct:100"),
        ],
    ];
    let mut nav_row = vec![
        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
    ];
    if let Some(back) = back_callback {
        nav_row.push(InlineKeyboardButton::callback("⬅️ Back", back.to_string()));
    }
    rows.push(nav_row);
    InlineKeyboardMarkup::new(rows)
}

pub fn order_placed_keyboard(symbol: &str) -> InlineKeyboardMarkup {
    let sym_lower = symbol.to_lowercase();
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
        ],
        vec![
            InlineKeyboardButton::callback("❌ Close Position", format!("menu:close:{sym_lower}")),
        ],
    ])
}

pub fn pending_order_placed_keyboard(_symbol: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("📋 View Open Orders", "action:orders"),
        ],
        vec![
            InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
        ],
    ])
}

pub fn leverage_keyboard(max_leverage: Option<f64>) -> InlineKeyboardMarkup {
    let max = max_leverage.unwrap_or(20.0).max(1.0);
    let mut options = Vec::new();
    for &opt in &[2.0, 5.0, 10.0, 20.0] {
        if opt < max {
            options.push(opt);
        }
    }
    if !options.contains(&max) {
        options.push(max);
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if options.len() <= 4 {
        let row: Vec<InlineKeyboardButton> = options
            .into_iter()
            .map(|opt| {
                InlineKeyboardButton::callback(format!("{opt:.0}x"), format!("lev:{opt:.0}"))
            })
            .collect();
        rows.push(row);
    } else {
        let row1: Vec<InlineKeyboardButton> = options[..3]
            .iter()
            .map(|opt| {
                InlineKeyboardButton::callback(format!("{opt:.0}x"), format!("lev:{opt:.0}"))
            })
            .collect();
        let row2: Vec<InlineKeyboardButton> = options[3..]
            .iter()
            .map(|opt| {
                InlineKeyboardButton::callback(format!("{opt:.0}x"), format!("lev:{opt:.0}"))
            })
            .collect();
        rows.push(row1);
        rows.push(row2);
    }
    rows.push(vec![InlineKeyboardButton::callback("❌ Cancel", "action:cancel")]);
    InlineKeyboardMarkup::new(rows)
}

pub fn order_type_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⚡ Market Order", "ordertype:market"),
            InlineKeyboardButton::callback("🎯 Limit Order", "ordertype:limit"),
        ],
        vec![
            InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
        ],
    ])
}

pub fn alerts_menu_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("➕ Set Alert", "alerts:new"),
            InlineKeyboardButton::callback("📋 My Alerts", "alerts:list"),
        ],
        vec![
            InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
        ],
    ])
}

pub fn alert_markets_keyboard(markets: &[crate::trading::MarketInfo]) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();
    let mut current_row = Vec::new();
    for m in markets {
        let icon = asset_icon(&m.base_asset);
        current_row.push(InlineKeyboardButton::callback(
            format!("{icon} {}", m.base_asset),
            format!("alert:market:{}", m.short_name),
        ));
        if current_row.len() == 2 {
            rows.push(std::mem::take(&mut current_row));
        }
    }
    if !current_row.is_empty() {
        rows.push(current_row);
    }
    rows.push(vec![
        InlineKeyboardButton::callback("⬅️ Back", "menu:alerts"),
    ]);
    InlineKeyboardMarkup::new(rows)
}

pub fn back_to_main_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
        ],
    ])
}

// ─── Edit or Send Helper ──────────────────────────────────────────────────────

async fn edit_or_send(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    text: &str,
    keyboard: InlineKeyboardMarkup,
) -> ResponseResult<()> {
    if let Some(mid) = message_id {
        let res = bot
            .edit_message_text(chat_id, mid, text)
            .reply_markup(keyboard.clone())
            .parse_mode(ParseMode::Markdown)
            .await;
        match res {
            Ok(_) => return Ok(()),
            Err(ref e) => {
                let err_str = e.to_string();
                if err_str.contains("message is not modified") {
                    return Ok(());
                }
                warn!("edit_message_text markdown error: {e}, retrying without parse_mode");
                let res_plain = bot
                    .edit_message_text(chat_id, mid, text)
                    .reply_markup(keyboard.clone())
                    .await;
                match res_plain {
                    Ok(_) => return Ok(()),
                    Err(ref ep) => {
                        let ep_str = ep.to_string();
                        if ep_str.contains("message is not modified") {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    let send_res = bot
        .send_message(chat_id, text)
        .reply_markup(keyboard.clone())
        .parse_mode(ParseMode::Markdown)
        .await;

    if send_res.is_ok() {
        return Ok(());
    }

    // Fallback: send as plain text without Markdown parsing if Markdown entity parsing failed
    match bot.send_message(chat_id, text).reply_markup(keyboard).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.to_string().contains("message is not modified") {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

// ─── Dispatcher Entry Points ──────────────────────────────────────────────────

pub async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let (user_id, username) = match msg.from.as_ref() {
        Some(u) => (u.id.0, u.username.clone()),
        None => (chat_id.0 as u64, None),
    };
    let user_display = username.as_deref().unwrap_or("unknown");

    info!(cmd = ?cmd, user = user_display, user_id, "Received command");

    state.clear_session(chat_id).await;

    let result = match cmd {
        Command::Help => Box::pin(cmd_help(&bot, chat_id, user_id, &state)).await,
        Command::Start => Box::pin(cmd_start(&bot, chat_id, user_id, &state)).await,
        Command::Connect(args) => {
            if !args.trim().is_empty() {
                let _ = bot.delete_message(chat_id, msg.id).await;
            }
            Box::pin(cmd_connect(&bot, chat_id, user_id, username, &state, &args)).await
        }
        Command::Disconnect => Box::pin(cmd_disconnect(&bot, chat_id, user_id, &state)).await,
        Command::Balance => Box::pin(cmd_balance(&bot, chat_id, user_id, &state)).await,
        Command::Volume => Box::pin(cmd_volume(&bot, chat_id, user_id, &state)).await,
        Command::Long(args) => Box::pin(cmd_long(&bot, chat_id, user_id, &state, &args)).await,
        Command::Short(args) => Box::pin(cmd_short(&bot, chat_id, user_id, &state, &args)).await,
        Command::Close(args) => Box::pin(cmd_close(&bot, chat_id, user_id, &state, &args)).await,
        Command::Positions => Box::pin(cmd_positions(&bot, chat_id, user_id, &state)).await,
        Command::Orders => Box::pin(cmd_orders(&bot, chat_id, user_id, &state)).await,
        Command::Alert(args) => Box::pin(cmd_alert(&bot, chat_id, user_id, &state, &args)).await,
        Command::History => Box::pin(cmd_history(&bot, chat_id, user_id, &state)).await,
        Command::Admin => Box::pin(cmd_admin(&bot, chat_id, user_id, &state)).await,
        Command::Cancel => Box::pin(cmd_cancel(&bot, chat_id)).await,
    };

    if let Err(ref e) = result {
        error!(error = %e, "Handler returned error");
    }

    result
}

pub async fn handle_message(bot: Bot, msg: Message, state: Arc<BotState>) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let (user_id, username) = match msg.from.as_ref() {
        Some(u) => (u.id.0, u.username.clone()),
        None => (chat_id.0 as u64, None),
    };
    let text = match msg.text() {
        Some(t) => t.trim(),
        None => return Ok(()),
    };

    if text.eq_ignore_ascii_case("/cancel") {
        state.clear_session(chat_id).await;
        bot.send_message(chat_id, "❌ Cancelled.")
            .reply_markup(back_to_main_keyboard())
            .await?;
        return Ok(());
    }

    let session = state.get_session(chat_id).await;
    match session {
        Some(pending) => {
            if let TradeStep::AwaitingPrivateKey = pending.step {
                // Instantly delete the user's message containing private key
                let _ = bot.delete_message(chat_id, msg.id).await;
                state.clear_session(chat_id).await;
                Box::pin(handle_connect_private_key(&bot, chat_id, user_id, username, &state, text)).await
            } else {
                Box::pin(handle_trade_step(&bot, chat_id, user_id, &state, pending, text)).await
            }
        }
        None => {
            let is_connected = state.users.get_user(user_id).await.is_some();
            bot.send_message(
                chat_id,
                "❓ Tap a button below or send /start to open the menu:",
            )
            .reply_markup(main_menu_keyboard(is_connected))
            .await?;
            Ok(())
        }
    }
}

pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(q.id.clone()).await;

    let (chat_id, message_id) = match q.message {
        Some(ref m) => (m.chat().id, m.id()),
        None => return Ok(()),
    };

    let user_id = q.from.id.0;
    let username = q.from.username.clone();

    let data = match q.data {
        Some(ref d) => d.as_str(),
        None => return Ok(()),
    };

    info!(callback_data = data, user_id, "Received callback query");

    if data == "menu:main" {
        state.clear_session(chat_id).await;
        Box::pin(show_main_menu(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if data == "action:connect" {
        Box::pin(cmd_connect(&bot, chat_id, user_id, username, &state, "")).await?;
    } else if data == "action:disconnect" {
        Box::pin(cmd_disconnect(&bot, chat_id, user_id, &state)).await?;
    } else if data == "menu:markets" {
        Box::pin(show_markets_menu(&bot, chat_id, Some(message_id), &state)).await?;
    } else if let Some(sym) = data.strip_prefix("market:") {
        Box::pin(show_market_actions_menu(&bot, chat_id, Some(message_id), sym, &state)).await?;
    } else if let Some(sym) = data.strip_prefix("menu:close:") {
        if state.users.get_user(user_id).await.is_none() {
            Box::pin(show_not_connected_notice(&bot, chat_id, Some(message_id))).await?;
        } else {
            Box::pin(show_close_options_menu(&bot, chat_id, Some(message_id), sym)).await?;
        }
    } else if let Some(rest) = data.strip_prefix("trade:") {
        if state.users.get_user(user_id).await.is_none() {
            Box::pin(show_not_connected_notice(&bot, chat_id, Some(message_id))).await?;
        } else {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                let dir = if parts[0] == "long" {
                    TradeDirection::Long
                } else {
                    TradeDirection::Short
                };
                Box::pin(start_trade_from_button(&bot, chat_id, Some(message_id), user_id, &state, dir, parts[1])).await?;
            }
        }
    } else if let Some(rest) = data.strip_prefix("close:") {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() == 2 {
            let symbol = parts[0];
            let pct: f64 = parts[1].parse().unwrap_or(100.0);
            Box::pin(execute_button_close(&bot, chat_id, Some(message_id), user_id, &state, symbol, pct)).await?;
        }
    } else if let Some(sym) = data.strip_prefix("close_custom:") {
        if state.users.get_user(user_id).await.is_none() {
            Box::pin(show_not_connected_notice(&bot, chat_id, Some(message_id))).await?;
        } else {
            Box::pin(prompt_custom_close_percentage(&bot, chat_id, Some(message_id), sym, &state)).await?;
        }
    } else if let Some(pct_str) = data.strip_prefix("size_pct:") {
        if let Ok(pct) = pct_str.parse::<f64>() {
            Box::pin(handle_order_size_percentage(&bot, chat_id, user_id, &state, pct)).await?;
        }
    } else if let Some(lev_str) = data.strip_prefix("lev:") {
        if let Ok(lev) = lev_str.parse::<f64>() {
            Box::pin(handle_leverage_selection(&bot, chat_id, user_id, &state, lev)).await?;
        }
    } else if let Some(choice_str) = data.strip_prefix("ordertype:") {
        Box::pin(handle_order_type_choice(&bot, chat_id, user_id, &state, choice_str)).await?;
    } else if data == "action:positions" {
        Box::pin(show_positions_view(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if data == "action:orders" {
        Box::pin(show_orders_view(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if let Some(rest) = data.strip_prefix("cancel_order:") {
        Box::pin(handle_cancel_order_button(&bot, chat_id, Some(message_id), user_id, &state, rest)).await?;
    } else if let Some(rest) = data.strip_prefix("cnl:") {
        Box::pin(handle_cancel_order_button(&bot, chat_id, Some(message_id), user_id, &state, rest)).await?;
    } else if data == "action:history" {
        Box::pin(show_history_view(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if data == "menu:alerts" {
        Box::pin(show_alerts_menu(&bot, chat_id, Some(message_id))).await?;
    } else if data == "alerts:new" {
        Box::pin(show_alert_market_select(&bot, chat_id, Some(message_id), &state)).await?;
    } else if data == "alerts:list" {
        Box::pin(show_alerts_list(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if let Some(sym) = data.strip_prefix("alert:market:") {
        Box::pin(start_alert_setup(&bot, chat_id, Some(message_id), &state, sym)).await?;
    } else if let Some(alert_id) = data.strip_prefix("alert:del:") {
        Box::pin(handle_delete_alert(&bot, chat_id, Some(message_id), user_id, &state, alert_id)).await?;
    } else if data == "action:balance" {
        Box::pin(show_balance_view(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if data == "action:volume" {
        Box::pin(show_volume_view(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    } else if data == "action:cancel" {
        state.clear_session(chat_id).await;
        Box::pin(show_main_menu(&bot, chat_id, Some(message_id), user_id, &state)).await?;
    }

    Ok(())
}

// ─── Menu Views ───────────────────────────────────────────────────────────────

async fn show_main_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let network_str = state.config.network.to_string();
    let maybe_user = state.users.get_user(user_id).await;

    let (status_line, is_connected) = match maybe_user {
        Some(ref u) => {
            let short = if u.address.len() > 14 {
                format!("{}…{}", &u.address[..8], &u.address[u.address.len() - 6..])
            } else {
                u.address.clone()
            };
            (format!("🟢 Connected: `{short}`\nAccount ID: `{}`", u.account_id), true)
        }
        None => ("⚪ Wallet: *Not Connected* (Tap Connect Wallet to start)".to_string(), false),
    };

    let text = format!(
        "💼 *Pocket Real*\n\
         _Fast & Secure Perp Trading on IOTA_\n\n\
         Network: `{}`\n\
         {}\n\n\
         Select an option below:",
        network_str,
        status_line
    );
    edit_or_send(bot, chat_id, message_id, &text, main_menu_keyboard(is_connected)).await
}

async fn show_markets_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    state: &BotState,
) -> ResponseResult<()> {
    send_typing(bot, chat_id).await;
    let markets = state.trading.get_markets().await;
    let text = "📊 *Markets*\n\nSelect an asset to trade:";
    edit_or_send(bot, chat_id, message_id, text, markets_keyboard(&markets)).await
}

async fn show_market_actions_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    symbol: &str,
    state: &BotState,
) -> ResponseResult<()> {
    let sym_upper = symbol.to_uppercase();
    let icon = asset_icon(&sym_upper);

    let market_id = state.resolve_market(symbol).unwrap_or_else(|| sym_upper.clone());
    let stats_res = state.trading.get_market_stats(&market_id).await;

    let stats_text = match stats_res {
        Ok(s) => {
            let funding_line = match s.funding_rate {
                Some(fr) => {
                    let fr_pct = fr * 100.0;
                    let sign = if fr_pct >= 0.0 { "+" } else { "" };
                    let cd_str = match s.next_funding_time_ms {
                        Some(ms) => {
                            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
                            if ms > now_ms {
                                let diff_min = (ms - now_ms) / 60000;
                                let h = diff_min / 60;
                                let m = diff_min % 60;
                                format!(" (in {h}h {m}m)")
                            } else {
                                String::new()
                            }
                        }
                        None => String::new(),
                    };
                    format!("• Funding Rate: `{sign}{fr_pct:.4}% / 8h`{cd_str}\n")
                }
                None => "• Funding Rate: `N/A`\n".to_string(),
            };

            let change_line = match s.price_change_24h {
                Some(pc) => {
                    let sign = if pc >= 0.0 { "+" } else { "" };
                    format!("• 24h Change: `{sign}{pc:.2}%`\n")
                }
                None => String::new(),
            };

            format!(
                "• Mark Price: *${}*\n\
                 {}{}",
                format_usd(s.mark_price),
                change_line,
                funding_line
            )
        }
        Err(_) => "• Price unavailable".to_string(),
    };

    let text = format!(
        "{icon} *{sym_upper} / USDT Perpetual*\n\n\
         {}\n\
         Select an action for *{sym_upper}*:",
        stats_text
    );
    edit_or_send(
        bot,
        chat_id,
        message_id,
        &text,
        market_actions_keyboard(&sym_upper),
    )
    .await
}

async fn show_close_options_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    symbol: &str,
) -> ResponseResult<()> {
    let sym_upper = symbol.to_uppercase();
    let text = format!(
        "❌ *Close {sym_upper} Position*\n\n\
         Select what percentage of your position to close:",
    );
    edit_or_send(
        bot,
        chat_id,
        message_id,
        &text,
        close_options_keyboard(&sym_upper),
    )
    .await
}

async fn prompt_custom_close_percentage(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    symbol: &str,
    state: &BotState,
) -> ResponseResult<()> {
    let market_id = state.resolve_market(symbol).unwrap_or_else(|| symbol.to_string());
    let sym_upper = BotState::market_name(&market_id);

    let pending = PendingTrade {
        direction: TradeDirection::Long,
        market_id: market_id.clone(),
        market_symbol: sym_upper.clone(),
        step: TradeStep::AwaitingClosePercentage {
            market_id,
            symbol: sym_upper.clone(),
        },
    };
    state.set_session(chat_id, pending).await;

    let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
        InlineKeyboardButton::callback("⬅️ Back", format!("menu:close:{}", symbol.to_lowercase())),
    ]]);

    let text = format!(
        "✏️ *Close {sym_upper} Position — Custom Percentage*\n\n\
         How much of your position do you want to close?\n\
         Reply with a percentage (e.g. `10%`, `75%`, `33.3%`):"
    );

    edit_or_send(bot, chat_id, message_id, &text, cancel_kb).await
}

async fn show_not_connected_notice(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let text = "⚠️ *Wallet Not Connected*\n\n\
                You must connect your IOTA wallet before viewing your balance or placing trades.\n\n\
                Tap *Connect Wallet* below to link your account securely:";
    edit_or_send(bot, chat_id, message_id, text, unconnected_keyboard()).await
}

async fn show_balance_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let (account_id, indexer_id) = match user_opt {
        Some(ref u) => (u.account_id.clone(), u.indexer_account_id.clone()),
        None => {
            return show_not_connected_notice(bot, chat_id, message_id).await;
        }
    };

    match state.trading.get_user_balance(&account_id, indexer_id.as_deref()).await {
        Ok(bal) => {
            let text = format!(
                "💰 *Balance*\n\n\
                 • Total Equity: *${}*\n\
                 • Available Margin: *${}*",
                format_usd(bal.total_equity),
                format_usd(bal.available_margin),
            );
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
        }
        Err(e) => {
            let err_text = format!("❌ *Failed to fetch balance*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &err_text, back_to_main_keyboard()).await?;
        }
    }
    Ok(())
}

async fn show_volume_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let (account_id, indexer_id) = match user_opt {
        Some(ref u) => (u.account_id.clone(), u.indexer_account_id.clone()),
        None => {
            return show_not_connected_notice(bot, chat_id, message_id).await;
        }
    };

    match state.trading.get_user_volume(&account_id, indexer_id.as_deref()).await {
        Ok(vol) => {
            let mut lines = vec![format!("• Total: *${}*", format_volume(vol.total))];
            if !vol.by_market.is_empty() {
                let mut sorted_markets: Vec<_> = vol.by_market.iter().collect();
                sorted_markets.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (sym, notional) in sorted_markets {
                    let icon = asset_icon(sym);
                    lines.push(format!("• {icon} {sym}: *${}*", format_volume(*notional)));
                }
            } else {
                lines.push(format!("• ₿ BTC: *${}*", format_volume(vol.btc)));
                lines.push(format!("• ⟠ ETH: *${}*", format_volume(vol.eth)));
                lines.push(format!("• ◎ SOL: *${}*", format_volume(vol.sol)));
            }
            let text = format!("📊 *Trading Volume*\n\n{}", lines.join("\n"));
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
        }
        Err(e) => {
            let err_text = format!("❌ *Failed to fetch volume*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &err_text, back_to_main_keyboard()).await?;
        }
    }
    Ok(())
}

async fn show_positions_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let user = match user_opt {
        Some(u) => u,
        None => return show_not_connected_notice(bot, chat_id, message_id).await,
    };

    send_typing(bot, chat_id).await;

    match state
        .trading
        .get_user_all_open_positions(&user.account_id, user.indexer_account_id.as_deref())
        .await
    {
        Ok(positions) if positions.is_empty() => {
            let text = "📋 *Open Positions*\n\nYou currently have no open positions.";
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback("📊 Open a Position", "menu:markets")],
                vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
            ]);
            edit_or_send(bot, chat_id, message_id, text, kb).await?;
        }
        Ok(positions) => {
            let mut text = String::from("📋 *Your Open Positions*\n\n");
            let mut close_buttons = Vec::new();

            for pos in &positions {
                let sym = BotState::market_name(&pos.market_id);
                let pnl_emoji = if pos.unrealized_pnl >= 0.0 { "🟢" } else { "🔴" };
                let pnl_sign = if pos.unrealized_pnl >= 0.0 { "+" } else { "" };
                let roe_sign = if pos.roe_pct >= 0.0 { "+" } else { "" };

                text.push_str(&format!(
                    "*{sym}* | *{dir}*\n\
                     • Size: `{size:.4}` {sym} (${notional})\n\
                     • Entry Price: `${entry}`\n\
                     • Mark Price: `${mark}`\n\
                     • PnL: {pnl_emoji} *{pnl_sign}${pnl}* ({roe_sign}{roe:.2}%)\n\n",
                    sym = sym,
                    dir = pos.direction.to_uppercase(),
                    size = pos.size,
                    notional = format_usd(pos.notional),
                    entry = format_usd(pos.entry_price.unwrap_or(0.0)),
                    mark = format_usd(pos.mark_price.unwrap_or(0.0)),
                    pnl_emoji = pnl_emoji,
                    pnl_sign = pnl_sign,
                    pnl = format_usd(pos.unrealized_pnl.abs()),
                    roe_sign = roe_sign,
                    roe = pos.roe_pct,
                ));

                close_buttons.push(vec![
                    InlineKeyboardButton::callback(
                        format!("❌ Close {sym}"),
                        format!("menu:close:{}", sym.to_lowercase()),
                    )
                ]);
            }

            close_buttons.push(vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")]);
            let kb = InlineKeyboardMarkup::new(close_buttons);
            edit_or_send(bot, chat_id, message_id, &text, kb).await?;
        }
        Err(e) => {
            let err_text = format!("❌ *Failed to fetch positions*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &err_text, back_to_main_keyboard()).await?;
        }
    }
    Ok(())
}

async fn show_orders_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let (account_id, trading_obj_id, indexer_id) = match user_opt {
        Some(ref u) => (
            u.account_id.clone(),
            u.trading_account_object_id.clone(),
            u.indexer_account_id.clone(),
        ),
        None if state.config.admin_user_id == Some(user_id) => {
            let eff_acc = state.trading.get_effective_account_id().await;
            (
                eff_acc.clone(),
                Some(state.config.account_id.clone()),
                Some(eff_acc),
            )
        }
        None => return show_not_connected_notice(bot, chat_id, message_id).await,
    };

    send_typing(bot, chat_id).await;

    let orders_future = state
        .trading
        .get_user_open_orders(
            &account_id,
            trading_obj_id.as_deref(),
            indexer_id.as_deref(),
        );

    let res = tokio::time::timeout(Duration::from_secs(8), orders_future).await;

    match res {
        Ok(Ok(orders)) if orders.is_empty() => {
            let text = "📋 *Open Limit Orders*\n\nNo open limit orders.";
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback("📊 Place an Order", "menu:markets")],
                vec![InlineKeyboardButton::callback("🔄 Refresh", "action:orders")],
                vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
            ]);
            let _ = edit_or_send(bot, chat_id, message_id, text, kb).await;
        }
        Ok(Ok(orders)) => {
            let mut text = format!("📋 *Your Open Limit Orders ({})*\n\n", orders.len());
            let mut buttons = Vec::new();

            for ord in &orders {
                let sym = &ord.market_symbol;
                let is_buy = ord.side.eq_ignore_ascii_case("buy") || ord.side.eq_ignore_ascii_case("long");
                let side_emoji = if is_buy { "🟢 BUY" } else { "🔴 SELL" };

                let time_str = if ord.timestamp > 0 {
                    chrono::DateTime::from_timestamp_millis(ord.timestamp)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "Recent".into())
                } else {
                    "Recent".into()
                };

                let remaining_str = if ord.remaining_quantity < ord.quantity && ord.remaining_quantity > 0.0 {
                    format!("`{}` / `{}` {}", ord.remaining_quantity, ord.quantity, sym)
                } else {
                    format!("`{}` {}", ord.quantity, sym)
                };

                text.push_str(&format!(
                    "• *{sym}* | {side_emoji}\n\
                       Size: {size}\n\
                       Limit Price: *${price}*\n\
                       Order ID: `{id}` | ({time})\n\n",
                    sym = sym,
                    side_emoji = side_emoji,
                    size = remaining_str,
                    price = format_usd(ord.price),
                    id = ord.order_id_str,
                    time = time_str,
                ));

                // Compact callback_data (under 64 bytes limit in Telegram)
                buttons.push(vec![
                    InlineKeyboardButton::callback(
                        format!("❌ Cancel {side_emoji} {sym} @ ${}", format_usd(ord.price)),
                        format!("cnl:{}:{}", sym.to_lowercase(), ord.order_id_str),
                    )
                ]);
            }

            buttons.push(vec![
                InlineKeyboardButton::callback("🔄 Refresh", "action:orders"),
                InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main"),
            ]);
            let kb = InlineKeyboardMarkup::new(buttons);
            let _ = edit_or_send(bot, chat_id, message_id, &text, kb).await;
        }
        Ok(Err(e)) => {
            let err_text = format!(
                "❌ *Failed to fetch open orders*\n\n{}\n\nPlease try refreshing in a few moments.",
                e
            );
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback("🔄 Retry", "action:orders")],
                vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
            ]);
            let _ = edit_or_send(bot, chat_id, message_id, &err_text, kb).await;
        }
        Err(_) => {
            let timeout_text = "⚠️ *Request Timed Out*\n\nThe Real.xyz indexer took too long to respond. Please try refreshing in a few moments.";
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback("🔄 Retry", "action:orders")],
                vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
            ]);
            let _ = edit_or_send(bot, chat_id, message_id, timeout_text, kb).await;
        }
    }
    Ok(())
}

async fn handle_cancel_order_button(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
    rest: &str,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let is_admin = state.config.admin_user_id == Some(user_id);
    if user_opt.is_none() && !is_admin {
        return show_not_connected_notice(bot, chat_id, message_id).await;
    }

    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        bot.send_message(chat_id, "❌ Invalid cancel request.").await?;
        return Ok(());
    }

    let market_key = parts[0];
    let market_id = state.trading.resolve_market(market_key).unwrap_or_else(|| market_key.to_string());
    let order_id: u128 = match parts[1].parse::<u128>() {
        Ok(id) => id,
        Err(_) => {
            let clean = parts[1].trim_start_matches("0x").trim_start_matches("0X");
            match u128::from_str_radix(clean, 16) {
                Ok(id) => id,
                Err(_) => {
                    bot.send_message(chat_id, "❌ Invalid order ID.").await?;
                    return Ok(());
                }
            }
        }
    };

    send_typing(bot, chat_id).await;

    let market_sym = BotState::market_name(&market_id);
    let res = if let Some(user) = user_opt {
        let decrypted = match crate::crypto::decrypt_user_key(&state.config.encryption_key, user_id, &user.encrypted_privkey) {
            Ok(k) => k,
            Err(e) => {
                bot.send_message(chat_id, format!("❌ Decryption error: {e}")).await?;
                return Ok(());
            }
        };

        let mut r = state.trading.cancel_user_order_by_id(&user.account_id, &decrypted, &market_id, order_id).await;
        if r.as_ref().map_or(false, |c| !c.success) {
            if let Some(ref toid) = user.trading_account_object_id {
                if !toid.eq_ignore_ascii_case(&user.account_id) {
                    if let Ok(retry_cancel) = state.trading.cancel_user_order_by_id(toid, &decrypted, &market_id, order_id).await {
                        if retry_cancel.success {
                            r = Ok(retry_cancel);
                        }
                    }
                }
            }
        }
        drop(decrypted);
        r
    } else {
        // Admin fallback via default client
        state.trading.cancel_order_by_id(&market_id, order_id).await
    };

    match res {
        Ok(cancel) if cancel.success => {
            let text = format!(
                "✅ *Order Cancelled Successfully!*\n\n\
                 • Market: *{market_sym}*\n\
                 • Order ID: `{order_id}`\n\
                 • Status: Cancelled on Real.xyz engine"
            );
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback("📋 View Open Orders", "action:orders")],
                vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
            ]);
            let _ = edit_or_send(bot, chat_id, message_id, &text, kb).await;
        }
        Ok(_) => {
            let text = format!(
                "⚠️ *Order Cancellation Notice*\n\n\
                 Order `{order_id}` in *{market_sym}* was not cancelled by the engine. \
                 It may have already been filled or cancelled."
            );
            let kb = InlineKeyboardMarkup::new(vec![
                vec![InlineKeyboardButton::callback("📋 View Open Orders", "action:orders")],
                vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
            ]);
            let _ = edit_or_send(bot, chat_id, message_id, &text, kb).await;
        }
        Err(e) => {
            let text = format!("❌ *Failed to cancel order*\n\n{e}");
            let _ = edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await;
        }
    }

    Ok(())
}

async fn show_history_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let user = match user_opt {
        Some(u) => u,
        None => return show_not_connected_notice(bot, chat_id, message_id).await,
    };

    send_typing(bot, chat_id).await;

    match state
        .trading
        .get_user_trade_history(&user.account_id, user.indexer_account_id.as_deref(), 8)
        .await
    {
        Ok(fills) if fills.is_empty() => {
            let text = "📜 *Trade History*\n\nNo recent trade fills found for this account.";
            edit_or_send(bot, chat_id, message_id, text, back_to_main_keyboard()).await?;
        }
        Ok(fills) => {
            let mut text = String::from("📜 *Recent Trade History (Last 8 Fills)*\n\n");
            for f in &fills {
                let sym = &f.market_symbol;
                let side_emoji = if f.side.eq_ignore_ascii_case("buy") { "🟢 BUY" } else { "🔴 SELL" };
                let time_str = if f.timestamp > 0 {
                    chrono::DateTime::from_timestamp_millis(f.timestamp)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "Recent".into())
                } else {
                    "Recent".into()
                };

                text.push_str(&format!(
                    "• *{sym}* — {side}\n\
                       Size: `{size}` | Price: `${price}`\n\
                       Value: `${notional}` | _{time}_\n\n",
                    sym = sym,
                    side = side_emoji,
                    size = f.size,
                    price = format_usd(f.price),
                    notional = format_usd(f.notional),
                    time = time_str,
                ));
            }
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
        }
        Err(e) => {
            let err_text = format!("❌ *Failed to fetch trade history*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &err_text, back_to_main_keyboard()).await?;
        }
    }
    Ok(())
}

async fn show_alerts_menu(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let text = "🔔 *Price Alerts*\n\nSet notifications for when an asset hits your target price:";
    edit_or_send(bot, chat_id, message_id, text, alerts_menu_keyboard()).await
}

async fn show_alert_market_select(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    state: &BotState,
) -> ResponseResult<()> {
    let markets = state.trading.get_markets().await;
    let text = "🔔 *New Price Alert*\n\nSelect an asset to set an alert for:";
    edit_or_send(bot, chat_id, message_id, text, alert_markets_keyboard(&markets)).await
}

async fn show_alerts_list(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    let alerts = state.alerts.get_user_alerts(user_id).await;
    if alerts.is_empty() {
        let text = "🔔 *Your Price Alerts*\n\nYou have no active price alerts.";
        let kb = InlineKeyboardMarkup::new(vec![
            vec![InlineKeyboardButton::callback("➕ Add New Alert", "alerts:new")],
            vec![InlineKeyboardButton::callback("⬅️ Back to Menu", "menu:main")],
        ]);
        edit_or_send(bot, chat_id, message_id, text, kb).await?;
        return Ok(());
    }

    let mut text = String::from("🔔 *Your Active Price Alerts:*\n\n");
    let mut buttons = Vec::new();

    for a in &alerts {
        let cond = if a.trigger_above { "≥" } else { "≤" };
        let status = if a.triggered { "⚡ Triggered" } else { "⏳ Active" };
        text.push_str(&format!(
            "• *{}* {} *${}* ({})\n",
            a.symbol,
            cond,
            format_usd(a.target_price),
            status,
        ));
        buttons.push(vec![InlineKeyboardButton::callback(
            format!("🗑️ Delete {} ${}", a.symbol, a.target_price as i64),
            format!("alert:del:{}", a.id),
        )]);
    }

    buttons.push(vec![
        InlineKeyboardButton::callback("➕ Add Alert", "alerts:new"),
        InlineKeyboardButton::callback("⬅️ Back", "menu:alerts"),
    ]);

    edit_or_send(bot, chat_id, message_id, &text, InlineKeyboardMarkup::new(buttons)).await
}

async fn start_alert_setup(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    state: &BotState,
    symbol: &str,
) -> ResponseResult<()> {
    let sym_upper = symbol.to_uppercase();
    let market_id = state.resolve_market(symbol).unwrap_or_else(|| sym_upper.clone());
    let mark_price = state.trading.get_market_mark_price(&market_id).await.unwrap_or(0.0);

    let pending = PendingTrade {
        direction: TradeDirection::Long,
        market_id: market_id.clone(),
        market_symbol: sym_upper.clone(),
        step: TradeStep::AwaitingAlertTarget {
            market_id: market_id.clone(),
            symbol: sym_upper.clone(),
        },
    };
    state.set_session(chat_id, pending).await;

    let price_info = if mark_price > 0.0 {
        format!("Current price: *${}*\n\n", format_usd(mark_price))
    } else {
        String::new()
    };

    let text = format!(
        "🔔 *Set Alert for {sym_upper}*\n\n\
         {price_info}\
         Please enter the target price to trigger an alert\n*(e.g. 85000, 3150, 185)*:"
    );

    let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
    ]]);

    edit_or_send(bot, chat_id, message_id, &text, cancel_kb).await
}

async fn handle_delete_alert(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
    alert_id: &str,
) -> ResponseResult<()> {
    let _ = state.alerts.remove_alert(alert_id, user_id).await;
    show_alerts_list(bot, chat_id, message_id, user_id, state).await
}

// ─── Connect / Disconnect Handlers ──────────────────────────────────────────

async fn handle_connect_private_key(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    username: Option<String>,
    state: &BotState,
    raw_key: &str,
) -> ResponseResult<()> {
    let key = raw_key.trim();
    if !key.starts_with("iotaprivkey1") {
        bot.send_message(
            chat_id,
            "❌ *Invalid Key Format*\n\nPrivate key must start with `iotaprivkey1`.",
        )
        .reply_markup(main_menu_keyboard(false))
        .parse_mode(ParseMode::Markdown)
        .await?;
        return Ok(());
    }

    send_typing(bot, chat_id).await;

    // Validate key & derive IOTA wallet address
    let address = match crate::trading::derive_address_from_key(key) {
        Ok(addr) if addr.starts_with("0x") => addr,
        Ok(addr) => addr,
        Err(_) => {
            bot.send_message(
                chat_id,
                "❌ *Invalid Key*\n\nUnable to derive address from provided key.",
            )
            .reply_markup(main_menu_keyboard(false))
            .parse_mode(ParseMode::Markdown)
            .await?;
            return Ok(());
        }
    };

    // Lookup on-chain account in Real.xyz indexer
    let (account_id, trading_account_obj_id) = match state.trading.lookup_account_by_address(&address).await {
        Some(lookup) => (lookup.account_id, lookup.trading_account_object_id),
        None => {
            // If not found in indexer accounts yet, default to address
            (address.clone(), None)
        }
    };

    // Encrypt the key with AES-256-GCM using master key + user_id
    let encrypted = match crate::crypto::encrypt_user_key(&state.config.encryption_key, user_id, key) {
        Ok(enc) => enc,
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ *Encryption Error*\n\nFailed to securely encrypt key: {e}"),
            )
            .reply_markup(main_menu_keyboard(false))
            .parse_mode(ParseMode::Markdown)
            .await?;
            return Ok(());
        }
    };

    let user_record = crate::users::StoredUser {
        telegram_id: user_id,
        username,
        address: address.clone(),
        account_id: account_id.clone(),
        trading_account_object_id: trading_account_obj_id.clone(),
        indexer_account_id: Some(account_id.clone()),
        encrypted_privkey: encrypted,
        connected_at: chrono::Utc::now().timestamp(),
    };

    if let Err(e) = state.users.save_user(user_record).await {
        bot.send_message(
            chat_id,
            format!("❌ *Storage Error*\n\nFailed to save user account: {e}"),
        )
        .reply_markup(main_menu_keyboard(false))
        .parse_mode(ParseMode::Markdown)
        .await?;
        return Ok(());
    }

    let short_addr = if address.len() > 14 {
        format!("{}…{}", &address[..8], &address[address.len() - 6..])
    } else {
        address.clone()
    };

    let obj_line = match &trading_account_obj_id {
        Some(oid) => format!("\n • Trading Account Object: `{oid}`"),
        None => String::new(),
    };

    let text = format!(
        "✅ *Wallet Successfully Connected!*\n\n\
         • Address: `{short_addr}`\n\
         • Account ID: `{account_id}`{obj_line}\n\
         • Security: *AES-256-GCM Encrypted*\n\n\
         🛡️ *Your private key is safe:*\n\
         • It has been encrypted with a unique key bound to your account.\n\
         • The original message was automatically wiped from Telegram.\n\
         • Plaintext keys are never logged and never stored on disk.\n\
         • Orders are signed on-demand in-memory and the key is immediately cleared.\n\n\
         Select an option below to begin trading:",
    );

    bot.send_message(chat_id, text)
        .reply_markup(main_menu_keyboard(true))
        .parse_mode(ParseMode::Markdown)
        .await?;

    Ok(())
}

async fn cmd_connect(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    username: Option<String>,
    state: &BotState,
    args: &str,
) -> ResponseResult<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let pending = PendingTrade {
            direction: TradeDirection::Long,
            market_id: String::new(),
            market_symbol: String::new(),
            step: TradeStep::AwaitingPrivateKey,
        };
        state.set_session(chat_id, pending).await;
        bot.send_message(
            chat_id,
            "🔐 *Connect to Pocket Real*\n\n\
             Please reply with your IOTA private key (`iotaprivkey1...`).\n\n\
             🛡️ *Security Guarantees:*\n\
             • Your message will be **instantly deleted** upon receipt.\n\
             • Your key is encrypted with **AES-256-GCM** before storage.\n\
             • The plain key is never logged and never stored on disk.\n\
             • Orders are signed strictly in-memory.\n\
             • No admin can view or export your private key.\n\n\
             Send `/cancel` or tap below to abort:",
        )
        .reply_markup(connect_prompt_keyboard())
        .parse_mode(ParseMode::Markdown)
        .await?;
    } else {
        handle_connect_private_key(bot, chat_id, user_id, username, state, trimmed).await?;
    }
    Ok(())
}

async fn cmd_disconnect(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    state.clear_session(chat_id).await;
    let removed = state.users.remove_user(user_id).await.unwrap_or(false);
    let msg = if removed {
        "🗑️ *Wallet Disconnected*\n\nYour encrypted keys and account records have been permanently erased from the bot."
    } else {
        "ℹ️ No connected wallet found."
    };

    bot.send_message(chat_id, msg)
        .reply_markup(main_menu_keyboard(false))
        .parse_mode(ParseMode::Markdown)
        .await?;

    Ok(())
}

// ─── Button Actions ───────────────────────────────────────────────────────────

async fn start_trade_from_button(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
    direction: TradeDirection,
    symbol: &str,
) -> ResponseResult<()> {
    if state.users.get_user(user_id).await.is_none() {
        return show_not_connected_notice(bot, chat_id, message_id).await;
    }

    let market_id = match state.resolve_market(symbol) {
        Some(m) => m,
        None => {
            bot.send_message(chat_id, "❌ Unknown market.").await?;
            return Ok(());
        }
    };
    let _ = state.trading.get_market_max_leverage_async(&market_id).await;
    let sym_upper = BotState::market_name(&market_id);

    let pending = PendingTrade {
        direction,
        market_id,
        market_symbol: sym_upper.clone(),
        step: TradeStep::AwaitingUsdAmount,
    };
    state.set_session(chat_id, pending).await;

    let (dir_name, emoji) = match direction {
        TradeDirection::Long => ("Long", "📈"),
        TradeDirection::Short => ("Short", "📉"),
    };

    let user_opt = state.users.get_user(user_id).await;
    let bal_info = match user_opt {
        Some(ref u) => state.trading.get_user_balance(&u.account_id, u.indexer_account_id.as_deref()).await.ok(),
        None => None,
    };
    let bal_line = match bal_info {
        Some(ref b) => format!("\n• Available Margin: *${:.2}*", b.available_margin),
        None => String::new(),
    };

    let text = format!(
        "{emoji} *{dir_name} {sym_upper}*\
         {bal_line}\n\n\
         How much do you want to use?\n\
         • Type an amount in USD (e.g. `50` = $50)\n\
         • Or type a percentage of balance (e.g. `50%`)\n\
         • Or tap a quick percentage below:"
    );

    let kb = order_size_keyboard(Some(&format!("market:{}", symbol.to_lowercase())));
    edit_or_send(bot, chat_id, message_id, &text, kb).await?;

    Ok(())
}

async fn handle_order_size_percentage(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    pct: f64,
) -> ResponseResult<()> {
    let session = state.get_session(chat_id).await;
    let mut pending = match session {
        Some(p) if matches!(p.step, TradeStep::AwaitingUsdAmount) => p,
        _ => return Ok(()),
    };

    let user = match state.users.get_user(user_id).await {
        Some(u) => u,
        None => {
            show_not_connected_notice(bot, chat_id, None).await?;
            return Ok(());
        }
    };

    let bal = match state.trading.get_user_balance(&user.account_id, user.indexer_account_id.as_deref()).await {
        Ok(b) => b,
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Failed to fetch available balance: {e}")).await?;
            return Ok(());
        }
    };

    if bal.available_margin <= 0.0 {
        bot.send_message(
            chat_id,
            "❌ Available margin is $0.00. Please deposit funds or enter a custom USD amount (e.g. 50):",
        )
        .reply_markup(order_size_keyboard(None))
        .await?;
        return Ok(());
    }

    let (usd, is_max_pct) = if pct >= 99.0 {
        // Reserve 10% (minimum $0.50) buffer for exchange taker fees, book spread, and 1% trading bound
        let safe = if bal.available_margin > 1.0 {
            (bal.available_margin - 0.50).min(bal.available_margin * 0.90)
        } else {
            bal.available_margin * 0.80
        };
        (((safe * 100.0).floor()) / 100.0, true)
    } else {
        (((bal.available_margin * (pct / 100.0) * 100.0).floor()) / 100.0, false)
    };
    if usd <= 0.0 {
        bot.send_message(
            chat_id,
            format!("❌ Calculated size from {pct:.0}% is too small (available: ${:.2}). Please enter a USD amount:", bal.available_margin),
        )
        .reply_markup(order_size_keyboard(None))
        .await?;
        return Ok(());
    }

    let max_lev = state.trading.get_market_max_leverage_async(&pending.market_id).await;
    pending.step = TradeStep::AwaitingLeverage { usd_amount: usd };
    state.set_session(chat_id, pending).await;

    let prompt_text = if is_max_pct {
        format!(
            "Selected: *${usd:.2}* (100% max with fee reserve from ${:.2})\n\n\
             What leverage? (Up to *{max_lev:.0}x* — choose or type):",
            bal.available_margin
        )
    } else {
        format!(
            "Selected: *${usd:.2}* ({pct:.0}% of available ${:.2})\n\n\
             What leverage? (Up to *{max_lev:.0}x* — choose or type):",
            bal.available_margin
        )
    };
    bot.send_message(chat_id, prompt_text)
        .reply_markup(leverage_keyboard(Some(max_lev)))
        .parse_mode(ParseMode::Markdown)
        .await?;
    Ok(())
}

async fn handle_leverage_selection(
    bot: &Bot,
    chat_id: ChatId,
    _user_id: u64,
    state: &BotState,
    leverage: f64,
) -> ResponseResult<()> {
    let session = state.get_session(chat_id).await;
    match session {
        Some(mut pending) => {
            if let TradeStep::AwaitingLeverage { usd_amount } = pending.step {
                let max_lev = state.trading.get_market_max_leverage(&pending.market_id);
                let clamped_leverage = leverage.min(max_lev).max(1.0);
                pending.step = TradeStep::AwaitingOrderType { usd_amount, leverage: clamped_leverage };
                state.set_session(chat_id, pending).await;
                bot.send_message(
                    chat_id,
                    format!("Leverage set to *{clamped_leverage:.0}x*.\n\nChoose order type:"),
                )
                .reply_markup(order_type_keyboard())
                .parse_mode(ParseMode::Markdown)
                .await?;
            } else {
                bot.send_message(chat_id, "ℹ️ No active trade setup awaiting leverage.")
                    .reply_markup(back_to_main_keyboard())
                    .await?;
            }
        }
        None => {
            bot.send_message(chat_id, "ℹ️ No active trade setup awaiting leverage.")
                .reply_markup(back_to_main_keyboard())
                .await?;
        }
    }
    Ok(())
}

async fn handle_order_type_choice(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    choice_str: &str,
) -> ResponseResult<()> {
    let session = state.get_session(chat_id).await;
    match session {
        Some(mut pending) => {
            if let TradeStep::AwaitingOrderType { usd_amount, leverage } = pending.step {
                if choice_str == "market" {
                    state.clear_session(chat_id).await;
                    execute_conversational_trade(
                        bot,
                        chat_id,
                        user_id,
                        state,
                        pending.direction,
                        &pending.market_id,
                        &pending.market_symbol,
                        usd_amount,
                        leverage,
                        crate::bot::state::OrderTypeChoice::Market,
                        None,
                    )
                    .await?;
                } else if choice_str == "limit" {
                    let market_symbol = pending.market_symbol.clone();
                    pending.step = TradeStep::AwaitingLimitPrice { usd_amount, leverage };
                    state.set_session(chat_id, pending).await;
                    let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
                        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
                    ]]);
                    bot.send_message(
                        chat_id,
                        format!("🎯 *Limit Order Price*\n\nWhat price would you like to place your limit order at for *{market_symbol}*?\n*(e.g. 84500, 3200)*"),
                    )
                    .reply_markup(cancel_kb)
                    .parse_mode(ParseMode::Markdown)
                    .await?;
                }
            } else {
                bot.send_message(chat_id, "ℹ️ No active trade setup awaiting order type.")
                    .reply_markup(back_to_main_keyboard())
                    .await?;
            }
        }
        None => {
            bot.send_message(chat_id, "ℹ️ No active trade setup awaiting order type.")
                .reply_markup(back_to_main_keyboard())
                .await?;
        }
    }
    Ok(())
}

async fn execute_button_close(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    user_id: u64,
    state: &BotState,
    symbol: &str,
    pct: f64,
) -> ResponseResult<()> {
    send_typing(bot, chat_id).await;

    let user_opt = state.users.get_user(user_id).await;
    let user = match user_opt {
        Some(u) => u,
        None => return show_not_connected_notice(bot, chat_id, message_id).await,
    };

    let market_id = match state.resolve_market(symbol) {
        Some(m) => m,
        None => {
            bot.send_message(chat_id, "❌ Unknown market.").await?;
            return Ok(());
        }
    };
    let sym_upper = BotState::market_name(&market_id);

    let pos_opt = match state
        .trading
        .get_user_open_position(&user.account_id, user.indexer_account_id.as_deref(), &market_id)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            let err_text = format!("❌ *Failed to check positions*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &err_text, back_to_main_keyboard()).await?;
            return Ok(());
        }
    };

    let pos = match pos_opt {
        Some(p) if p.size > 0.0 => p,
        _ => {
            let text = format!("ℹ️ No open position found for *{sym_upper}*.");
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
            return Ok(());
        }
    };

    let decimals = crate::trading::get_cached_market_decimals(&sym_upper);
    let min_size = crate::trading::get_cached_market_lot_size(&sym_upper);
    let close_fraction = (pct / 100.0).clamp(0.01, 1.0);
    let raw_close_qty = if pct >= 99.9 { pos.size } else { pos.size * close_fraction };
    let (mut qty_str, rounded_close_qty) = crate::trading::round_qty_to_decimals(raw_close_qty, decimals);
    if rounded_close_qty <= 0.0 && pos.size > 0.0 {
        let (fallback_str, _) = crate::trading::round_qty_to_decimals(pos.size.min(min_size), decimals);
        qty_str = fallback_str;
    }

    let is_pos_long = pos.direction.eq_ignore_ascii_case("long") || pos.direction.eq_ignore_ascii_case("buy");
    let close_side = if is_pos_long { "Sell" } else { "Buy" };

    // Decrypt key into Zeroizing buffer
    let decrypted = match crate::crypto::decrypt_user_key(&state.config.encryption_key, user_id, &user.encrypted_privkey) {
        Ok(k) => k,
        Err(e) => {
            let err_text = format!("❌ *Decryption Error*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &err_text, back_to_main_keyboard()).await?;
            return Ok(());
        }
    };

    let res = state
        .trading
        .place_user_close_order(&user.account_id, &decrypted, &market_id, close_side, &qty_str, None)
        .await;

    drop(decrypted);

    match res {
        Ok(order) if order.success => {
            let order_id = order.order_id.unwrap_or_else(|| "N/A".into());
            let text = format!(
                "✅ *Close Order Submitted*\n\n\
                 • Market: *{sym_upper}*\n\
                 • Position: *{pos_dir}* ({pos_size} {sym_upper})\n\
                 • Closing: *{pct:.0}%* (`{qty_str}` {sym_upper})\n\
                 • Price: *Market Price*\n\
                 • Order ID: `{order_id}`",
                pos_dir = pos.direction,
                pos_size = pos.size,
                pct = pct,
                order_id = order_id,
            );
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
        }
        Ok(order) => {
            let err = order.error_msg.unwrap_or_else(|| "Engine rejected order".into());
            let text = format!("❌ *Close Order Rejected*\n\nReason: {err}");
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
        }
        Err(e) => {
            let text = format!("❌ *Failed to submit close order*\n\n{e}");
            edit_or_send(bot, chat_id, message_id, &text, back_to_main_keyboard()).await?;
        }
    }

    Ok(())
}

// ─── Conversational Step Handler ──────────────────────────────────────────────

async fn handle_trade_step(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    mut pending: PendingTrade,
    text: &str,
) -> ResponseResult<()> {
    match pending.step {
        TradeStep::AwaitingPrivateKey => {
            // Already handled in handle_message
        }
        TradeStep::AwaitingMarket => {
            let market = match state.resolve_market(text) {
                Some(m) => m,
                None => {
                    bot.send_message(
                        chat_id,
                        format!("❌ Please choose a valid market: {} (or tap Cancel):", state.available_markets_summary()),
                    )
                    .reply_markup(markets_keyboard(&state.trading.get_cached_markets()))
                    .parse_mode(ParseMode::Markdown)
                    .await?;
                    return Ok(());
                }
            };
            let symbol = BotState::market_name(&market);
            pending.market_id = market;
            pending.market_symbol = symbol.clone();
            pending.step = TradeStep::AwaitingUsdAmount;
            state.set_session(chat_id, pending.clone()).await;

            let (dir_name, emoji) = match pending.direction {
                TradeDirection::Long => ("Long", "📈"),
                TradeDirection::Short => ("Short", "📉"),
            };

            let user_opt = state.users.get_user(user_id).await;
            let bal_info = match user_opt {
                Some(ref u) => state.trading.get_user_balance(&u.account_id, u.indexer_account_id.as_deref()).await.ok(),
                None => None,
            };
            let bal_line = match bal_info {
                Some(ref b) => format!("\n• Available Margin: *${:.2}*", b.available_margin),
                None => String::new(),
            };

            let text = format!(
                "{emoji} *{dir_name} {symbol}*\
                 {bal_line}\n\n\
                 How much do you want to use?\n\
                 • Type an amount in USD (e.g. `50` = $50)\n\
                 • Or type a percentage of balance (e.g. `50%`)\n\
                 • Or tap a quick percentage below:"
            );

            bot.send_message(chat_id, text)
                .reply_markup(order_size_keyboard(None))
                .parse_mode(ParseMode::Markdown)
                .await?;
        }
        TradeStep::AwaitingUsdAmount => {
            let trimmed = text.trim();
            let is_percentage = trimmed.contains('%');
            let (usd, is_max_pct) = if is_percentage {
                let cleaned = trimmed.replace('%', "");
                let pct = match cleaned.trim().parse::<f64>() {
                    Ok(v) if v > 0.0 && v <= 100.0 => v,
                    _ => {
                        bot.send_message(
                            chat_id,
                            "❌ Please enter a valid percentage between 1% and 100% (e.g. 25%, 50%, 100%) or tap Cancel:",
                        )
                        .reply_markup(order_size_keyboard(None))
                        .await?;
                        return Ok(());
                    }
                };

                let user = match state.users.get_user(user_id).await {
                    Some(u) => u,
                    None => {
                        show_not_connected_notice(bot, chat_id, None).await?;
                        return Ok(());
                    }
                };

                let bal = match state.trading.get_user_balance(&user.account_id, user.indexer_account_id.as_deref()).await {
                    Ok(b) => b,
                    Err(e) => {
                        bot.send_message(chat_id, format!("❌ Failed to fetch available balance: {e}")).await?;
                        return Ok(());
                    }
                };

                if bal.available_margin <= 0.0 {
                    bot.send_message(
                        chat_id,
                        "❌ Available margin is $0.00. Please deposit funds or enter a custom USD amount (e.g. 50):",
                    )
                    .reply_markup(order_size_keyboard(None))
                    .await?;
                    return Ok(());
                }

                let (calc_usd, is_max) = if pct >= 99.0 {
                    let safe = if bal.available_margin > 1.0 {
                        (bal.available_margin - 0.50).min(bal.available_margin * 0.90)
                    } else {
                        bal.available_margin * 0.80
                    };
                    ((((safe * 100.0).floor()) / 100.0).max(0.0), true)
                } else {
                    (((bal.available_margin * (pct / 100.0) * 100.0).floor()) / 100.0, false)
                };

                if calc_usd <= 0.0 {
                    bot.send_message(
                        chat_id,
                        format!("❌ Calculated size from {pct:.0}% is too small (available: ${:.2}). Please enter a USD amount:", bal.available_margin),
                    )
                    .reply_markup(order_size_keyboard(None))
                    .await?;
                    return Ok(());
                }
                (calc_usd, is_max)
            } else {
                let cleaned = trimmed.replace('$', "").replace(',', "");
                let val = match cleaned.parse::<f64>() {
                    Ok(v) if v > 0.0 => v,
                    _ => {
                        bot.send_message(
                            chat_id,
                            "❌ Please enter a valid USD amount (e.g. 50, 100) or percentage of balance (e.g. 50%):",
                        )
                        .reply_markup(order_size_keyboard(None))
                        .await?;
                        return Ok(());
                    }
                };

                let user_opt = state.users.get_user(user_id).await;
                let bal_opt = if let Some(ref u) = user_opt {
                    state.trading.get_user_balance(&u.account_id, u.indexer_account_id.as_deref()).await.ok()
                } else {
                    None
                };

                if let Some(bal) = bal_opt {
                    let safe = if bal.available_margin > 1.0 {
                        (bal.available_margin - 0.50).min(bal.available_margin * 0.90)
                    } else {
                        bal.available_margin * 0.80
                    };
                    let max_safe = (((safe * 100.0).floor()) / 100.0).max(0.0);
                    if val > bal.available_margin {
                        bot.send_message(
                            chat_id,
                            format!(
                                "❌ Amount *${val:.2}* exceeds your available margin (*${:.2}*). Please enter an amount up to *${max_safe:.2}*:",
                                bal.available_margin
                            ),
                        )
                        .reply_markup(order_size_keyboard(None))
                        .parse_mode(ParseMode::Markdown)
                        .await?;
                        return Ok(());
                    } else if val > max_safe {
                        (max_safe, true)
                    } else {
                        (val, false)
                    }
                } else {
                    (val, false)
                }
            };

            let max_lev = state.trading.get_market_max_leverage_async(&pending.market_id).await;
            pending.step = TradeStep::AwaitingLeverage { usd_amount: usd };
            state.set_session(chat_id, pending).await;

            let prompt_text = if is_max_pct {
                format!("Selected: *${usd:.2}* (Max available with fee reserve)\n\nWhat leverage? (Up to *{max_lev:.0}x* — choose or type):")
            } else {
                format!("Selected: *${usd:.2}*\n\nWhat leverage? (Up to *{max_lev:.0}x* — choose or type):")
            };
            bot.send_message(chat_id, prompt_text)
                .reply_markup(leverage_keyboard(Some(max_lev)))
                .parse_mode(ParseMode::Markdown)
                .await?;
        }
        TradeStep::AwaitingLeverage { usd_amount } => {
            let max_lev = state.trading.get_market_max_leverage(&pending.market_id);
            let cleaned = text.trim().trim_end_matches(['x', 'X']).trim();
            let lev = match cleaned.parse::<f64>() {
                Ok(v) if v >= 1.0 && v <= max_lev => v,
                _ => {
                    bot.send_message(
                        chat_id,
                        format!("❌ Please enter a valid leverage between 1x and {max_lev:.0}x (e.g. 5x, 10x, {max_lev:.0}x) or tap Cancel:"),
                    )
                    .reply_markup(leverage_keyboard(Some(max_lev)))
                    .await?;
                    return Ok(());
                }
            };

            pending.step = TradeStep::AwaitingOrderType { usd_amount, leverage: lev };
            state.set_session(chat_id, pending).await;
            bot.send_message(
                chat_id,
                format!("Leverage set to *{lev:.0}x*.\n\nChoose order type:"),
            )
            .reply_markup(order_type_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
        }
        TradeStep::AwaitingOrderType { .. } => {
            bot.send_message(
                chat_id,
                "Please choose an order type using the buttons below:",
            )
            .reply_markup(order_type_keyboard())
            .await?;
        }
        TradeStep::AwaitingLimitPrice { usd_amount, leverage } => {
            let cleaned = text.trim().replace('$', "").replace(',', "");
            let price = match cleaned.parse::<f64>() {
                Ok(v) if v > 0.0 => v,
                _ => {
                    let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
                        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
                    ]]);
                    bot.send_message(
                        chat_id,
                        "❌ Please enter a valid limit price (e.g. 84500) or tap Cancel:",
                    )
                    .reply_markup(cancel_kb)
                    .await?;
                    return Ok(());
                }
            };

            if let Ok(mark) = state.trading.get_market_mark_price(&pending.market_id).await {
                if mark > 0.0 {
                    match pending.direction {
                        TradeDirection::Long if price >= mark => {
                            let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
                                InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
                            ]]);
                            bot.send_message(
                                chat_id,
                                format!(
                                    "⚠️ *Limit Buy Price Warning*\n\n\
                                     • Market: *{}*\n\
                                     • Current Mark Price: *${}*\n\
                                     • Entered Price: *${}*\n\n\
                                     A Limit Buy must be placed *below* current market price so it rests as a pending order on the book. Entering a price at or above market price will fill immediately as a market taker order.\n\n\
                                     Please enter a price *below* ${} (e.g. ${}) or tap Cancel:",
                                    pending.market_symbol,
                                    format_usd(mark),
                                    format_usd(price),
                                    format_usd(mark),
                                    format_usd(mark * 0.98),
                                ),
                            )
                            .reply_markup(cancel_kb)
                            .parse_mode(ParseMode::Markdown)
                            .await?;
                            return Ok(());
                        }
                        TradeDirection::Short if price <= mark => {
                            let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
                                InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
                            ]]);
                            bot.send_message(
                                chat_id,
                                format!(
                                    "⚠️ *Limit Sell Price Warning*\n\n\
                                     • Market: *{}*\n\
                                     • Current Mark Price: *${}*\n\
                                     • Entered Price: *${}*\n\n\
                                     A Limit Sell must be placed *above* current market price so it rests as a pending order on the book. Entering a price at or below market price will fill immediately as a market taker order.\n\n\
                                     Please enter a price *above* ${} (e.g. ${}) or tap Cancel:",
                                    pending.market_symbol,
                                    format_usd(mark),
                                    format_usd(price),
                                    format_usd(mark),
                                    format_usd(mark * 1.02),
                                ),
                            )
                            .reply_markup(cancel_kb)
                            .parse_mode(ParseMode::Markdown)
                            .await?;
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }

            state.clear_session(chat_id).await;
            execute_conversational_trade(
                bot,
                chat_id,
                user_id,
                state,
                pending.direction,
                &pending.market_id,
                &pending.market_symbol,
                usd_amount,
                leverage,
                crate::bot::state::OrderTypeChoice::Limit,
                Some(price),
            )
            .await?;
        }
        TradeStep::AwaitingAlertTarget { market_id, symbol } => {
            let cleaned = text.trim().replace('$', "").replace(',', "");
            let target_price = match cleaned.parse::<f64>() {
                Ok(v) if v > 0.0 => v,
                _ => {
                    let cancel_kb = InlineKeyboardMarkup::new(vec![vec![
                        InlineKeyboardButton::callback("❌ Cancel", "action:cancel"),
                    ]]);
                    bot.send_message(
                        chat_id,
                        "❌ Please enter a valid target price (e.g. 85000) or tap Cancel:",
                    )
                    .reply_markup(cancel_kb)
                    .await?;
                    return Ok(());
                }
            };

            let current_price = state.trading.get_market_mark_price(&market_id).await.unwrap_or(target_price);
            let trigger_above = target_price >= current_price;
            let dir_str = if trigger_above { "rises to or above" } else { "drops to or below" };

            let alert = crate::alerts::PriceAlert {
                id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
                telegram_id: user_id,
                chat_id: chat_id.0,
                market_id,
                symbol: symbol.to_uppercase(),
                target_price,
                trigger_above,
                created_at: chrono::Utc::now().timestamp(),
                triggered: false,
            };

            let _ = state.alerts.add_alert(alert).await;
            state.clear_session(chat_id).await;

            let text = format!(
                "🔔 *Price Alert Created!*\n\n\
                 • Asset: *{}*\n\
                 • Target Price: *${}*\n\
                 • Current Mark: *${}*\n\
                 • Condition: Alert when price {} *${}*\n\n\
                 You will receive a notification the moment this price is reached.",
                symbol.to_uppercase(),
                format_usd(target_price),
                format_usd(current_price),
                dir_str,
                format_usd(target_price),
            );

            bot.send_message(chat_id, text)
                .reply_markup(back_to_main_keyboard())
                .parse_mode(ParseMode::Markdown)
                .await?;
        }
        TradeStep::AwaitingClosePercentage { market_id: _, symbol } => {
            state.clear_session(chat_id).await;
            let cleaned = text.trim().replace('%', "");
            let pct = match cleaned.trim().parse::<f64>() {
                Ok(v) if v > 0.0 && v <= 100.0 => v,
                _ => {
                    bot.send_message(
                        chat_id,
                        "❌ Invalid percentage. Please enter a percentage between 1 and 100 (e.g. 10%, 75%):",
                    )
                    .reply_markup(close_options_keyboard(&symbol))
                    .await?;
                    return Ok(());
                }
            };

            execute_button_close(bot, chat_id, None, user_id, state, &symbol, pct).await?;
        }
    }
    Ok(())
}

async fn execute_conversational_trade(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    direction: TradeDirection,
    market_id: &str,
    symbol: &str,
    usd_amount: f64,
    leverage: f64,
    order_type: crate::bot::state::OrderTypeChoice,
    limit_price: Option<f64>,
) -> ResponseResult<()> {
    send_typing(bot, chat_id).await;

    let user_opt = state.users.get_user(user_id).await;
    let user = match user_opt {
        Some(u) => u,
        None => return show_not_connected_notice(bot, chat_id, None).await,
    };

    if matches!(order_type, crate::bot::state::OrderTypeChoice::Limit) {
        if limit_price.is_none() || limit_price.unwrap_or(0.0) <= 0.0 {
            bot.send_message(
                chat_id,
                "❌ A valid limit price is required for Limit orders. Please try again.",
            )
            .reply_markup(back_to_main_keyboard())
            .await?;
            return Ok(());
        }
    }

    let mark_price = match state.trading.get_market_mark_price(market_id).await {
        Ok(p) if p > 0.0 => p,
        Ok(_) | Err(_) => {
            bot.send_message(
                chat_id,
                format!("❌ Unable to fetch current market price for {symbol}. Please try again later."),
            )
            .reply_markup(back_to_main_keyboard())
            .await?;
            return Ok(());
        }
    };

    if matches!(order_type, crate::bot::state::OrderTypeChoice::Limit) {
        if let Some(p) = limit_price {
            match direction {
                TradeDirection::Long if p >= mark_price => {
                    bot.send_message(
                        chat_id,
                        format!(
                            "❌ *Invalid Limit Buy Price*\n\n\
                             • Market: *{symbol}*\n\
                             • Current Mark Price: *${}*\n\
                             • Your Limit Price: *${}*\n\n\
                             A Limit Buy order must be placed *below* market price to rest on the book as a pending order. A price at or above market would fill immediately as a market taker order.\n\n\
                             Please place your limit buy below ${}.",
                            format_usd(mark_price),
                            format_usd(p),
                            format_usd(mark_price),
                        ),
                    )
                    .reply_markup(back_to_main_keyboard())
                    .parse_mode(ParseMode::Markdown)
                    .await?;
                    return Ok(());
                }
                TradeDirection::Short if p <= mark_price => {
                    bot.send_message(
                        chat_id,
                        format!(
                            "❌ *Invalid Limit Sell Price*\n\n\
                             • Market: *{symbol}*\n\
                             • Current Mark Price: *${}*\n\
                             • Your Limit Price: *${}*\n\n\
                             A Limit Sell order must be placed *above* market price to rest on the book as a pending order. A price at or below market would fill immediately as a market taker order.\n\n\
                             Please place your limit sell above ${}.",
                            format_usd(mark_price),
                            format_usd(p),
                            format_usd(mark_price),
                        ),
                    )
                    .reply_markup(back_to_main_keyboard())
                    .parse_mode(ParseMode::Markdown)
                    .await?;
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    // Sizing price calculation:
    // 1. For Limit orders (both Long and Short):
    //    - Limit Buy is placed below mark price. Evaluating at limit price would make raw_qty
    //      larger than at mark price, exceeding available margin when validated by the engine.
    //      Therefore, we size against max(limit_price, mark_price) * 1.01.
    //    - Limit Sell is placed above mark price. Sizing against max(limit_price, mark_price) * 1.01
    //      (which is limit_price * 1.01) reserves a safe margin buffer for order collateral.
    // 2. For Market orders (both Long and Short):
    //    Crossing the spread into the orderbook incurs taker fees (0.06%), spread slippage,
    //    and Real.xyz's 1% trading bound slippage check.
    //    Buffer mark_price by 1.5% so the resulting base asset quantity never causes
    //    the required margin + slippage bounds to exceed the user's allocated margin.
    let exec_price = if let Some(p) = limit_price {
        p.max(mark_price) * 1.01
    } else {
        mark_price * 1.015
    };
    let total_notional = usd_amount * leverage;
    let raw_qty = total_notional / exec_price;

    let decimals = crate::trading::get_cached_market_decimals(market_id);
    let (qty_str, rounded_qty) = crate::trading::floor_qty_to_decimals(raw_qty, decimals);

    if rounded_qty <= 0.0 {
        bot.send_message(
            chat_id,
            format!("❌ Order size for {symbol} is too small. Please increase your USD amount or leverage."),
        )
        .reply_markup(back_to_main_keyboard())
        .await?;
        return Ok(());
    }

    let (dir_str, emoji) = match direction {
        TradeDirection::Long => ("Long", "📈"),
        TradeDirection::Short => ("Short", "📉"),
    };

    let price_str = match (order_type, limit_price) {
        (crate::bot::state::OrderTypeChoice::Limit, Some(p)) => {
            crate::trading::sanitize_order_price_for_market(market_id, &p.to_string()).0
        }
        _ => {
            let price_decimals = crate::trading::get_cached_market_price_decimals(market_id);
            crate::trading::round_price_to_decimals(mark_price, price_decimals).0
        }
    };

    info!(
        user_id = user_id,
        market = %symbol,
        market_id = %market_id,
        direction = %dir_str,
        order_type = ?order_type,
        limit_price = ?limit_price,
        exec_price = exec_price,
        raw_qty = raw_qty,
        qty_str = %qty_str,
        price_str = %price_str,
        "execute_conversational_trade: placing order"
    );

    // Decrypt key into Zeroizing buffer
    let decrypted = match crate::crypto::decrypt_user_key(&state.config.encryption_key, user_id, &user.encrypted_privkey) {
        Ok(k) => k,
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ *Decryption Error*\n\n{e}"),
            )
            .reply_markup(back_to_main_keyboard())
            .await?;
            return Ok(());
        }
    };

    // CRITICAL ORDER FLOW GUARANTEE:
    // Strictly place ONLY the user-requested directional order.
    // Under no circumstances place an opposing order, auto-hedge, or auto-close.
    // The position remains open until the user manually triggers a close.
    let res = match (direction, order_type) {
        (TradeDirection::Long, crate::bot::state::OrderTypeChoice::Market) => {
            state.trading.place_user_market_long(&user.account_id, &decrypted, market_id, &qty_str).await
        }
        (TradeDirection::Short, crate::bot::state::OrderTypeChoice::Market) => {
            state.trading.place_user_market_short(&user.account_id, &decrypted, market_id, &qty_str).await
        }
        (TradeDirection::Long, crate::bot::state::OrderTypeChoice::Limit) => {
            state.trading.place_user_limit_long(&user.account_id, &decrypted, market_id, &qty_str, &price_str).await
        }
        (TradeDirection::Short, crate::bot::state::OrderTypeChoice::Limit) => {
            state.trading.place_user_limit_short(&user.account_id, &decrypted, market_id, &qty_str, &price_str).await
        }
    };

    drop(decrypted);

    match res {
        Ok(order) if order.success => {
            let order_id = order.order_id.unwrap_or_else(|| "N/A".into());
            let is_limit = matches!(order_type, crate::bot::state::OrderTypeChoice::Limit);
            let is_pending = is_limit && order.fills.is_empty();
            let final_qty = if !order.quantity.is_empty() { &order.quantity } else { &qty_str };

            if is_pending {
                let p = limit_price.unwrap_or(exec_price);
                let text = format!(
                    "⏳ *{dir_str} Limit Order Placed (Pending)*\n\n\
                     • Market: *{symbol}*\n\
                     • Limit Price: *${}*\n\
                     • Mark Price: *${}*\n\
                     • Size: `{final_qty}` {symbol}\n\
                     • Margin: *${:.2}* ({:.0}x)\n\
                     • Value: *${}*\n\
                     • Order ID: `{order_id}`\n\n\
                     • Status: ⏳ *Resting on Order Book (Pending Fill)*\n\
                     • Note: Order will fill automatically when {symbol} reaches ${}.",
                    format_usd(p),
                    format_usd(mark_price),
                    usd_amount,
                    leverage,
                    format_usd(total_notional),
                    format_usd(p),
                );

                bot.send_message(chat_id, text)
                    .reply_markup(pending_order_placed_keyboard(&symbol))
                    .parse_mode(ParseMode::Markdown)
                    .await?;
            } else {
                let fill_summary = if !order.fills.is_empty() {
                    let fill_str = order
                        .fills
                        .iter()
                        .map(|f| format!("• Filled `{}` @ `${}`", f.quantity, f.price))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("\n\n*Fills:*\n{fill_str}")
                } else {
                    String::new()
                };

                let type_str = match order_type {
                    crate::bot::state::OrderTypeChoice::Market => "Market",
                    crate::bot::state::OrderTypeChoice::Limit => "Limit",
                };

                let price_line = match limit_price {
                    Some(p) => format!("• Limit Price: *${}*\n• Mark Price: *${}*", format_usd(p), format_usd(mark_price)),
                    None => format!("• Price: *Market* (${})", format_usd(mark_price)),
                };

                let text = format!(
                    "{emoji} *{dir_str} {type_str} Order Filled!*\n\n\
                     • Market: *{symbol}*\n\
                     • Margin: *${:.2}*\n\
                     • Leverage: *{:.0}x*\n\
                     • Value: *${}*\n\
                     • Size: `{final_qty}` {symbol}\n\
                     {}\n\
                     • Order ID: `{order_id}`\
                     {fill_summary}",
                    usd_amount,
                    leverage,
                    format_usd(total_notional),
                    price_line,
                );
                bot.send_message(chat_id, text)
                    .reply_markup(order_placed_keyboard(&symbol))
                    .parse_mode(ParseMode::Markdown)
                    .await?;
            }
        }
        Ok(order) => {
            let err = order.error_msg.unwrap_or_else(|| "Engine rejected order".into());
            bot.send_message(
                chat_id,
                format!("❌ *Order Rejected*\n\nReason: {err}"),
            )
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
        }
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ *Failed to submit order*\n\n{e}"),
            )
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
        }
    }

    Ok(())
}

// ─── Slash Command Handlers ───────────────────────────────────────────────────

async fn cmd_start(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    show_main_menu(bot, chat_id, None, user_id, state).await
}

async fn cmd_help(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    let is_connected = state.users.get_user(user_id).await.is_some();
    let conn_status = if is_connected {
        "🟢 *Wallet Connected*"
    } else {
        "⚪ *Wallet Not Connected* — send `/connect` to link your key"
    };

    let msg = format!(
        "💼 *Pocket Real*\n\n\
         {}\n\n\
         *Available Commands:*\n\
         • `/start` — Open interactive trading menu\n\
         • `/positions` — View open positions & live PnL\n\
         • `/orders` — View and cancel open limit orders\n\
         • `/connect` — Connect wallet (`iotaprivkey1...`)\n\
         • `/disconnect` — Disconnect wallet & purge keys\n\
         • `/balance` — View total equity and available margin\n\
         • `/volume` — View your total volume breakdown\n\
         • `/history` — View your recent trade fills\n\
         • `/alert` — Set price alerts (e.g. `/alert btc 85000`)\n\
         • `/long <market>` — Start long order (e.g. `/long btc`)\n\
         • `/short <market>` — Start short order (e.g. `/short eth`)\n\
         • `/close <market>` — Close position (e.g. `/close btc`)\n\
         • `/admin` — View bot stats (admin only)\n\
         • `/cancel` — Abort active setup",
        conn_status
    );

    bot.send_message(chat_id, msg)
        .reply_markup(main_menu_keyboard(is_connected))
        .parse_mode(ParseMode::Markdown)
        .await?;
    Ok(())
}

async fn cmd_positions(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    show_positions_view(bot, chat_id, None, user_id, state).await
}

async fn cmd_orders(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    show_orders_view(bot, chat_id, None, user_id, state).await
}

async fn cmd_history(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    show_history_view(bot, chat_id, None, user_id, state).await
}

async fn cmd_alert(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    args: &str,
) -> ResponseResult<()> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.len() == 2 {
        let sym_arg = tokens[0];
        let price_arg = tokens[1].replace('$', "").replace(',', "");

        let target_price = match price_arg.parse::<f64>() {
            Ok(p) if p > 0.0 => p,
            _ => {
                bot.send_message(chat_id, "❌ Invalid price. Example: `/alert btc 85000`")
                    .parse_mode(ParseMode::Markdown)
                    .await?;
                return Ok(());
            }
        };

        let market_id = match state.resolve_market(sym_arg) {
            Some(m) => m,
            None => {
                bot.send_message(
                    chat_id,
                    format!("❌ Unknown market. Available: {}", state.available_markets_summary()),
                )
                .await?;
                return Ok(());
            }
        };
        let sym_upper = BotState::market_name(&market_id);
        let current_price = state.trading.get_market_mark_price(&market_id).await.unwrap_or(target_price);
        let trigger_above = target_price >= current_price;
        let dir_str = if trigger_above { "rises to or above" } else { "drops to or below" };

        let alert = crate::alerts::PriceAlert {
            id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
            telegram_id: user_id,
            chat_id: chat_id.0,
            market_id,
            symbol: sym_upper.clone(),
            target_price,
            trigger_above,
            created_at: chrono::Utc::now().timestamp(),
            triggered: false,
        };

        let _ = state.alerts.add_alert(alert).await;

        let text = format!(
            "🔔 *Price Alert Created!*\n\n\
             • Asset: *{sym_upper}*\n\
             • Target: *${}*\n\
             • Current Mark: *${}*\n\
             • Condition: Alert when price {} *${}*",
            format_usd(target_price),
            format_usd(current_price),
            dir_str,
            format_usd(target_price),
        );

        bot.send_message(chat_id, text)
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
        return Ok(());
    }

    show_alerts_menu(bot, chat_id, None).await
}

async fn cmd_admin(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
) -> ResponseResult<()> {
    match state.config.admin_user_id {
        Some(admin_id) if admin_id == user_id => {}
        _ => {
            bot.send_message(chat_id, "⛔ *Access Denied*: You are not authorized to use admin commands.")
                .parse_mode(ParseMode::Markdown)
                .await?;
            return Ok(());
        }
    }

    send_typing(bot, chat_id).await;

    let all_users = state.users.get_all_users().await;
    let total_users = all_users.len();

    let now = chrono::Utc::now().timestamp();
    let seven_days_ago = now - (7 * 86400);
    let active_users = all_users
        .iter()
        .filter(|u| u.connected_at >= seven_days_ago)
        .count();

    let total_vol = state.trading.get_all_users_volume(&all_users).await.unwrap_or(0.0);

    let text = format!(
        "👑 *Pocket Real Admin Dashboard*\n\n\
         • *Total Registered Users:* `{total_users}`\n\
         • *Active Users (7d):* `{active_users}`\n\
         • *Total Traded Volume:* `${}`\n\n\
         _Confidential — private keys remain hardware-isolated and AES-256 encrypted._",
        format_volume(total_vol)
    );

    bot.send_message(chat_id, text)
        .reply_markup(back_to_main_keyboard())
        .parse_mode(ParseMode::Markdown)
        .await?;

    Ok(())
}

async fn cmd_balance(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    show_balance_view(bot, chat_id, None, user_id, state).await
}

async fn cmd_volume(bot: &Bot, chat_id: ChatId, user_id: u64, state: &BotState) -> ResponseResult<()> {
    // /volume strictly displays user trading volume statistics; it NEVER places any orders.
    show_volume_view(bot, chat_id, None, user_id, state).await
}

async fn cmd_cancel(bot: &Bot, chat_id: ChatId) -> ResponseResult<()> {
    bot.send_message(chat_id, "❌ Active trade setup cancelled.")
        .reply_markup(back_to_main_keyboard())
        .await?;
    Ok(())
}

async fn cmd_long(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    args: &str,
) -> ResponseResult<()> {
    if state.users.get_user(user_id).await.is_none() {
        return show_not_connected_notice(bot, chat_id, None).await;
    }

    let tokens: Vec<&str> = args.split_whitespace().collect();

    if tokens.is_empty() {
        show_markets_menu(bot, chat_id, None, state).await?;
        return Ok(());
    }

    if tokens.len() == 1 {
        start_trade_from_button(bot, chat_id, None, user_id, state, TradeDirection::Long, tokens[0]).await?;
        return Ok(());
    }

    let parsed = match parse_order_input(args, state) {
        Ok(p) => p,
        Err(e) => {
            bot.send_message(chat_id, e.user_message()).await?;
            return Ok(());
        }
    };

    send_typing(bot, chat_id).await;

    let user = state.users.get_user(user_id).await.unwrap();
    let decrypted = match crate::crypto::decrypt_user_key(&state.config.encryption_key, user_id, &user.encrypted_privkey) {
        Ok(k) => k,
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Decryption error: {e}")).await?;
            return Ok(());
        }
    };

    // CRITICAL ORDER FLOW GUARANTEE:
    // Strictly execute ONLY the single requested long order.
    // Under no circumstances place an opposing short, hedge, or auto-closing order.
    // The position remains open until the user manually triggers a close.
    let res = match parsed {
        ParsedOrderArgs::Market { ref market, ref qty } => {
            state.trading.place_user_market_long(&user.account_id, &decrypted, market, qty).await
        }
        ParsedOrderArgs::Limit { ref market, ref qty, ref price } => {
            state.trading.place_user_limit_long(&user.account_id, &decrypted, market, qty, price).await
        }
    };
    drop(decrypted);

    let (text, is_success, is_pending) = match res {
        Ok(ref r) => {
            let is_limit = matches!(parsed, ParsedOrderArgs::Limit { .. });
            let pending = is_limit && r.fills.is_empty() && r.success;
            (format_order_result(r), r.success, pending)
        }
        Err(e) => (format!("❌ Failed to place order: {e}"), false, false),
    };
    let market_sym = match parsed {
        ParsedOrderArgs::Market { ref market, .. } => BotState::market_name(market),
        ParsedOrderArgs::Limit { ref market, .. } => BotState::market_name(market),
    };
    let kb = if is_pending {
        pending_order_placed_keyboard(&market_sym)
    } else if is_success {
        order_placed_keyboard(&market_sym)
    } else {
        back_to_main_keyboard()
    };
    bot.send_message(chat_id, text)
        .reply_markup(kb)
        .parse_mode(ParseMode::Markdown)
        .await?;
    Ok(())
}

async fn cmd_short(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    args: &str,
) -> ResponseResult<()> {
    if state.users.get_user(user_id).await.is_none() {
        return show_not_connected_notice(bot, chat_id, None).await;
    }

    let tokens: Vec<&str> = args.split_whitespace().collect();

    if tokens.is_empty() {
        show_markets_menu(bot, chat_id, None, state).await?;
        return Ok(());
    }

    if tokens.len() == 1 {
        start_trade_from_button(bot, chat_id, None, user_id, state, TradeDirection::Short, tokens[0]).await?;
        return Ok(());
    }

    let parsed = match parse_order_input(args, state) {
        Ok(p) => p,
        Err(e) => {
            bot.send_message(chat_id, e.user_message()).await?;
            return Ok(());
        }
    };

    send_typing(bot, chat_id).await;

    let user = state.users.get_user(user_id).await.unwrap();
    let decrypted = match crate::crypto::decrypt_user_key(&state.config.encryption_key, user_id, &user.encrypted_privkey) {
        Ok(k) => k,
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Decryption error: {e}")).await?;
            return Ok(());
        }
    };

    // CRITICAL ORDER FLOW GUARANTEE:
    // Strictly execute ONLY the single requested short order.
    // Under no circumstances place an opposing long, hedge, or auto-closing order.
    // The position remains open until the user manually triggers a close.
    let res = match parsed {
        ParsedOrderArgs::Market { ref market, ref qty } => {
            state.trading.place_user_market_short(&user.account_id, &decrypted, market, qty).await
        }
        ParsedOrderArgs::Limit { ref market, ref qty, ref price } => {
            state.trading.place_user_limit_short(&user.account_id, &decrypted, market, qty, price).await
        }
    };
    drop(decrypted);

    let (text, is_success, is_pending) = match res {
        Ok(ref r) => {
            let is_limit = matches!(parsed, ParsedOrderArgs::Limit { .. });
            let pending = is_limit && r.fills.is_empty() && r.success;
            (format_order_result(r), r.success, pending)
        }
        Err(e) => (format!("❌ Failed to place order: {e}"), false, false),
    };
    let market_sym = match parsed {
        ParsedOrderArgs::Market { ref market, .. } => BotState::market_name(market),
        ParsedOrderArgs::Limit { ref market, .. } => BotState::market_name(market),
    };
    let kb = if is_pending {
        pending_order_placed_keyboard(&market_sym)
    } else if is_success {
        order_placed_keyboard(&market_sym)
    } else {
        back_to_main_keyboard()
    };
    bot.send_message(chat_id, text)
        .reply_markup(kb)
        .parse_mode(ParseMode::Markdown)
        .await?;
    Ok(())
}

async fn cmd_close(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    state: &BotState,
    args: &str,
) -> ResponseResult<()> {
    let user_opt = state.users.get_user(user_id).await;
    let user = match user_opt {
        Some(u) => u,
        None => return show_not_connected_notice(bot, chat_id, None).await,
    };

    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.is_empty() {
        show_markets_menu(bot, chat_id, None, state).await?;
        return Ok(());
    }

    let (market_token, percentage, manual_price) = parse_close_args(&tokens, state);
    let market_id = match state.resolve_market(&market_token) {
        Some(m) => m,
        None => {
            bot.send_message(
                chat_id,
                format!("❌ Unknown market `{market_token}`. Select a market:"),
            )
            .reply_markup(markets_keyboard(&state.trading.get_cached_markets()))
            .parse_mode(ParseMode::Markdown)
            .await?;
            return Ok(());
        }
    };

    send_typing(bot, chat_id).await;
    let symbol = BotState::market_name(&market_id);

    let pos_opt = match state
        .trading
        .get_user_open_position(&user.account_id, user.indexer_account_id.as_deref(), &market_id)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ Failed to check positions: {e}"),
            )
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
            return Ok(());
        }
    };

    let pos = match pos_opt {
        Some(p) if p.size > 0.0 => p,
        _ => {
            bot.send_message(
                chat_id,
                format!("ℹ️ No open position found for *{symbol}*."),
            )
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
            return Ok(());
        }
    };

    let decimals = crate::trading::get_cached_market_decimals(&symbol);
    let min_size = crate::trading::get_cached_market_lot_size(&symbol);
    let close_fraction = (percentage / 100.0).clamp(0.01, 1.0);
    let raw_close_qty = if percentage >= 99.9 { pos.size } else { pos.size * close_fraction };
    let (mut qty_str, rounded_close_qty) = crate::trading::round_qty_to_decimals(raw_close_qty, decimals);
    if rounded_close_qty <= 0.0 && pos.size > 0.0 {
        let (fallback_str, _) = crate::trading::round_qty_to_decimals(pos.size.min(min_size), decimals);
        qty_str = fallback_str;
    }

    let is_pos_long = pos.direction.eq_ignore_ascii_case("long") || pos.direction.eq_ignore_ascii_case("buy");
    let close_side = if is_pos_long { "Sell" } else { "Buy" };

    // Decrypt key into Zeroizing buffer
    let decrypted = match crate::crypto::decrypt_user_key(&state.config.encryption_key, user_id, &user.encrypted_privkey) {
        Ok(k) => k,
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ Decryption error: {e}"),
            )
            .reply_markup(back_to_main_keyboard())
            .await?;
            return Ok(());
        }
    };

    let res = state
        .trading
        .place_user_close_order(&user.account_id, &decrypted, &market_id, close_side, &qty_str, manual_price.as_deref())
        .await;

    drop(decrypted);

    match res {
        Ok(order) if order.success => {
            let order_id = order.order_id.unwrap_or_else(|| "N/A".into());
            let price_desc = match manual_price {
                Some(ref p) => format!("Limit @ `${p}`"),
                None => "Market Price".to_string(),
            };
            let text = format!(
                "✅ *Close Order Submitted*\n\n\
                 • Market: *{symbol}*\n\
                 • Position: *{pos_dir}* ({pos_size} {symbol})\n\
                 • Closing: *{pct:.0}%* (`{qty_str}` {symbol})\n\
                 • Price: *{price_desc}*\n\
                 • Order ID: `{order_id}`",
                pos_dir = pos.direction,
                pos_size = pos.size,
                pct = percentage,
                order_id = order_id,
            );
            bot.send_message(chat_id, text)
                .reply_markup(back_to_main_keyboard())
                .parse_mode(ParseMode::Markdown)
                .await?;
        }
        Ok(order) => {
            let err = order.error_msg.unwrap_or_else(|| "Engine rejected order".into());
            bot.send_message(
                chat_id,
                format!("❌ *Close Order Rejected*\n\nReason: {err}"),
            )
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
        }
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ *Failed to submit close order*\n\n{e}"),
            )
            .reply_markup(back_to_main_keyboard())
            .parse_mode(ParseMode::Markdown)
            .await?;
        }
    }

    Ok(())
}

fn parse_close_args(tokens: &[&str], state: &BotState) -> (String, f64, Option<String>) {
    if tokens.is_empty() {
        return (
            state.config.default_market.clone().unwrap_or_else(|| "btc".into()),
            100.0,
            None,
        );
    }

    let token0_clean = tokens[0].trim_end_matches('%');
    if let Ok(num) = token0_clean.parse::<f64>() {
        let default_m = state.config.default_market.clone().unwrap_or_else(|| "btc".into());
        if num <= 100.0 {
            let price = tokens.get(1).map(|s| s.to_string());
            return (default_m, num, price);
        } else {
            return (default_m, 100.0, Some(tokens[0].to_string()));
        }
    }

    let market = tokens[0].to_string();

    if tokens.len() == 1 {
        return (market, 100.0, None);
    }

    let token1_clean = tokens[1].trim_end_matches('%');
    if let Ok(num) = token1_clean.parse::<f64>() {
        if num <= 100.0 {
            let price = tokens.get(2).map(|s| s.to_string());
            (market, num, price)
        } else {
            (market, 100.0, Some(tokens[1].to_string()))
        }
    } else {
        (market, 100.0, None)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

enum ParsedOrderArgs {
    Limit { market: String, qty: String, price: String },
    Market { market: String, qty: String },
}

fn parse_order_input(
    args: &str,
    state: &BotState,
) -> Result<ParsedOrderArgs, BotError> {
    let parts: Vec<&str> = args.split_whitespace().collect();
    match parts.as_slice() {
        [market_arg, qty, price] => {
            let market = state.resolve_market(market_arg).ok_or_else(|| {
                BotError::InvalidArg(format!(
                    "Unknown market '{market_arg}'. Available: {}",
                    state.available_markets_summary()
                ))
            })?;
            let (clean_qty, _decimals, rounded) = crate::trading::sanitize_order_quantity_for_market(&market, qty);
            if rounded <= 0.0 {
                let min_lot = crate::trading::get_cached_market_lot_size(&market);
                let err_msg = if min_lot > 0.0 {
                    format!("Order size must be at least {min_lot}")
                } else {
                    "Order size must be greater than 0.".into()
                };
                return Err(BotError::InvalidArg(err_msg));
            }
            let (clean_price, _, _) = crate::trading::sanitize_order_price_for_market(&market, price);
            Ok(ParsedOrderArgs::Limit {
                market,
                qty: clean_qty,
                price: clean_price,
            })
        }
        [arg1, arg2] => {
            if let Some(market) = state.resolve_market(arg1) {
                let (clean_qty, _decimals, rounded) = crate::trading::sanitize_order_quantity_for_market(&market, arg2);
                if rounded <= 0.0 {
                    let min_lot = crate::trading::get_cached_market_lot_size(&market);
                    let err_msg = if min_lot > 0.0 {
                        format!("Order size must be at least {min_lot}")
                    } else {
                        "Order size must be greater than 0.".into()
                    };
                    return Err(BotError::InvalidArg(err_msg));
                }
                Ok(ParsedOrderArgs::Market {
                    market,
                    qty: clean_qty,
                })
            } else {
                let market = state
                    .resolve_market("")
                    .ok_or(BotError::NoDefaultMarket)?;
                let (clean_qty, _decimals, rounded) = crate::trading::sanitize_order_quantity_for_market(&market, arg1);
                if rounded <= 0.0 {
                    let min_lot = crate::trading::get_cached_market_lot_size(&market);
                    let err_msg = if min_lot > 0.0 {
                        format!("Order size must be at least {min_lot}")
                    } else {
                        "Order size must be greater than 0.".into()
                    };
                    return Err(BotError::InvalidArg(err_msg));
                }
                let (clean_price, _, _) = crate::trading::sanitize_order_price_for_market(&market, arg2);
                Ok(ParsedOrderArgs::Limit {
                    market,
                    qty: clean_qty,
                    price: clean_price,
                })
            }
        }
        _ => Err(BotError::InvalidArg(
            "Usage:\n• /long <market> (interactive)\n• /long <market> <size> (market order)\n• /long <market> <size> <price> (limit order)\n\nExample: `/long btc 0.1` or `/long btc 0.1 82000`".into(),
        )),
    }
}

fn format_order_result(r: &crate::trading::OrderResult) -> String {
    let side_emoji = if r.side.to_lowercase().contains("buy") { "📈" } else { "📉" };
    let is_limit = r.price != "Market";
    let is_pending = is_limit && r.fills.is_empty() && r.success;

    let title = if is_pending {
        format!("⏳ {side_emoji} *{} Limit Order Placed (Pending)*", r.side)
    } else if r.success {
        format!("✅ {side_emoji} *{} Order*", r.side)
    } else {
        format!("❌ {side_emoji} *{} Order Rejected*", r.side)
    };

    let market_display = BotState::market_name(&r.market);

    let mut text = format!(
        "{title}\n\n\
         • Market: *{market_display}*\n\
         • Size: `{}`\n\
         • Price: `${}`\n\
         • Client ID: `{}`",
        r.quantity,
        r.price,
        r.client_order_id,
    );

    if let Some(ref oid) = r.order_id {
        text.push_str(&format!("\n• Order ID: `{oid}`"));
    }

    if is_pending {
        text.push_str("\n\n• Status: ⏳ *Resting on Order Book (Pending Fill)*\n• Note: Order will fill automatically when price reaches target.");
    }

    if !r.fills.is_empty() {
        text.push_str("\n\n*Fills:*");
        for f in &r.fills {
            text.push_str(&format!(
                "\n• qty `{}` @ `${}`",
                f.quantity,
                f.price
            ));
        }
    }

    if let Some(ref err) = r.error_msg {
        text.push_str(&format!("\n\n*Error:*\n• {err}"));
    }

    text
}

fn format_usd(val: f64) -> String {
    let formatted = format!("{:.2}", val);
    let parts: Vec<&str> = formatted.split('.').collect();
    let int_part = add_commas(parts[0]);
    if parts.len() > 1 {
        format!("{}.{}", int_part, parts[1])
    } else {
        int_part
    }
}

/// Format order quantity (size) rounded to the market's allowed decimals before submitting to Real.xyz.
/// Prevents Real.xyz engine error 121 (OrderQuantityHasTooManyDecimalPlaces).
fn format_asset_qty(symbol: impl AsRef<str>, qty: f64) -> String {
    let decimals = crate::trading::get_cached_market_decimals(symbol.as_ref());
    let (formatted, _) = crate::trading::round_qty_to_decimals(qty, decimals);
    formatted
}

fn format_volume(val: f64) -> String {
    let rounded = val.round() as i64;
    add_commas(&rounded.to_string())
}

fn add_commas(num_str: &str) -> String {
    let mut result = String::new();
    let is_negative = num_str.starts_with('-');
    let digits = if is_negative { &num_str[1..] } else { num_str };
    let len = digits.len();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    if is_negative {
        format!("-{}", result)
    } else {
        result
    }
}

async fn send_typing(bot: &Bot, chat_id: ChatId) {
    let _ = bot
        .send_chat_action(chat_id, teloxide::types::ChatAction::Typing)
        .await;
}
