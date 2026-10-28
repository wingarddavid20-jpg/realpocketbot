//! Shared application state passed to every handler via `dptree`.

use std::{collections::HashMap, sync::Arc};
use teloxide::types::ChatId;
use tokio::sync::RwLock;
use crate::{alerts::AlertStore, config::Config, trading::TradingClient, users::UserStore};

pub const BTC_MARKET: &str = "0x03d9f492be9ec3289c0ecbd5b60e9df3c8c74bb5fd51f697a2b85f065f51a9d5";
pub const ETH_MARKET: &str = "0xc677aafef3f33649973c8d86acb0f929f54fe1a6168b200399b9465d9e0fd00c";
pub const SOL_MARKET: &str = "0xca22f2b22f8c16be70d29e5cc8434bedb778c51d6cb224b64c3ee7d152567936";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDirection {
    Long,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderTypeChoice {
    Market,
    Limit,
}

#[derive(Debug, Clone)]
pub enum TradeStep {
    AwaitingPrivateKey,
    AwaitingMarket,
    AwaitingUsdAmount,
    AwaitingLeverage { usd_amount: f64 },
    AwaitingOrderType { usd_amount: f64, leverage: f64 },
    AwaitingLimitPrice { usd_amount: f64, leverage: f64 },
    AwaitingAlertTarget { market_id: String, symbol: String },
    AwaitingClosePercentage { market_id: String, symbol: String },
}

#[derive(Debug, Clone)]
pub struct PendingTrade {
    pub direction: TradeDirection,
    pub market_id: String,
    pub market_symbol: String,
    pub step: TradeStep,
}

/// State shared across all Telegram handlers.
pub struct BotState {
    pub config: Config,
    pub trading: TradingClient,
    pub users: Arc<UserStore>,
    pub alerts: Arc<AlertStore>,
    sessions: RwLock<HashMap<ChatId, PendingTrade>>,
}

impl BotState {
    pub fn new(
        config: Config,
        trading: TradingClient,
        users: Arc<UserStore>,
        alerts: Arc<AlertStore>,
    ) -> Self {
        Self {
            config,
            trading,
            users,
            alerts,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn set_session(&self, chat_id: ChatId, trade: PendingTrade) {
        let mut map = self.sessions.write().await;
        map.insert(chat_id, trade);
    }

    pub async fn get_session(&self, chat_id: ChatId) -> Option<PendingTrade> {
        let map = self.sessions.read().await;
        map.get(&chat_id).cloned()
    }

    pub async fn clear_session(&self, chat_id: ChatId) {
        let mut map = self.sessions.write().await;
        map.remove(&chat_id);
    }

    /// Resolve a market identifier dynamically: accepts short symbols like "btc", "eth", "sol", "hype",
    /// full pair names like "BTC-USDT", or explicit "0x..." IDs.
    /// Falls back to `DEFAULT_MARKET` if empty.
    pub fn resolve_market(&self, provided: &str) -> Option<String> {
        let trimmed = provided.trim();
        if trimmed.is_empty() {
            if let Some(ref def) = self.config.default_market {
                return Some(def.clone());
            }
            return self.trading.get_cached_markets().first().map(|m| m.market_id.clone());
        }

        self.trading.resolve_market(trimmed)
    }

    /// Format a market ID into a user-friendly name (e.g. "BTC", "ETH", "SOL", "HYPE").
    pub fn market_name(market_id: &str) -> String {
        if let Some(name) = crate::trading::get_cached_market_name(market_id) {
            return name;
        }

        if market_id.eq_ignore_ascii_case(BTC_MARKET) {
            "BTC".into()
        } else if market_id.eq_ignore_ascii_case(ETH_MARKET) {
            "ETH".into()
        } else if market_id.eq_ignore_ascii_case(SOL_MARKET) {
            "SOL".into()
        } else if market_id.eq_ignore_ascii_case("0x7fa781f5e1d1da7165cf96e5a6898235f55f2d812dd429d3feed69ff776dfa70") {
            "HYPE".into()
        } else if market_id.len() > 10 {
            format!("{}…{}", &market_id[..6], &market_id[market_id.len() - 4..])
        } else {
            market_id.to_string()
        }
    }

    /// Summary of all currently available short market names (e.g. "btc, eth, sol, hype").
    pub fn available_markets_summary(&self) -> String {
        let markets = self.trading.get_cached_markets();
        markets
            .iter()
            .map(|m| m.short_name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Type alias for the shared state reference threaded through handlers.
pub type SharedState = Arc<BotState>;
