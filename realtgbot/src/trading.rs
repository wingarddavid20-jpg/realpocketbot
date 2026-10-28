//! All Real.xyz trading logic lives here, cleanly separated from Telegram.
//!
//! `TradingClient` wraps the SDK's `SigningClient` and exposes high-level
//! async methods for every bot command. Handlers in `bot/` call only these
//! methods and never touch the SDK types directly.

use std::{collections::HashMap, sync::Arc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use sdk::{
    SigningClient,
    crypto::{RawSigner, Signer},
    types::{
        CancelOrderRequest, OrdersTerminated, Report, SubmitOrderRequest,
        TradeExecuted, TransactionReport,
    },
};

use crate::{
    bot::state::{BTC_MARKET, ETH_MARKET, SOL_MARKET},
    config::{Config, Network},
    error::{BotError, BotResult},
};

// ─── Global Dynamic Market Names & Precision Cache ──────────────────────────

/// Global map for instantaneous market_id -> base_asset symbol resolution.
static CACHED_MARKET_NAMES: std::sync::RwLock<Option<HashMap<String, String>>> =
    std::sync::RwLock::new(None);

/// Global map for instantaneous market_id / symbol / base_asset -> quantity decimal precision.
static CACHED_MARKET_DECIMALS: std::sync::RwLock<Option<HashMap<String, usize>>> =
    std::sync::RwLock::new(None);

/// Global map for instantaneous market_id / symbol / base_asset -> minimum order lot size.
static CACHED_MARKET_LOT_SIZES: std::sync::RwLock<Option<HashMap<String, f64>>> =
    std::sync::RwLock::new(None);

/// Global map for instantaneous market_id / symbol / base_asset -> price decimal precision.
static CACHED_MARKET_PRICE_DECIMALS: std::sync::RwLock<Option<HashMap<String, usize>>> =
    std::sync::RwLock::new(None);

/// Fallback precision map matching Real.xyz engine order rules (size_decimal_places):
/// BTC → 4 decimals (lot size 0.0001), ETH → 3 decimals (lot size 0.001),
/// SOL → 2 decimals (lot size 0.01), HYPE → 2 decimals (lot size 0.01),
/// DOGE/XRP → 0 decimals, default → 3 decimals.
pub fn fallback_qty_decimals(identifier: &str) -> usize {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") {
        4
    } else if clean.starts_with("ETH") {
        3
    } else if clean.starts_with("SOL") {
        2
    } else if clean.starts_with("HYPE") {
        2
    } else if clean.starts_with("DOGE") || clean.starts_with("XRP") {
        0
    } else if clean.starts_with("BNB") || clean.starts_with("ZEC") {
        2
    } else {
        3
    }
}

pub fn fallback_market_lot_size(identifier: &str) -> f64 {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") {
        0.0001
    } else if clean.starts_with("ETH") {
        0.001
    } else if clean.starts_with("SOL") {
        0.01
    } else if clean.starts_with("HYPE") {
        0.01
    } else if clean.starts_with("DOGE") {
        10.0
    } else if clean.starts_with("XRP") {
        1.0
    } else {
        0.001
    }
}

pub fn fallback_price_decimals(identifier: &str) -> usize {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") {
        1
    } else if clean.starts_with("ETH") {
        2
    } else if clean.starts_with("SOL") {
        3
    } else if clean.starts_with("HYPE") {
        3
    } else if clean.starts_with("XAUT") {
        2
    } else {
        1
    }
}

pub fn update_cached_market_names(markets: &[MarketInfo]) {
    let mut map = HashMap::new();
    for m in markets {
        map.insert(m.market_id.to_lowercase(), m.base_asset.clone());
    }
    if let Ok(mut guard) = CACHED_MARKET_NAMES.write() {
        *guard = Some(map);
    }
}

pub fn update_cached_market_decimals(markets: &[MarketInfo]) {
    let mut map = HashMap::new();
    for m in markets {
        let dec = m.qty_decimals;
        map.insert(m.market_id.to_lowercase(), dec);
        map.insert(m.base_asset.to_lowercase(), dec);
        map.insert(m.symbol.to_lowercase(), dec);
        map.insert(m.short_name.to_lowercase(), dec);
    }
    if let Ok(mut guard) = CACHED_MARKET_DECIMALS.write() {
        if let Some(ref mut existing) = *guard {
            for (k, v) in map {
                existing.insert(k, v);
            }
        } else {
            *guard = Some(map);
        }
    }
}

pub fn update_cached_market_lot_sizes(markets: &[MarketInfo]) {
    let mut map = HashMap::new();
    for m in markets {
        let lot = m.lot_size;
        map.insert(m.market_id.to_lowercase(), lot);
        map.insert(m.base_asset.to_lowercase(), lot);
        map.insert(m.symbol.to_lowercase(), lot);
        map.insert(m.short_name.to_lowercase(), lot);
    }
    if let Ok(mut guard) = CACHED_MARKET_LOT_SIZES.write() {
        if let Some(ref mut existing) = *guard {
            for (k, v) in map {
                existing.insert(k, v);
            }
        } else {
            *guard = Some(map);
        }
    }
}

pub fn set_cached_market_decimals(market_id_or_sym: &str, decimals: usize) {
    let key = market_id_or_sym.trim().to_lowercase();
    if let Ok(mut guard) = CACHED_MARKET_DECIMALS.write() {
        if let Some(ref mut map) = *guard {
            map.insert(key, decimals);
        } else {
            let mut map = HashMap::new();
            map.insert(key, decimals);
            *guard = Some(map);
        }
    }
}

pub fn get_cached_market_name(market_id: &str) -> Option<String> {
    if let Ok(guard) = CACHED_MARKET_NAMES.read() {
        if let Some(ref map) = *guard {
            if let Some(name) = map.get(&market_id.to_lowercase()) {
                return Some(name.clone());
            }
        }
    }
    None
}

pub fn get_cached_market_decimals(identifier: &str) -> usize {
    let key = identifier.trim().to_lowercase();
    if let Ok(guard) = CACHED_MARKET_DECIMALS.read() {
        if let Some(ref map) = *guard {
            if let Some(&dec) = map.get(&key) {
                return dec;
            }
        }
    }
    if let Some(base_name) = get_cached_market_name(identifier) {
        if let Ok(guard) = CACHED_MARKET_DECIMALS.read() {
            if let Some(ref map) = *guard {
                if let Some(&dec) = map.get(&base_name.to_lowercase()) {
                    return dec;
                }
            }
        }
        return fallback_qty_decimals(&base_name);
    }
    fallback_qty_decimals(identifier)
}

pub fn get_cached_market_lot_size(identifier: &str) -> f64 {
    let key = identifier.trim().to_lowercase();
    if let Ok(guard) = CACHED_MARKET_LOT_SIZES.read() {
        if let Some(ref map) = *guard {
            if let Some(&lot) = map.get(&key) {
                return lot;
            }
        }
    }
    if let Some(base_name) = get_cached_market_name(identifier) {
        if let Ok(guard) = CACHED_MARKET_LOT_SIZES.read() {
            if let Some(ref map) = *guard {
                if let Some(&lot) = map.get(&base_name.to_lowercase()) {
                    return lot;
                }
            }
        }
        return fallback_market_lot_size(&base_name);
    }
    fallback_market_lot_size(identifier)
}

pub fn update_cached_market_price_decimals(markets: &[MarketInfo]) {
    let mut map = HashMap::new();
    for m in markets {
        let dec = m.price_decimals;
        map.insert(m.market_id.to_lowercase(), dec);
        map.insert(m.base_asset.to_lowercase(), dec);
        map.insert(m.symbol.to_lowercase(), dec);
        map.insert(m.short_name.to_lowercase(), dec);
    }
    if let Ok(mut guard) = CACHED_MARKET_PRICE_DECIMALS.write() {
        if let Some(ref mut existing) = *guard {
            for (k, v) in map {
                existing.insert(k, v);
            }
        } else {
            *guard = Some(map);
        }
    }
}

pub fn set_cached_market_price_decimals(market_id_or_sym: &str, decimals: usize) {
    let key = market_id_or_sym.trim().to_lowercase();
    if let Ok(mut guard) = CACHED_MARKET_PRICE_DECIMALS.write() {
        if let Some(ref mut map) = *guard {
            map.insert(key, decimals);
        } else {
            let mut map = HashMap::new();
            map.insert(key, decimals);
            *guard = Some(map);
        }
    }
}

pub fn get_cached_market_price_decimals(identifier: &str) -> usize {
    let key = identifier.trim().to_lowercase();
    if let Ok(guard) = CACHED_MARKET_PRICE_DECIMALS.read() {
        if let Some(ref map) = *guard {
            if let Some(&dec) = map.get(&key) {
                return dec;
            }
        }
    }
    if let Some(base_name) = get_cached_market_name(identifier) {
        if let Ok(guard) = CACHED_MARKET_PRICE_DECIMALS.read() {
            if let Some(ref map) = *guard {
                if let Some(&dec) = map.get(&base_name.to_lowercase()) {
                    return dec;
                }
            }
        }
        return fallback_price_decimals(&base_name);
    }
    fallback_price_decimals(identifier)
}

/// Parse quantity precision / lot size from indexer ticker payload, with fallback map.
pub fn parse_qty_decimals(item: &serde_json::Value, base_asset: &str) -> usize {
    let direct_keys = [
        "quantity_precision",
        "qty_precision",
        "size_decimals",
        "qty_decimals",
        "base_asset_precision",
        "lot_size_decimals",
    ];
    for key in direct_keys {
        if let Some(val) = item.get(key) {
            if let Some(n) = val.as_u64() {
                if n <= 8 {
                    return n as usize;
                }
            } else if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    if n <= 8 {
                        return n;
                    }
                }
            }
        }
    }

    let lot_keys = [
        "lot_size",
        "step_size",
        "min_order_size",
        "min_qty",
        "min_quantity",
        "order_size_increment",
    ];
    for key in lot_keys {
        if let Some(val) = item.get(key) {
            if let Some(s) = val.as_str() {
                if let Some(pos) = s.find('.') {
                    let decimals = s[pos + 1..].trim_end_matches('0').len();
                    if decimals > 0 && decimals <= 8 {
                        return decimals;
                    }
                }
            } else if let Some(f) = val.as_f64() {
                if f > 0.0 && f < 1.0 {
                    let s = format!("{:.8}", f);
                    if let Some(pos) = s.find('.') {
                        let decimals = s[pos + 1..].trim_end_matches('0').len();
                        if decimals > 0 && decimals <= 8 {
                            return decimals;
                        }
                    }
                }
            }
        }
    }

    fallback_qty_decimals(base_asset)
}

/// Round a quantity to a specified number of decimal places and format as string.
/// E.g. round_qty_to_decimals(0.0148, 2) -> ("0.01", 0.01)
pub fn round_qty_to_decimals(qty: f64, decimals: usize) -> (String, f64) {
    let factor = 10_f64.powi(decimals as i32);
    let rounded = (qty * factor).round() / factor;
    let formatted = format!("{:.prec$}", rounded, prec = decimals);
    (formatted, rounded)
}

/// Round and format a string quantity according to a market's allowed decimals.
pub fn sanitize_order_quantity_for_market(market: &str, qty: &str) -> (String, usize, f64) {
    let decimals = get_cached_market_decimals(market);
    if let Ok(val) = qty.parse::<f64>() {
        let (formatted, rounded) = round_qty_to_decimals(val, decimals);
        (formatted, decimals, rounded)
    } else {
        (qty.to_string(), decimals, 0.0)
    }
}

/// Determine whether an error string represents Real.xyz Error 121 (OrderQuantityHasTooManyDecimalPlaces).
pub fn is_decimal_precision_error(err_str: &str) -> bool {
    let lower = err_str.to_lowercase();
    lower.contains("121")
        || lower.contains("orderquantityhastoomanydecimalplaces")
        || lower.contains("too many decimal places")
        || lower.contains("decimal places")
}

/// Compute one fewer decimal place for retry upon receiving Error 121.
pub fn next_fewer_decimal_qty(market: &str, current_qty: &str, current_decimals: usize) -> Option<(String, usize)> {
    if current_decimals == 0 {
        return None;
    }
    let new_decimals = current_decimals - 1;
    if let Ok(val) = current_qty.parse::<f64>() {
        let (formatted, rounded) = round_qty_to_decimals(val, new_decimals);
        if rounded > 0.0 {
            set_cached_market_decimals(market, new_decimals);
            return Some((formatted, new_decimals));
        }
    }
    None
}

/// Round a price to a specified number of decimal places and format as string.
/// E.g. round_price_to_decimals(84500.55, 1) -> ("84500.6", 84500.6)
pub fn round_price_to_decimals(price: f64, decimals: usize) -> (String, f64) {
    let factor = 10_f64.powi(decimals as i32);
    let rounded = (price * factor).round() / factor;
    let formatted = format!("{:.prec$}", rounded, prec = decimals);
    (formatted, rounded)
}

/// Round and format a string price according to a market's allowed price decimals.
pub fn sanitize_order_price_for_market(market: &str, price: &str) -> (String, usize, f64) {
    let decimals = get_cached_market_price_decimals(market);
    let cleaned = price.trim().replace('$', "").replace(',', "");
    if let Ok(val) = cleaned.parse::<f64>() {
        let (formatted, rounded) = round_price_to_decimals(val, decimals);
        (formatted, decimals, rounded)
    } else {
        (price.to_string(), decimals, 0.0)
    }
}

/// Determine whether an error string represents Real.xyz Error 114 (OrderPriceHasTooManyDecimalPlaces).
pub fn is_price_decimal_error(err_str: &str) -> bool {
    let lower = err_str.to_lowercase();
    lower.contains("114")
        || lower.contains("orderpricehastoomanydecimalplaces")
        || lower.contains("price has too many decimal places")
}

/// Helper to get next fewer decimal place for price if Error 114 occurs.
pub fn next_fewer_decimal_price(
    market: &str,
    price: &str,
    current_decimals: usize,
) -> Option<(String, usize)> {
    if current_decimals == 0 {
        return None;
    }
    let new_decimals = current_decimals - 1;
    let cleaned = price.trim().replace('$', "").replace(',', "");
    if let Ok(val) = cleaned.parse::<f64>() {
        let (formatted, _) = round_price_to_decimals(val, new_decimals);
        set_cached_market_price_decimals(market, new_decimals);
        return Some((formatted, new_decimals));
    }
    None
}

// ─── Public data shapes returned to handlers ─────────────────────────────────

/// Information on a live market loaded dynamically from Real.xyz indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    pub market_id: String,
    pub symbol: String,        // e.g. "BTC-USDT"
    pub base_asset: String,    // e.g. "BTC"
    pub quote_asset: String,   // e.g. "USDT"
    pub short_name: String,    // e.g. "btc"
    pub mark_price: f64,
    pub index_price: f64,
    pub last_traded_price: Option<f64>,
    pub price_change_24h: Option<f64>,
    pub high_price_24h: Option<f64>,
    pub low_price_24h: Option<f64>,
    pub volume_24h: Option<f64>,
    pub turnover_24h: Option<f64>,
    pub perp_next_funding_rate: Option<f64>,
    pub perp_next_funding_time_ms: Option<u64>,
    #[serde(default = "default_market_qty_decimals")]
    pub qty_decimals: usize,
    #[serde(default = "default_market_lot_size")]
    pub lot_size: f64,
    #[serde(default = "default_market_price_decimals")]
    pub price_decimals: usize,
}

fn default_market_qty_decimals() -> usize {
    3
}

fn default_market_lot_size() -> f64 {
    0.0001
}

fn default_market_price_decimals() -> usize {
    1
}

#[derive(Debug, Clone)]
pub struct MarketsCache {
    pub markets: Vec<MarketInfo>,
    pub last_updated_secs: i64,
}

pub fn fallback_markets() -> Vec<MarketInfo> {
    vec![
        MarketInfo {
            market_id: BTC_MARKET.to_string(),
            symbol: "BTC-USDT".into(),
            base_asset: "BTC".into(),
            quote_asset: "USDT".into(),
            short_name: "btc".into(),
            mark_price: 0.0,
            index_price: 0.0,
            last_traded_price: None,
            price_change_24h: None,
            high_price_24h: None,
            low_price_24h: None,
            volume_24h: None,
            turnover_24h: None,
            perp_next_funding_rate: None,
            perp_next_funding_time_ms: None,
            qty_decimals: 4,
            lot_size: 0.0001,
            price_decimals: 1,
        },
        MarketInfo {
            market_id: ETH_MARKET.to_string(),
            symbol: "ETH-USDT".into(),
            base_asset: "ETH".into(),
            quote_asset: "USDT".into(),
            short_name: "eth".into(),
            mark_price: 0.0,
            index_price: 0.0,
            last_traded_price: None,
            price_change_24h: None,
            high_price_24h: None,
            low_price_24h: None,
            volume_24h: None,
            turnover_24h: None,
            perp_next_funding_rate: None,
            perp_next_funding_time_ms: None,
            qty_decimals: 3,
            lot_size: 0.001,
            price_decimals: 2,
        },
        MarketInfo {
            market_id: SOL_MARKET.to_string(),
            symbol: "SOL-USDT".into(),
            base_asset: "SOL".into(),
            quote_asset: "USDT".into(),
            short_name: "sol".into(),
            mark_price: 0.0,
            index_price: 0.0,
            last_traded_price: None,
            price_change_24h: None,
            high_price_24h: None,
            low_price_24h: None,
            volume_24h: None,
            turnover_24h: None,
            perp_next_funding_rate: None,
            perp_next_funding_time_ms: None,
            qty_decimals: 2,
            lot_size: 0.01,
            price_decimals: 3,
        },
        MarketInfo {
            market_id: "0x7fa781f5e1d1da7165cf96e5a6898235f55f2d812dd429d3feed69ff776dfa70".into(),
            symbol: "HYPE-USDT".into(),
            base_asset: "HYPE".into(),
            quote_asset: "USDT".into(),
            short_name: "hype".into(),
            mark_price: 0.0,
            index_price: 0.0,
            last_traded_price: None,
            price_change_24h: None,
            high_price_24h: None,
            low_price_24h: None,
            volume_24h: None,
            turnover_24h: None,
            perp_next_funding_rate: None,
            perp_next_funding_time_ms: None,
            qty_decimals: 2,
            lot_size: 0.01,
            price_decimals: 3,
        },
    ]
}

/// Summary of an account / engine health state.
#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub account_id: String,
    pub network: String,
    /// Engine chain id.
    pub chain_id: Option<String>,
}

/// Account lookup result containing both the primary Account ID and on-chain object ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountLookup {
    /// Account ID ("id" from indexer), used for placing orders and querying positions/balance.
    pub account_id: String,
    /// On-chain trading account object ID ("account_object_id" from indexer), kept if needed for balance/deposit reads.
    pub trading_account_object_id: Option<String>,
}

/// Balance and margin information from the Real.xyz indexer.
#[derive(Debug, Clone, Default)]
pub struct BalanceInfo {
    pub total_equity: f64,
    pub available_margin: f64,
}

/// Aggregated trading volume broken down by asset.
#[derive(Debug, Clone, Default)]
pub struct VolumeInfo {
    pub total: f64,
    pub btc: f64,
    pub eth: f64,
    pub sol: f64,
    pub by_market: HashMap<String, f64>,
}

/// An open perpetual position from the Real.xyz indexer.
#[derive(Debug, Clone, Default)]
pub struct OpenPosition {
    pub market_id: String,
    pub direction: String,
    pub size: f64,
    pub entry_price: Option<f64>,
    pub mark_price: Option<f64>,
    pub notional: f64,
    pub unrealized_pnl: f64,
    pub roe_pct: f64,
}

/// A completed trade record from account fills.
#[derive(Debug, Clone, Default)]
pub struct TradeHistoryRecord {
    pub trade_id: String,
    pub market_symbol: String,
    pub side: String,
    pub price: f64,
    pub size: f64,
    pub notional: f64,
    pub timestamp: i64,
}

/// Real-time market statistics including funding rate.
#[derive(Debug, Clone, Default)]
pub struct MarketStats {
    pub mark_price: f64,
    pub last_price: Option<f64>,
    pub high_24h: Option<f64>,
    pub low_24h: Option<f64>,
    pub price_change_24h: Option<f64>,
    pub funding_rate: Option<f64>,
    pub next_funding_time_ms: Option<u64>,
}

/// Result of a submitted order.
#[derive(Debug, Clone)]
pub struct OrderResult {
    pub success: bool,
    /// The engine's u128 order id, formatted as a string.
    pub order_id: Option<String>,
    pub client_order_id: String,
    pub side: String,
    pub quantity: String,
    pub price: String,
    pub market: String,
    /// Human-readable rejection reason from OrderError, if any.
    pub error_msg: Option<String>,
    pub fills: Vec<FillInfo>,
}

/// A single fill/trade that occurred when the order matched.
#[derive(Debug, Clone)]
pub struct FillInfo {
    pub trade_id: String,
    pub price: String,
    pub quantity: String,
}

/// Result of a cancel operation.
#[derive(Debug, Clone)]
pub struct CancelResult {
    pub success: bool,
    pub cancelled_ids: Vec<String>,
    pub count: usize,
}

// ─── Key & Address Derivation ────────────────────────────────────────────────
/// Validate an IOTA private key and derive the associated wallet address.
pub fn derive_address_from_key(key: &str) -> BotResult<String> {
    RawSigner::new(key)
        .map(|s| s.address())
        .map_err(|e| BotError::Trading(format!("Invalid private key: {e}")))
}

// ─── TradingClient ────────────────────────────────────────────────────────────

/// Thread-safe wrapper around `SigningClient` and Real.xyz Indexer REST API.
#[derive(Clone)]
pub struct TradingClient {
    client: Arc<Mutex<SigningClient>>,
    config: Arc<Config>,
    http: reqwest::Client,
    indexer_account_id: Arc<RwLock<Option<String>>>,
    markets_cache: Arc<std::sync::RwLock<MarketsCache>>,
}

impl TradingClient {
    /// Build a new `TradingClient` from the loaded config.
    pub async fn new(cfg: &Config) -> BotResult<Self> {
        let signer: Signer = cfg.private_key.as_str().into();
        let client = match cfg.network {
            Network::Testnet => SigningClient::new_testnet(signer),
            Network::Mainnet => SigningClient::new_mainnet(signer),
        }
        .map_err(|e| BotError::Trading(format!("Failed to build SigningClient: {e}")))?;

        info!("SigningClient created for {:?} network", cfg.network);
        let initial_markets = fallback_markets();
        update_cached_market_names(&initial_markets);
        update_cached_market_decimals(&initial_markets);

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            config: Arc::new(cfg.clone()),
            http: reqwest::Client::new(),
            indexer_account_id: Arc::new(RwLock::new(None)),
            markets_cache: Arc::new(std::sync::RwLock::new(MarketsCache {
                markets: initial_markets,
                last_updated_secs: 0,
            })),
        })
    }

    pub fn network(&self) -> &Network {
        &self.config.network
    }

    pub fn get_market_qty_decimals(&self, market_id_or_sym: &str) -> usize {
        get_cached_market_decimals(market_id_or_sym)
    }

    pub fn set_market_qty_decimals(&self, market_id_or_sym: &str, decimals: usize) {
        set_cached_market_decimals(market_id_or_sym, decimals);
    }

    /// Fetch dynamic list of active markets from Real.xyz Indexer API.
    pub async fn fetch_markets_from_indexer(&self) -> BotResult<Vec<MarketInfo>> {
        // Fetch live order rules from /api/v1/markets to extract size_decimal_places, lot_size & price_decimal_places
        let markets_url = format!("{}/api/v1/markets", self.config.network.indexer_url());
        let mut rules_map: HashMap<String, (usize, f64, usize)> = HashMap::new();
        if let Ok(resp) = self.http.get(&markets_url).send().await {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(arr) = val.get("data").and_then(|d| d.as_array()) {
                    for m in arr {
                        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let sym = m.get("symbol").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let base = sym.split(['-', '/']).next().unwrap_or("").to_string();
                        let rules = m.get("order_rules");
                        let dec = rules
                            .and_then(|r| r.get("size_decimal_places"))
                            .and_then(|v| v.as_str().and_then(|s| s.parse::<usize>().ok()).or_else(|| v.as_u64().map(|n| n as usize)));
                        let lot = rules
                            .and_then(|r| r.get("lot_size"))
                            .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()));
                        let price_dec = rules
                            .and_then(|r| r.get("price_decimal_places"))
                            .and_then(|v| v.as_str().and_then(|s| s.parse::<usize>().ok()).or_else(|| v.as_u64().map(|n| n as usize)))
                            .or_else(|| {
                                rules.and_then(|r| r.get("tick_size")).and_then(|v| {
                                    if let Some(s) = v.as_str() {
                                        if let Some(pos) = s.find('.') {
                                            return Some(s[pos + 1..].trim_end_matches('0').len());
                                        }
                                    } else if let Some(f) = v.as_f64() {
                                        if f > 0.0 && f < 1.0 {
                                            let s = format!("{:.8}", f);
                                            if let Some(pos) = s.find('.') {
                                                return Some(s[pos + 1..].trim_end_matches('0').len());
                                            }
                                        }
                                    }
                                    None
                                })
                            })
                            .unwrap_or_else(|| fallback_price_decimals(&base));

                        if let (Some(d), Some(l)) = (dec, lot) {
                            if !id.is_empty() {
                                rules_map.insert(id, (d, l, price_dec));
                            }
                            if !sym.is_empty() {
                                rules_map.insert(sym.clone(), (d, l, price_dec));
                            }
                            if !base.is_empty() {
                                rules_map.insert(base, (d, l, price_dec));
                            }
                        }
                    }
                }
            }
        }

        let url = format!("{}/api/v1/markets/tickers", self.config.network.indexer_url());
        let resp = self.http.get(&url).send().await
            .map_err(|e| BotError::Trading(format!("Failed to fetch markets: {e}")))?;

        let val = resp.json::<serde_json::Value>().await
            .map_err(|e| BotError::Trading(format!("Failed to parse markets JSON: {e}")))?;

        let items = val.get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| BotError::Trading("Invalid markets data array from indexer".into()))?;

        let mut markets = Vec::new();
        for item in items {
            let market_id = match item.get("market_id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => id.to_string(),
                _ => continue,
            };

            let symbol = item.get("symbol")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if symbol.is_empty() {
                continue;
            }

            let parts: Vec<&str> = symbol.split(|c| c == '-' || c == '/').collect();
            let base_asset = parts.first().unwrap_or(&symbol.as_str()).trim().to_uppercase();
            let quote_asset = parts.get(1).unwrap_or(&"USDT").trim().to_uppercase();
            let short_name = base_asset.to_lowercase();

            let mark_price = item.get("mark_price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("mark_price").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);

            let index_price = item.get("index_price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("index_price").and_then(|v| v.as_f64()))
                .unwrap_or(mark_price);

            let last_traded_price = item.get("last_traded_price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("last_traded_price").and_then(|v| v.as_f64()));

            let price_change_24h = item.get("price_change_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("price_change_24h").and_then(|v| v.as_f64()));

            let high_price_24h = item.get("high_price_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("high_price_24h").and_then(|v| v.as_f64()));

            let low_price_24h = item.get("low_price_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("low_price_24h").and_then(|v| v.as_f64()));

            let volume_24h = item.get("volume_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("volume_24h").and_then(|v| v.as_f64()));

            let turnover_24h = item.get("turnover_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("turnover_24h").and_then(|v| v.as_f64()));

            let perp_next_funding_rate = item.get("perp_next_funding_rate")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("perp_next_funding_rate").and_then(|v| v.as_f64()));

            let perp_next_funding_time_ms = item.get("perp_next_funding_time_ms")
                .and_then(|v| v.as_u64());

            let (qty_decimals, lot_size, price_decimals) = if let Some(&(d, l, p)) = rules_map.get(&market_id.to_lowercase())
                .or_else(|| rules_map.get(&symbol.to_lowercase()))
                .or_else(|| rules_map.get(&base_asset.to_lowercase()))
            {
                (d, l, p)
            } else {
                (parse_qty_decimals(item, &base_asset), fallback_market_lot_size(&base_asset), fallback_price_decimals(&base_asset))
            };

            markets.push(MarketInfo {
                market_id,
                symbol,
                base_asset,
                quote_asset,
                short_name,
                mark_price,
                index_price,
                last_traded_price,
                price_change_24h,
                high_price_24h,
                low_price_24h,
                volume_24h,
                turnover_24h,
                perp_next_funding_rate,
                perp_next_funding_time_ms,
                qty_decimals,
                lot_size,
                price_decimals,
            });
        }

        // Sort with BTC, ETH, SOL first if present, then alphabetical
        markets.sort_by(|a, b| {
            let order = |sym: &str| match sym {
                "BTC" => 0,
                "ETH" => 1,
                "SOL" => 2,
                _ => 3,
            };
            let ord_a = order(&a.base_asset);
            let ord_b = order(&b.base_asset);
            if ord_a != ord_b {
                ord_a.cmp(&ord_b)
            } else {
                a.base_asset.cmp(&b.base_asset)
            }
        });

        Ok(markets)
    }

    /// Retrieve markets list, refreshing from the indexer if the cache is expired (> 3 minutes) or empty.
    pub async fn get_markets(&self) -> Vec<MarketInfo> {
        let now = chrono::Utc::now().timestamp();
        {
            if let Ok(guard) = self.markets_cache.read() {
                if !guard.markets.is_empty() && (now - guard.last_updated_secs) < 180 {
                    return guard.markets.clone();
                }
            }
        }

        // Needs refresh
        match self.fetch_markets_from_indexer().await {
            Ok(fresh_markets) if !fresh_markets.is_empty() => {
                info!(count = fresh_markets.len(), "Refreshed markets list from Real.xyz indexer");
                update_cached_market_names(&fresh_markets);
                update_cached_market_decimals(&fresh_markets);
                update_cached_market_lot_sizes(&fresh_markets);
                update_cached_market_price_decimals(&fresh_markets);
                if let Ok(mut guard) = self.markets_cache.write() {
                    guard.markets = fresh_markets.clone();
                    guard.last_updated_secs = now;
                }
                fresh_markets
            }
            Ok(_) | Err(_) => {
                // Return cached if available, else fallbacks
                if let Ok(guard) = self.markets_cache.read() {
                    if !guard.markets.is_empty() {
                        return guard.markets.clone();
                    }
                }
                fallback_markets()
            }
        }
    }

    /// Retrieve the current cached markets list synchronously.
    pub fn get_cached_markets(&self) -> Vec<MarketInfo> {
        if let Ok(guard) = self.markets_cache.read() {
            if !guard.markets.is_empty() {
                return guard.markets.clone();
            }
        }
        fallback_markets()
    }

    /// Resolve market by short name (btc, eth, sol, hype), symbol (BTC-USDT), or 0x ID.
    pub fn resolve_market(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        if trimmed.starts_with("0x") {
            return Some(trimmed.to_string());
        }

        let markets = self.get_cached_markets();
        let input_lower = trimmed.to_lowercase();
        for m in &markets {
            if m.short_name == input_lower
                || m.base_asset.eq_ignore_ascii_case(trimmed)
                || m.symbol.eq_ignore_ascii_case(trimmed)
                || m.symbol.replace('-', "").eq_ignore_ascii_case(trimmed)
                || m.market_id.eq_ignore_ascii_case(trimmed)
            {
                return Some(m.market_id.clone());
            }
        }

        // Static fallbacks for resilience
        match input_lower.as_str() {
            "btc" | "btc-usdt" | "btcusdt" => Some(BTC_MARKET.to_string()),
            "eth" | "eth-usdt" | "ethusdt" => Some(ETH_MARKET.to_string()),
            "sol" | "sol-usdt" | "solusdt" => Some(SOL_MARKET.to_string()),
            "hype" | "hype-usdt" | "hypeusdt" => {
                Some("0x7fa781f5e1d1da7165cf96e5a6898235f55f2d812dd429d3feed69ff776dfa70".to_string())
            }
            _ => None,
        }
    }

    /// Resolve a market ID to its display asset name (e.g. "BTC", "ETH", "SOL", "HYPE").
    pub fn get_market_name(&self, market_id: &str) -> String {
        if let Some(name) = get_cached_market_name(market_id) {
            return name;
        }

        let markets = self.get_cached_markets();
        for m in &markets {
            if m.market_id.eq_ignore_ascii_case(market_id) {
                return m.base_asset.clone();
            }
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

    /// Lookup Account ID and on-chain trading account object ID by IOTA wallet address.
    pub async fn lookup_account_by_address(&self, address: &str) -> Option<AccountLookup> {
        let url = format!("{}/api/v1/accounts", self.config.network.indexer_url());
        if let Ok(resp) = self.http.get(&url).send().await {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(list) = val.get("data").and_then(|d| d.as_array()) {
                    for item in list {
                        if let (Some(addr), Some(oid), Some(id)) = (
                            item.get("address").and_then(|a| a.as_str()),
                            item.get("account_object_id").and_then(|o| o.as_str()),
                            item.get("id").and_then(|i| i.as_str()),
                        ) {
                            if addr.eq_ignore_ascii_case(address) {
                                return Some(AccountLookup {
                                    account_id: id.to_string(),
                                    trading_account_object_id: Some(oid.to_string()),
                                });
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Resolve indexer internal account UUID from any on-chain object ID or Account ID.
    pub async fn resolve_user_indexer_id(&self, account_identifier: &str) -> Option<String> {
        let url = format!("{}/api/v1/accounts", self.config.network.indexer_url());
        if let Ok(resp) = self.http.get(&url).send().await {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(list) = val.get("data").and_then(|d| d.as_array()) {
                    for item in list {
                        if let (Some(oid), Some(id)) = (
                            item.get("account_object_id").and_then(|o| o.as_str()),
                            item.get("id").and_then(|i| i.as_str()),
                        ) {
                            if oid.eq_ignore_ascii_case(account_identifier)
                                || id.eq_ignore_ascii_case(account_identifier)
                            {
                                return Some(id.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    // ─── Balance & Volume (via Real.xyz Indexer REST API) ────────────────────

    /// Resolve and cache the indexer internal account UUID from config account_id (handles object ID or Account ID).
    pub async fn resolve_indexer_account_id(&self) -> Option<String> {
        {
            let guard = self.indexer_account_id.read().await;
            if let Some(ref id) = *guard {
                return Some(id.clone());
            }
        }

        let url = format!("{}/api/v1/accounts", self.config.network.indexer_url());
        match self.http.get(&url).send().await {
            Ok(resp) => {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if let Some(list) = val.get("data").and_then(|d| d.as_array()) {
                        for item in list {
                            if let (Some(oid), Some(id)) = (
                                item.get("account_object_id").and_then(|o| o.as_str()),
                                item.get("id").and_then(|i| i.as_str()),
                            ) {
                                if oid.eq_ignore_ascii_case(&self.config.account_id)
                                    || id.eq_ignore_ascii_case(&self.config.account_id)
                                {
                                    let mut guard = self.indexer_account_id.write().await;
                                    *guard = Some(id.to_string());
                                    return Some(id.to_string());
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to query indexer accounts: {e}");
            }
        }

        None
    }

    /// Return the resolved Account ID for the configured default account.
    pub async fn get_effective_account_id(&self) -> String {
        if let Some(id) = self.resolve_indexer_account_id().await {
            return id;
        }
        self.config.account_id.clone()
    }

    /// Query the account balance and available margin for a user.
    pub async fn get_user_balance(
        &self,
        account_id: &str,
        indexer_id_opt: Option<&str>,
    ) -> BotResult<BalanceInfo> {
        let indexer_id = match indexer_id_opt {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => match self.resolve_user_indexer_id(account_id).await {
                Some(id) => id,
                None => account_id.to_string(),
            },
        };

        let url = format!(
            "{}/api/v1/accounts/{}",
            self.config.network.indexer_url(),
            indexer_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch balance: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse balance: {e}")))?;

        let data = val.get("data").unwrap_or(&val);
        let total_equity = data
            .get("equity")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        let available_margin = data
            .get("available_balance")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        Ok(BalanceInfo {
            total_equity,
            available_margin,
        })
    }

    /// Query default account balance.
    pub async fn get_account_balance(&self) -> BotResult<BalanceInfo> {
        self.get_user_balance(&self.config.account_id, None).await
    }

    /// Query total trading volume across markets for a specific user.
    pub async fn get_user_volume(
        &self,
        account_id: &str,
        indexer_id_opt: Option<&str>,
    ) -> BotResult<VolumeInfo> {
        let indexer_id = match indexer_id_opt {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => match self.resolve_user_indexer_id(account_id).await {
                Some(id) => id,
                None => account_id.to_string(),
            },
        };

        let url = format!(
            "{}/api/v1/accounts/{}/fills",
            self.config.network.indexer_url(),
            indexer_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch volume: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse volume fills: {e}")))?;

        let mut vol = VolumeInfo::default();
        if let Some(fills) = val.get("data").and_then(|d| d.as_array()) {
            for f in fills {
                let notional: f64 = if let Some(fv) = f
                    .get("filled_value")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    fv
                } else {
                    let p = f
                        .get("price")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let q = f
                        .get("volume")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    p * q
                };

                vol.total += notional;

                if let Some(m) = f.get("market_id").and_then(|v| v.as_str()) {
                    let sym = self.get_market_name(m);
                    *vol.by_market.entry(sym).or_insert(0.0) += notional;

                    if m.eq_ignore_ascii_case(BTC_MARKET) {
                        vol.btc += notional;
                    } else if m.eq_ignore_ascii_case(ETH_MARKET) {
                        vol.eth += notional;
                    } else if m.eq_ignore_ascii_case(SOL_MARKET) {
                        vol.sol += notional;
                    }
                }
            }
        }

        Ok(vol)
    }

    /// Query total trading volume across markets.
    pub async fn get_account_volume(&self) -> BotResult<VolumeInfo> {
        self.get_user_volume(&self.config.account_id, None).await
    }

    /// Fetch current mark price for a market from the indexer ticker.
    pub async fn get_market_mark_price(&self, market_id: &str) -> BotResult<f64> {
        let url = format!("{}/api/v1/markets/{}/ticker", self.config.network.indexer_url(), market_id);
        let resp = self.http.get(&url).send().await
            .map_err(|e| BotError::Trading(format!("Failed to fetch ticker: {e}")))?;

        let val = resp.json::<serde_json::Value>().await
            .map_err(|e| BotError::Trading(format!("Failed to parse ticker: {e}")))?;

        let data = val.get("data").unwrap_or(&val);

        let mark_price = data.get("mark_price")
            .or_else(|| data.get("mp"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| {
                data.get("last_traded_price")
                    .or_else(|| data.get("ltp"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
            })
            .or_else(|| data.get("mark_price").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);

        if mark_price > 0.0 {
            Ok(mark_price)
        } else {
            Err(BotError::Trading(format!("Price unavailable for market {market_id}")))
        }
    }

    /// Fetch open position for a given market from the indexer for a specific user.
    pub async fn get_user_open_position(
        &self,
        account_id: &str,
        indexer_id_opt: Option<&str>,
        market_id: &str,
    ) -> BotResult<Option<OpenPosition>> {
        let indexer_id = match indexer_id_opt {
            Some(id) if !id.is_empty() => Some(id.to_string()),
            _ => self.resolve_user_indexer_id(account_id).await.or_else(|| Some(account_id.to_string())),
        };

        let base = self.config.network.indexer_url();
        let url = if let Some(ref acc) = indexer_id {
            format!("{}/api/v1/positions/live?f[account_ids]={}&f[market_ids]={}", base, acc, market_id)
        } else {
            format!("{}/api/v1/positions/live?f[market_ids]={}", base, market_id)
        };

        let resp = self.http.get(&url).send().await
            .map_err(|e| BotError::Trading(format!("Failed to fetch positions: {e}")))?;

        let val = resp.json::<serde_json::Value>().await
            .map_err(|e| BotError::Trading(format!("Failed to parse positions: {e}")))?;

        if let Some(positions) = val.get("data").and_then(|d| d.as_array()) {
            for p in positions {
                let status = p.get("status").or_else(|| p.get("st")).and_then(|v| v.as_str()).unwrap_or("");
                if status.eq_ignore_ascii_case("closed") {
                    continue;
                }

                let m_id = p.get("market_id").or_else(|| p.get("m")).and_then(|v| v.as_str()).unwrap_or("");
                if !m_id.is_empty() && !m_id.eq_ignore_ascii_case(market_id) {
                    continue;
                }

                let size = p.get("size")
                    .or_else(|| p.get("s"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| p.get("size").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);

                if size <= 0.0 {
                    continue;
                }

                let direction = p.get("direction")
                    .or_else(|| p.get("d"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Long")
                    .to_string();

                let entry_price = p.get("average_entry_price")
                    .or_else(|| p.get("aep"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let mark_price = p.get("live_values")
                    .or_else(|| p.get("pv"))
                    .and_then(|v| v.get("mark_price").or_else(|| v.get("mp")))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let is_long = direction.eq_ignore_ascii_case("long") || direction.eq_ignore_ascii_case("buy");
                let notional = mark_price.map(|mp| mp * size).unwrap_or(0.0);
                let unrealized_pnl = match (entry_price, mark_price) {
                    (Some(ep), Some(mp)) => {
                        if is_long {
                            (mp - ep) * size
                        } else {
                            (ep - mp) * size
                        }
                    }
                    _ => 0.0,
                };
                let roe_pct = match entry_price {
                    Some(ep) if ep > 0.0 && size > 0.0 => (unrealized_pnl / (ep * size)) * 100.0,
                    _ => 0.0,
                };

                return Ok(Some(OpenPosition {
                    market_id: market_id.to_string(),
                    direction,
                    size,
                    entry_price,
                    mark_price,
                    notional,
                    unrealized_pnl,
                    roe_pct,
                }));
            }
        }

        Ok(None)
    }

    /// Fetch all open positions across all markets for a specific user.
    pub async fn get_user_all_open_positions(
        &self,
        account_id: &str,
        indexer_id_opt: Option<&str>,
    ) -> BotResult<Vec<OpenPosition>> {
        let indexer_id = match indexer_id_opt {
            Some(id) if !id.is_empty() => Some(id.to_string()),
            _ => self.resolve_user_indexer_id(account_id).await.or_else(|| Some(account_id.to_string())),
        };

        let base = self.config.network.indexer_url();
        let url = if let Some(ref acc) = indexer_id {
            format!("{}/api/v1/positions/live?f[account_ids]={}", base, acc)
        } else {
            format!("{}/api/v1/positions/live", base)
        };

        let resp = self.http.get(&url).send().await
            .map_err(|e| BotError::Trading(format!("Failed to fetch positions: {e}")))?;

        let val = resp.json::<serde_json::Value>().await
            .map_err(|e| BotError::Trading(format!("Failed to parse positions: {e}")))?;

        let mut results = Vec::new();
        if let Some(positions) = val.get("data").and_then(|d| d.as_array()) {
            for p in positions {
                let status = p.get("status").or_else(|| p.get("st")).and_then(|v| v.as_str()).unwrap_or("");
                if status.eq_ignore_ascii_case("closed") {
                    continue;
                }

                let market_id = p.get("market_id").or_else(|| p.get("m")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                if market_id.is_empty() {
                    continue;
                }

                let size = p.get("size")
                    .or_else(|| p.get("s"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| p.get("size").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);

                if size <= 0.0 {
                    continue;
                }

                let direction = p.get("direction")
                    .or_else(|| p.get("d"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Long")
                    .to_string();

                let entry_price = p.get("average_entry_price")
                    .or_else(|| p.get("aep"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let mark_price = p.get("live_values")
                    .or_else(|| p.get("pv"))
                    .and_then(|v| v.get("mark_price").or_else(|| v.get("mp")))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let is_long = direction.eq_ignore_ascii_case("long") || direction.eq_ignore_ascii_case("buy");
                let notional = mark_price.map(|mp| mp * size).unwrap_or(0.0);
                let unrealized_pnl = match (entry_price, mark_price) {
                    (Some(ep), Some(mp)) => {
                        if is_long {
                            (mp - ep) * size
                        } else {
                            (ep - mp) * size
                        }
                    }
                    _ => 0.0,
                };
                let roe_pct = match entry_price {
                    Some(ep) if ep > 0.0 && size > 0.0 => (unrealized_pnl / (ep * size)) * 100.0,
                    _ => 0.0,
                };

                results.push(OpenPosition {
                    market_id,
                    direction,
                    size,
                    entry_price,
                    mark_price,
                    notional,
                    unrealized_pnl,
                    roe_pct,
                });
            }
        }

        Ok(results)
    }

    /// Fetch comprehensive market statistics including real-time funding rate.
    pub async fn get_market_stats(&self, market_id: &str) -> BotResult<MarketStats> {
        let url = format!("{}/api/v1/markets/{}/ticker", self.config.network.indexer_url(), market_id);
        let resp = self.http.get(&url).send().await
            .map_err(|e| BotError::Trading(format!("Failed to fetch ticker: {e}")))?;

        let val = resp.json::<serde_json::Value>().await
            .map_err(|e| BotError::Trading(format!("Failed to parse ticker: {e}")))?;

        let data = val.get("data").unwrap_or(&val);

        let mark_price = data.get("mark_price")
            .or_else(|| data.get("mp"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("mark_price").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);

        let last_price = data.get("last_traded_price")
            .or_else(|| data.get("ltp"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("last_traded_price").and_then(|v| v.as_f64()));

        let high_24h = data.get("high_price_24h")
            .or_else(|| data.get("hp24h"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("high_price_24h").and_then(|v| v.as_f64()));

        let low_24h = data.get("low_price_24h")
            .or_else(|| data.get("lp24h"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("low_price_24h").and_then(|v| v.as_f64()));

        let price_change_24h = data.get("price_change_24h")
            .or_else(|| data.get("pc24h"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("price_change_24h").and_then(|v| v.as_f64()));

        let funding_rate = data.get("perp_next_funding_rate")
            .or_else(|| data.get("pnfr"))
            .or_else(|| data.get("funding_rate"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("perp_next_funding_rate").and_then(|v| v.as_f64()));

        let next_funding_time_ms = data.get("perp_next_funding_time_ms")
            .or_else(|| data.get("pnft_ms"))
            .and_then(|v| v.as_u64());

        Ok(MarketStats {
            mark_price,
            last_price,
            high_24h,
            low_24h,
            price_change_24h,
            funding_rate,
            next_funding_time_ms,
        })
    }

    /// Fetch the last N trades/fills for a user.
    pub async fn get_user_trade_history(
        &self,
        account_id: &str,
        indexer_id_opt: Option<&str>,
        limit: usize,
    ) -> BotResult<Vec<TradeHistoryRecord>> {
        let indexer_id = match indexer_id_opt {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => match self.resolve_user_indexer_id(account_id).await {
                Some(id) => id,
                None => account_id.to_string(),
            },
        };

        let url = format!(
            "{}/api/v1/accounts/{}/fills",
            self.config.network.indexer_url(),
            indexer_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch history: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse history: {e}")))?;

        let mut list = Vec::new();
        if let Some(fills) = val.get("data").and_then(|d| d.as_array()) {
            for f in fills {
                let trade_id = f.get("id").or_else(|| f.get("trade_id"))
                    .and_then(|v| v.as_str()).unwrap_or("").to_string();

                let market_id = f.get("market_id").and_then(|v| v.as_str()).unwrap_or("");
                let market_symbol = self.get_market_name(market_id);

                let side = f.get("side").or_else(|| f.get("direction"))
                    .and_then(|v| v.as_str()).unwrap_or("Trade").to_string();

                let price = f.get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let size = f.get("volume").or_else(|| f.get("size"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let notional = if let Some(fv) = f.get("filled_value")
                    .and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()) {
                    fv
                } else {
                    price * size
                };

                let timestamp = f.get("created_at_epoch_ms")
                    .or_else(|| f.get("timestamp_ms"))
                    .and_then(|v| v.as_i64())
                    .or_else(|| {
                        f.get("created_at")
                            .and_then(|v| v.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or(0);

                list.push(TradeHistoryRecord {
                    trade_id,
                    market_symbol,
                    side,
                    price,
                    size,
                    notional,
                    timestamp,
                });
            }
        }

        list.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        list.truncate(limit);
        Ok(list)
    }

    /// Calculate total trading volume aggregated across all registered bot users.
    pub async fn get_all_users_volume(&self, users: &[crate::users::StoredUser]) -> BotResult<f64> {
        let mut total = 0.0;
        for u in users {
            if let Ok(vol) = self.get_user_volume(&u.account_id, u.indexer_account_id.as_deref()).await {
                total += vol.total;
            }
        }
        Ok(total)
    }

    /// Fetch open position for a given market from the indexer.
    pub async fn get_open_position(&self, market_id: &str) -> BotResult<Option<OpenPosition>> {
        self.get_user_open_position(&self.config.account_id, None, market_id).await
    }

    // ─── Public Order Operations ────────────────────────────────────────────

    /// Fallback sanitization helper if market is not specified (defaults to 3 decimals).
    pub fn sanitize_order_quantity(qty: &str) -> String {
        if let Ok(val) = qty.parse::<f64>() {
            let (formatted, _) = round_qty_to_decimals(val, 3);
            formatted
        } else {
            qty.to_string()
        }
    }

    /// Place a market buy (long) order with immediate-or-cancel execution.
    pub async fn place_market_long(
        &self,
        market: &str,
        quantity: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let coid = Uuid::new_v4().to_string();
        let account_id = self.get_effective_account_id().await;
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(&account_id)
            .buy()
            .market_order()
            .quantity(&clean_qty)
            .immediate_or_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build market long order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;
        drop(client);

        let mut res = self.parse_order_result(report, "Buy (Market)", &clean_qty, "Market", market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying market buy with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .buy()
                    .market_order()
                    .quantity(&retry_qty)
                    .immediate_or_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Buy (Market)", &retry_qty, "Market", market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    /// Place a market sell (short) order with immediate-or-cancel execution.
    pub async fn place_market_short(
        &self,
        market: &str,
        quantity: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let coid = Uuid::new_v4().to_string();
        let account_id = self.get_effective_account_id().await;
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(&account_id)
            .sell()
            .market_order()
            .quantity(&clean_qty)
            .immediate_or_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build market short order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;
        drop(client);

        let mut res = self.parse_order_result(report, "Sell (Market)", &clean_qty, "Market", market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying market sell with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .sell()
                    .market_order()
                    .quantity(&retry_qty)
                    .immediate_or_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Sell (Market)", &retry_qty, "Market", market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    /// Place a reduce-only order to close an open position (market or limit).
    pub async fn place_close_order(
        &self,
        market: &str,
        side: &str,
        quantity: &str,
        price: Option<&str>,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let (clean_price_opt, price_decimals) = match price {
            Some(p) => {
                let (cp, pd, _) = sanitize_order_price_for_market(market, p);
                (Some(cp), pd)
            }
            None => (None, 0),
        };
        let coid = Uuid::new_v4().to_string();
        let account_id = self.get_effective_account_id().await;
        let mut builder = SubmitOrderRequest::builder();
        builder = builder.market(market).account(&account_id);

        if side.eq_ignore_ascii_case("buy") {
            builder = builder.buy();
        } else {
            builder = builder.sell();
        }

        if let Some(ref p) = clean_price_opt {
            builder = builder.limit_order(p).good_till_cancel();
        } else {
            builder = builder.market_order().immediate_or_cancel();
        }

        let order = builder
            .quantity(&clean_qty)
            .reduce_only(true)
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build close order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;
        drop(client);

        let price_label = clean_price_opt.as_deref().unwrap_or("Market");
        let mut res = self.parse_order_result(report, side, &clean_qty, price_label, market, &coid)?;

        if !res.success && clean_price_opt.is_some() && res.error_msg.as_deref().map_or(false, is_price_decimal_error) {
            let cur_p = clean_price_opt.as_deref().unwrap();
            if let Some((retry_price, new_p_dec)) = next_fewer_decimal_price(market, cur_p, price_decimals) {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying close order with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                let mut retry_builder = SubmitOrderRequest::builder();
                retry_builder = retry_builder.market(market).account(&account_id);
                if side.eq_ignore_ascii_case("buy") {
                    retry_builder = retry_builder.buy();
                } else {
                    retry_builder = retry_builder.sell();
                }
                retry_builder = retry_builder.limit_order(&retry_price).good_till_cancel();
                if let Ok(retry_order) = retry_builder
                    .quantity(&clean_qty)
                    .reduce_only(true)
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, side, &clean_qty, &retry_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying close order with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                let mut retry_builder = SubmitOrderRequest::builder();
                retry_builder = retry_builder.market(market).account(&account_id);
                if side.eq_ignore_ascii_case("buy") {
                    retry_builder = retry_builder.buy();
                } else {
                    retry_builder = retry_builder.sell();
                }
                if let Some(ref p) = clean_price_opt {
                    retry_builder = retry_builder.limit_order(p).good_till_cancel();
                } else {
                    retry_builder = retry_builder.market_order().immediate_or_cancel();
                }
                if let Ok(retry_order) = retry_builder
                    .quantity(&retry_qty)
                    .reduce_only(true)
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, side, &retry_qty, price_label, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    /// Place a limit buy (long) order.
    pub async fn place_limit_long(
        &self,
        market: &str,
        quantity: &str,
        price: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let (clean_price, price_decimals, _) = sanitize_order_price_for_market(market, price);
        let coid = Uuid::new_v4().to_string();
        let account_id = self.get_effective_account_id().await;
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(&account_id)
            .buy()
            .limit_order(&clean_price)
            .quantity(&clean_qty)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;
        drop(client);

        let mut res = self.parse_order_result(report, "Buy", &clean_qty, &clean_price, market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_price_decimal_error) {
            if let Some((retry_price, new_p_dec)) = next_fewer_decimal_price(market, &clean_price, price_decimals) {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying limit buy with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .buy()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Buy", &clean_qty, &retry_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying limit buy with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .buy()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Buy", &retry_qty, &clean_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    /// Place a limit sell (short) order.
    pub async fn place_limit_short(
        &self,
        market: &str,
        quantity: &str,
        price: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let (clean_price, price_decimals, _) = sanitize_order_price_for_market(market, price);
        let coid = Uuid::new_v4().to_string();
        let account_id = self.get_effective_account_id().await;
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(&account_id)
            .sell()
            .limit_order(&clean_price)
            .quantity(&clean_qty)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;
        drop(client);

        let mut res = self.parse_order_result(report, "Sell", &clean_qty, &clean_price, market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_price_decimal_error) {
            if let Some((retry_price, new_p_dec)) = next_fewer_decimal_price(market, &clean_price, price_decimals) {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying limit sell with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .sell()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Sell", &clean_qty, &retry_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying limit sell with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .sell()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        drop(client);
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Sell", &retry_qty, &clean_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    /// Cancel all open orders in a given market.
    pub async fn cancel_all_orders(&self, market: &str) -> BotResult<CancelResult> {
        let account_id = self.get_effective_account_id().await;
        let req = CancelOrderRequest::builder()
            .market(market)
            .account(&account_id)
            .build()
            .map_err(|e| BotError::Trading(format!("Build cancel: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .cancel_order(req, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("cancel_order: {e}")))?;
        drop(client);

        debug!("cancel_order report: {report:?}");
        Ok(self.parse_cancel_result(report))
    }

    /// Cancel a single order by its engine order id (u128 as decimal string).
    pub async fn cancel_order_by_id(
        &self,
        market: &str,
        order_id: u128,
    ) -> BotResult<CancelResult> {
        let account_id = self.get_effective_account_id().await;
        let req = CancelOrderRequest::builder()
            .market(market)
            .account(&account_id)
            .order_id(order_id)
            .build()
            .map_err(|e| BotError::Trading(format!("Build cancel: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .cancel_order(req, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("cancel_order: {e}")))?;
        drop(client);

        Ok(self.parse_cancel_result(report))
    }

    // ─── Per-User Order Operations (with on-demand decrypted keys) ───────────

    /// Create an ephemeral SigningClient for a user's decrypted private key.
    pub fn create_user_signing_client(&self, decrypted_key: &str) -> BotResult<SigningClient> {
        let signer: Signer = decrypted_key.into();
        match self.config.network {
            Network::Testnet => SigningClient::new_testnet(signer),
            Network::Mainnet => SigningClient::new_mainnet(signer),
        }
        .map_err(|e| BotError::Trading(format!("Failed to create user SigningClient: {e}")))
    }

    pub async fn place_user_market_long(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        quantity: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let coid = Uuid::new_v4().to_string();
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(account_id)
            .buy()
            .market_order()
            .quantity(&clean_qty)
            .immediate_or_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build market long order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;

        let mut res = self.parse_order_result(report, "Buy (Market)", &clean_qty, "Market", market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user market buy with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .buy()
                    .market_order()
                    .quantity(&retry_qty)
                    .immediate_or_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Buy (Market)", &retry_qty, "Market", market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    pub async fn place_user_market_short(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        quantity: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let coid = Uuid::new_v4().to_string();
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(account_id)
            .sell()
            .market_order()
            .quantity(&clean_qty)
            .immediate_or_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build market short order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;

        let mut res = self.parse_order_result(report, "Sell (Market)", &clean_qty, "Market", market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user market sell with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .sell()
                    .market_order()
                    .quantity(&retry_qty)
                    .immediate_or_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Sell (Market)", &retry_qty, "Market", market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    pub async fn place_user_limit_long(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        quantity: &str,
        price: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let (clean_price, price_decimals, _) = sanitize_order_price_for_market(market, price);
        let coid = Uuid::new_v4().to_string();
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(account_id)
            .buy()
            .limit_order(&clean_price)
            .quantity(&clean_qty)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;

        let mut res = self.parse_order_result(report, "Buy", &clean_qty, &clean_price, market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_price_decimal_error) {
            if let Some((retry_price, new_p_dec)) = next_fewer_decimal_price(market, &clean_price, price_decimals) {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying user limit buy with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .buy()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Buy", &clean_qty, &retry_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user limit buy with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .buy()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Buy", &retry_qty, &clean_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    pub async fn place_user_limit_short(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        quantity: &str,
        price: &str,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let (clean_price, price_decimals, _) = sanitize_order_price_for_market(market, price);
        let coid = Uuid::new_v4().to_string();
        let order = SubmitOrderRequest::builder()
            .market(market)
            .account(account_id)
            .sell()
            .limit_order(&clean_price)
            .quantity(&clean_qty)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;

        let mut res = self.parse_order_result(report, "Sell", &clean_qty, &clean_price, market, &coid)?;

        if !res.success && res.error_msg.as_deref().map_or(false, is_price_decimal_error) {
            if let Some((retry_price, new_p_dec)) = next_fewer_decimal_price(market, &clean_price, price_decimals) {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying user limit sell with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .sell()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Sell", &clean_qty, &retry_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user limit sell with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .sell()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, "Sell", &retry_qty, &clean_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    pub async fn place_user_close_order(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        side: &str,
        quantity: &str,
        price: Option<&str>,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let (clean_price_opt, price_decimals) = match price {
            Some(p) => {
                let (cp, pd, _) = sanitize_order_price_for_market(market, p);
                (Some(cp), pd)
            }
            None => (None, 0),
        };
        let coid = Uuid::new_v4().to_string();
        let mut builder = SubmitOrderRequest::builder();
        builder = builder.market(market).account(account_id);

        if side.eq_ignore_ascii_case("buy") {
            builder = builder.buy();
        } else {
            builder = builder.sell();
        }

        if let Some(ref p) = clean_price_opt {
            builder = builder.limit_order(p).good_till_cancel();
        } else {
            builder = builder.market_order().immediate_or_cancel();
        }

        let order = builder
            .quantity(&clean_qty)
            .reduce_only(true)
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build close order: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("submit_order: {e}")))?;

        let price_label = clean_price_opt.as_deref().unwrap_or("Market");
        let mut res = self.parse_order_result(report, side, &clean_qty, price_label, market, &coid)?;

        if !res.success && clean_price_opt.is_some() && res.error_msg.as_deref().map_or(false, is_price_decimal_error) {
            let cur_p = clean_price_opt.as_deref().unwrap();
            if let Some((retry_price, new_p_dec)) = next_fewer_decimal_price(market, cur_p, price_decimals) {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying user close order with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                let mut retry_builder = SubmitOrderRequest::builder();
                retry_builder = retry_builder.market(market).account(account_id);
                if side.eq_ignore_ascii_case("buy") {
                    retry_builder = retry_builder.buy();
                } else {
                    retry_builder = retry_builder.sell();
                }
                retry_builder = retry_builder.limit_order(&retry_price).good_till_cancel();
                if let Ok(retry_order) = retry_builder
                    .quantity(&clean_qty)
                    .reduce_only(true)
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, side, &clean_qty, &retry_price, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success && res.error_msg.as_deref().map_or(false, is_decimal_precision_error) {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals) {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user close order with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                let mut retry_builder = SubmitOrderRequest::builder();
                retry_builder = retry_builder.market(market).account(account_id);
                if side.eq_ignore_ascii_case("buy") {
                    retry_builder = retry_builder.buy();
                } else {
                    retry_builder = retry_builder.sell();
                }
                if let Some(ref p) = clean_price_opt {
                    retry_builder = retry_builder.limit_order(p).good_till_cancel();
                } else {
                    retry_builder = retry_builder.market_order().immediate_or_cancel();
                }
                if let Ok(retry_order) = retry_builder
                    .quantity(&retry_qty)
                    .reduce_only(true)
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    if let Ok(retry_rep) = client.submit_order(retry_order, Some(retry_req)).await {
                        if let Ok(retry_res) = self.parse_order_result(retry_rep, side, &retry_qty, price_label, market, &retry_coid) {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    pub async fn cancel_user_all_orders(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
    ) -> BotResult<CancelResult> {
        let req = CancelOrderRequest::builder()
            .market(market)
            .account(account_id)
            .build()
            .map_err(|e| BotError::Trading(format!("Build cancel: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .cancel_order(req, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("cancel_order: {e}")))?;
        drop(client);

        Ok(self.parse_cancel_result(report))
    }

    pub async fn cancel_user_order_by_id(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        order_id: u128,
    ) -> BotResult<CancelResult> {
        let req = CancelOrderRequest::builder()
            .market(market)
            .account(account_id)
            .order_id(order_id)
            .build()
            .map_err(|e| BotError::Trading(format!("Build cancel: {e}")))?;

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .cancel_order(req, Some(req_id))
            .await
            .map_err(|e| BotError::Trading(format!("cancel_order: {e}")))?;
        drop(client);

        Ok(self.parse_cancel_result(report))
    }

    // ─── Private helpers ─────────────────────────────────────────────────────

    fn parse_order_result(
        &self,
        report: TransactionReport,
        side: &str,
        quantity: &str,
        price: &str,
        market: &str,
        client_order_id: &str,
    ) -> BotResult<OrderResult> {
        if !report.success {
            let err_msg = report
                .errors
                .as_ref()
                .map(|map| {
                    map.values()
                        .map(|e| format!("{:?}", e))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_else(|| "unknown error".into());

            warn!("Order transaction rejected: {err_msg}");
            return Ok(OrderResult {
                success: false,
                order_id: None,
                client_order_id: client_order_id.to_string(),
                side: side.to_string(),
                quantity: quantity.to_string(),
                price: price.to_string(),
                market: market.to_string(),
                error_msg: Some(err_msg),
                fills: vec![],
            });
        }

        for receipt in &report.receipts {
            if let Report::SubmitOrderReport { order, trades } = receipt {
                let error_msg = order.error.as_ref().map(|e| format!("{e:?}"));
                let order_id = Some(order.id.to_string());

                let fills = trades
                    .iter()
                    .map(|t| FillInfo {
                        trade_id: t.id.to_string(),
                        price: t.price.to_string(),
                        quantity: t.volume.to_string(),
                    })
                    .collect();

                let accepted = error_msg.is_none();
                if !accepted {
                    warn!("Order placed but engine rejected: {:?}", error_msg);
                }

                return Ok(OrderResult {
                    success: accepted,
                    order_id,
                    client_order_id: client_order_id.to_string(),
                    side: side.to_string(),
                    quantity: quantity.to_string(),
                    price: price.to_string(),
                    market: market.to_string(),
                    error_msg,
                    fills,
                });
            }
        }

        warn!("No SubmitOrderReport receipt in successful transaction");
        Ok(OrderResult {
            success: report.success,
            order_id: None,
            client_order_id: client_order_id.to_string(),
            side: side.to_string(),
            quantity: quantity.to_string(),
            price: price.to_string(),
            market: market.to_string(),
            error_msg: None,
            fills: vec![],
        })
    }

    fn parse_cancel_result(&self, report: TransactionReport) -> CancelResult {
        if !report.success {
            return CancelResult {
                success: false,
                cancelled_ids: vec![],
                count: 0,
            };
        }

        for receipt in &report.receipts {
            if let Report::CancelOrderReport { orders } = receipt {
                let ids: Vec<String> =
                    orders.order_ids.iter().map(|id| id.to_string()).collect();
                let count = ids.len();
                return CancelResult {
                    success: true,
                    cancelled_ids: ids,
                    count,
                };
            }
        }

        CancelResult {
            success: report.success,
            cancelled_ids: vec![],
            count: 0,
        }
    }
}
