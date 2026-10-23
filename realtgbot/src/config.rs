//! Application configuration loaded from environment variables.
//!
//! Required variables:
//! - `TELEGRAM_BOT_TOKEN` — Telegram BotFather token
//! - `PRIVATE_KEY`        — IOTA bech32 private key (`iotaprivkey1…`)
//! - `ACCOUNT_ID`         — on-chain trading account object ID (`0x…`)
//!
//! Optional:
//! - `DEFAULT_MARKET`     — default market object ID if omitted from commands
//! - `NETWORK`            — `testnet` (default) or `mainnet`

use crate::error::BotError;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// Telegram bot token
    pub telegram_token: String,
    /// IOTA bech32 private key (never logged)
    pub private_key: String,
    /// On-chain trading account object ID
    pub account_id: String,
    /// Optional default market object ID
    pub default_market: Option<String>,
    /// Network to connect to
    pub network: Network,
    /// Master encryption key for user private keys
    pub encryption_key: String,
    /// Telegram user ID authorized for admin commands
    pub admin_user_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    Testnet,
    Mainnet,
}

impl Network {
    pub fn indexer_url(&self) -> &'static str {
        match self {
            Network::Testnet => "https://indexer.api.testnet.real.xyz",
            Network::Mainnet => "https://indexer.api.real.xyz",
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Network::Testnet => write!(f, "testnet"),
            Network::Mainnet => write!(f, "mainnet"),
        }
    }
}

impl Config {
    /// Read and validate all required environment variables.
    pub fn from_env() -> Result<Self, BotError> {
        let telegram_token = require_env("TELEGRAM_BOT_TOKEN")?;
        let private_key = require_env("PRIVATE_KEY")?;
        let account_id = require_env("ACCOUNT_ID")?;
        let default_market = env::var("DEFAULT_MARKET").ok();

        // Validate formats eagerly so we fail fast on startup
        if !private_key.starts_with("iotaprivkey1") {
            return Err(BotError::Config(
                "PRIVATE_KEY must be an IOTA bech32 key starting with 'iotaprivkey1'".into(),
            ));
        }

        if !account_id.starts_with("0x") {
            return Err(BotError::Config(
                "ACCOUNT_ID must be a 0x-prefixed hex object ID".into(),
            ));
        }

        if let Some(ref m) = default_market {
            if !m.starts_with("0x") {
                return Err(BotError::Config(
                    "DEFAULT_MARKET must be a 0x-prefixed hex object ID".into(),
                ));
            }
        }

        let network = match env::var("NETWORK").as_deref() {
            Ok("mainnet") => Network::Mainnet,
            _ => Network::Testnet, // default
        };

        let encryption_key = env::var("ENCRYPTION_KEY").unwrap_or_else(|_| {
            format!("realtgbot_enc_{private_key}")
        });

        let admin_user_id = env::var("ADMIN_USER_ID")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());

        Ok(Self {
            telegram_token,
            private_key,
            account_id,
            default_market,
            network,
            encryption_key,
            admin_user_id,
        })
    }
}

fn require_env(key: &str) -> Result<String, BotError> {
    env::var(key).map_err(|_| {
        BotError::Config(format!(
            "Required environment variable '{}' is not set",
            key
        ))
    })
}
