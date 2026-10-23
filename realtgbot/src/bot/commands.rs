//! Telegram slash-command definitions.

use teloxide::utils::command::BotCommands;

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Real.xyz Trading Bot:")]
pub enum Command {
    #[command(description = "Show available commands")]
    Help,

    #[command(description = "Welcome message and bot status")]
    Start,

    #[command(description = "Show account balance and available margin")]
    Balance,

    #[command(
        description = "Open a Long position (step-by-step) — /long <market>\nExample: /long btc"
    )]
    Long(String),

    #[command(
        description = "Open a Short position (step-by-step) — /short <market>\nExample: /short eth"
    )]
    Short(String),

    #[command(
        description = "Close position — /close <market> [percentage] [price]\nExample: /close btc, /close btc 50"
    )]
    Close(String),

    #[command(description = "Show total trading volume breakdown")]
    Volume,

    #[command(
        description = "Connect your wallet with IOTA private key — /connect [key]"
    )]
    Connect(String),

    #[command(description = "Disconnect wallet and delete encrypted keys")]
    Disconnect,

    #[command(description = "Show open positions with live PnL")]
    Positions,

    #[command(description = "Set or view price alerts — /alert <market> <price>")]
    Alert(String),

    #[command(description = "Show recent trade history")]
    History,

    #[command(description = "Admin stats dashboard (admin only)")]
    Admin,

    #[command(description = "Cancel active trade setup")]
    Cancel,
}
