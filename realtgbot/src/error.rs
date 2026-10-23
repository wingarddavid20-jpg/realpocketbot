//! Unified error type for the bot.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BotError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Trading error: {0}")]
    Trading(String),

    #[error("Invalid command argument: {0}")]
    InvalidArg(String),

    #[error("SDK error: {0}")]
    Sdk(#[from] anyhow::Error),

    #[error("Order was rejected by the matching engine: {0}")]
    OrderRejected(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("No default market configured — pass <market> explicitly")]
    NoDefaultMarket,
}

impl BotError {
    /// Human-readable message suitable for sending back to the Telegram user.
    pub fn user_message(&self) -> String {
        match self {
            BotError::Config(msg) => format!("⚙️ Configuration problem: {msg}"),
            BotError::Trading(msg) => format!("⚠️ Trading error: {msg}"),
            BotError::InvalidArg(msg) => format!("❌ Bad argument: {msg}"),
            BotError::Sdk(e) => format!("🔧 SDK error: {e}"),
            BotError::OrderRejected(reason) => format!("🚫 Order rejected: {reason}"),
            BotError::Internal(msg) => format!("⚠️ Internal error: {msg}"),
            BotError::NoDefaultMarket => {
                "❌ No default market set. Pass market ID explicitly.".into()
            }
        }
    }
}

/// Convenience alias used throughout the crate.
pub type BotResult<T> = Result<T, BotError>;
