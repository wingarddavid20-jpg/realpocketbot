//! All Real.xyz trading logic lives here, cleanly separated from Telegram.
//!
//! `TradingClient` wraps the SDK's `SigningClient` and exposes high-level
//! async methods for every bot command. Handlers in `bot/` call only these
//! methods and never touch the SDK types directly.

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use sdk::{
    crypto::{RawSigner, Signer},
    types::{
        CancelOrderRequest, OrderStatus, OrderType, Report, SubmitOrderRequest, TimeInForce,
        TradeExecuted, TransactionReport,
    },
    SigningClient,
};

use crate::{
    bot::state::{BTC_MARKET, ETH_MARKET, HYPE_MARKET, SOL_MARKET, XAUT_MARKET},
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

/// Global map for instantaneous market_id / symbol / base_asset -> max allowed leverage.
static CACHED_MARKET_MAX_LEVERAGE: std::sync::RwLock<Option<HashMap<String, f64>>> =
    std::sync::RwLock::new(None);

/// Fallback max leverage per market (e.g. BTC 40x, ETH 25x, SOL 20x, HYPE 20x, XAUT 20x, default 20x).
pub fn fallback_max_leverage(identifier: &str) -> f64 {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") || clean.eq_ignore_ascii_case(BTC_MARKET) {
        40.0
    } else if clean.starts_with("ETH") || clean.eq_ignore_ascii_case(ETH_MARKET) {
        25.0
    } else if clean.starts_with("SOL") || clean.eq_ignore_ascii_case(SOL_MARKET) {
        20.0
    } else if clean.starts_with("HYPE") || clean.eq_ignore_ascii_case(HYPE_MARKET) {
        20.0
    } else if clean.starts_with("XAUT") || clean.eq_ignore_ascii_case(XAUT_MARKET) {
        20.0
    } else {
        20.0
    }
}

/// Fallback precision map matching Real.xyz engine order rules (size_decimal_places):
/// BTC → 4 decimals (lot size 0.0001), ETH → 3 decimals (lot size 0.001),
/// SOL → 2 decimals (lot size 0.01), HYPE → 2 decimals (lot size 0.01),
/// XAUT → 3 decimals (lot size 0.001), DOGE/XRP → 0 decimals, default → 2 decimals.
pub fn fallback_qty_decimals(identifier: &str) -> usize {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") || clean.eq_ignore_ascii_case(BTC_MARKET) {
        4
    } else if clean.starts_with("ETH") || clean.eq_ignore_ascii_case(ETH_MARKET) {
        3
    } else if clean.starts_with("SOL") || clean.eq_ignore_ascii_case(SOL_MARKET) {
        2
    } else if clean.starts_with("HYPE") || clean.eq_ignore_ascii_case(HYPE_MARKET) {
        2
    } else if clean.starts_with("XAUT") || clean.eq_ignore_ascii_case(XAUT_MARKET) {
        3
    } else if clean.starts_with("DOGE") || clean.starts_with("XRP") {
        0
    } else if clean.starts_with("BNB") || clean.starts_with("ZEC") {
        2
    } else {
        2
    }
}

pub fn fallback_market_lot_size(identifier: &str) -> f64 {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") || clean.eq_ignore_ascii_case(BTC_MARKET) {
        0.0001
    } else if clean.starts_with("ETH") || clean.eq_ignore_ascii_case(ETH_MARKET) {
        0.001
    } else if clean.starts_with("SOL") || clean.eq_ignore_ascii_case(SOL_MARKET) {
        0.01
    } else if clean.starts_with("HYPE") || clean.eq_ignore_ascii_case(HYPE_MARKET) {
        0.01
    } else if clean.starts_with("XAUT") || clean.eq_ignore_ascii_case(XAUT_MARKET) {
        0.001
    } else if clean.starts_with("DOGE") {
        10.0
    } else if clean.starts_with("XRP") {
        1.0
    } else {
        0.01
    }
}

pub fn fallback_price_decimals(identifier: &str) -> usize {
    let clean = identifier.trim().to_uppercase();
    if clean.starts_with("BTC") || clean.eq_ignore_ascii_case(BTC_MARKET) {
        1
    } else if clean.starts_with("ETH") || clean.eq_ignore_ascii_case(ETH_MARKET) {
        2
    } else if clean.starts_with("SOL") || clean.eq_ignore_ascii_case(SOL_MARKET) {
        3
    } else if clean.starts_with("HYPE") || clean.eq_ignore_ascii_case(HYPE_MARKET) {
        3
    } else if clean.starts_with("XAUT") || clean.eq_ignore_ascii_case(XAUT_MARKET) {
        2
    } else {
        2
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

pub fn update_cached_market_max_leverage(markets: &[MarketInfo]) {
    let mut map = HashMap::new();
    for m in markets {
        let lev = m.max_leverage;
        map.insert(m.market_id.to_lowercase(), lev);
        map.insert(m.base_asset.to_lowercase(), lev);
        map.insert(m.symbol.to_lowercase(), lev);
        map.insert(m.short_name.to_lowercase(), lev);
    }
    if let Ok(mut guard) = CACHED_MARKET_MAX_LEVERAGE.write() {
        if let Some(ref mut existing) = *guard {
            for (k, v) in map {
                existing.insert(k, v);
            }
        } else {
            *guard = Some(map);
        }
    }
}

pub fn set_cached_market_max_leverage(market_id_or_sym: &str, max_lev: f64) {
    let key = market_id_or_sym.trim().to_lowercase();
    if let Ok(mut guard) = CACHED_MARKET_MAX_LEVERAGE.write() {
        if let Some(ref mut map) = *guard {
            map.insert(key, max_lev);
        } else {
            let mut map = HashMap::new();
            map.insert(key, max_lev);
            *guard = Some(map);
        }
    }
}

pub fn get_cached_market_max_leverage(identifier: &str) -> f64 {
    let key = identifier.trim().to_lowercase();
    if let Ok(guard) = CACHED_MARKET_MAX_LEVERAGE.read() {
        if let Some(ref map) = *guard {
            if let Some(&lev) = map.get(&key) {
                if lev > 0.0 {
                    return lev;
                }
            }
        }
    }
    if let Some(base_name) = get_cached_market_name(identifier) {
        if let Ok(guard) = CACHED_MARKET_MAX_LEVERAGE.read() {
            if let Some(ref map) = *guard {
                if let Some(&lev) = map.get(&base_name.to_lowercase()) {
                    if lev > 0.0 {
                        return lev;
                    }
                }
            }
        }
        return fallback_max_leverage(&base_name);
    }
    fallback_max_leverage(identifier)
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

/// Floor a quantity to a specified number of decimal places and format as string.
/// Unlike round_qty_to_decimals, this never rounds up, guaranteeing that the
/// required margin never exceeds the intended USD allocation.
pub fn floor_qty_to_decimals(qty: f64, decimals: usize) -> (String, f64) {
    let factor = 10_f64.powi(decimals as i32);
    let floored = (qty * factor).floor() / factor;
    let formatted = format!("{:.prec$}", floored, prec = decimals);
    (formatted, floored)
}

/// Floor a price to the market's price increment.
pub fn floor_price_to_decimals(price: f64, decimals: usize) -> (String, f64) {
    let factor = 10_f64.powi(decimals as i32);
    let floored = (price * factor).floor() / factor;
    let formatted = format!("{:.prec$}", floored, prec = decimals);
    (formatted, floored)
}

/// Ceil a price to the market's price increment.
pub fn ceil_price_to_decimals(price: f64, decimals: usize) -> (String, f64) {
    let factor = 10_f64.powi(decimals as i32);
    let ceiled = (price * factor).ceil() / factor;
    let formatted = format!("{:.prec$}", ceiled, prec = decimals);
    (formatted, ceiled)
}

/// Extra room past the price level that covers the order, so a small book move still fills.
const DEPTH_PRICE_BUFFER: f64 = 0.0015;
/// Used when `/depth` is missing. Tight on purpose: a wide cap is charged as extra margin.
const FALLBACK_MARK_OFFSET: f64 = 0.003;
/// Default taker fee when the indexer payload has not been loaded yet (BTC/ETH live schedule).
const DEFAULT_TAKER_FEE: f64 = 0.00019;
const DEFAULT_TRADING_BOUND: f64 = 0.01;

/// One price level from `/api/v1/markets/{id}/depth`.
#[derive(Debug, Clone, Copy)]
pub struct BookLevel {
    pub price: f64,
    pub qty: f64,
}

/// Cached risk schedule for one market, used by sizing and the protective price.
#[derive(Debug, Clone, Copy)]
pub struct MarketRiskParams {
    pub imr: f64,
    pub taker_fee: f64,
    pub trading_bound: f64,
    pub lot_size: f64,
    pub qty_decimals: usize,
    pub price_decimals: usize,
}

/// Quote produced by [`TradingClient::resolve_marketable_qty`].
#[derive(Debug, Clone)]
pub struct MarketableQuote {
    pub quantity: String,
    pub quantity_f: f64,
    pub price: String,
    pub price_f: f64,
    pub mark: f64,
}

/// Initial-margin rate actually used for sizing.
///
/// The caller's leverage cannot exceed `1/imr`. A 100x request on a 40x market
/// (imr 0.025) stays at 0.025; a 10x request uses 0.10.
pub fn capped_imr(market_imr: f64, leverage: f64) -> f64 {
    let market_imr = if market_imr > 0.0 { market_imr } else { 0.05 };
    if leverage > 0.0 {
        (1.0 / leverage).max(market_imr)
    } else {
        market_imr
    }
}

/// Margin the engine reserves per one unit of base.
///
/// Buys above the mark pay `max(0, P - mark)` on top of initial margin and the
/// taker fee. Sells below the mark pay `max(0, mark - P)`.
pub fn marketable_margin_per_unit(
    price: f64,
    mark: f64,
    is_buy: bool,
    imr: f64,
    taker_fee: f64,
) -> f64 {
    if !(price > 0.0) {
        return 0.0;
    }
    let slippage = if is_buy {
        (price - mark).max(0.0)
    } else {
        (mark - price).max(0.0)
    };
    price * imr.max(0.0) + price * taker_fee.max(0.0) + slippage
}

/// Floor `qty` to a whole number of lots, then to the market's decimal scale.
pub fn floor_qty_to_lot(qty: f64, lot_size: f64, decimals: usize) -> (String, f64) {
    if !(qty > 0.0) {
        return floor_qty_to_decimals(0.0, decimals);
    }
    let stepped = if lot_size > 0.0 {
        let steps = ((qty / lot_size) + 1e-8).floor();
        (steps * lot_size).max(0.0)
    } else {
        qty
    };
    floor_qty_to_decimals(stepped, decimals)
}

/// Largest lot-aligned quantity whose margin fits in `budget`.
///
/// `qty = budget / (P*imr + P*taker_fee + slippage)`, where slippage is
/// `max(0, P-mark)` for a buy and `max(0, mark-P)` for a sell.
pub fn size_marketable_qty(
    budget: f64,
    price: f64,
    mark: f64,
    is_buy: bool,
    imr: f64,
    taker_fee: f64,
    lot_size: f64,
    decimals: usize,
) -> (String, f64) {
    let per_unit = marketable_margin_per_unit(price, mark, is_buy, imr, taker_fee);
    if !(budget > 0.0) || !(per_unit > 0.0) {
        return floor_qty_to_decimals(0.0, decimals);
    }
    floor_qty_to_lot(budget / per_unit, lot_size, decimals)
}

/// Protective IOC price.
///
/// Walks `levels` (asks for a buy, bids for a sell) until `qty` is covered,
/// adds a 0.15% buffer, and caps the result strictly inside `trading_bound`.
/// An empty book falls back to mark ± 0.3%.
pub fn protective_slippage_price(
    mark: f64,
    is_buy: bool,
    price_decimals: usize,
    levels: &[BookLevel],
    qty: f64,
    trading_bound: f64,
) -> (String, f64) {
    if !(mark > 0.0) {
        return ("0".to_string(), 0.0);
    }
    let mut book: Vec<BookLevel> = levels
        .iter()
        .copied()
        .filter(|lvl| lvl.price > 0.0 && lvl.qty > 0.0)
        .collect();
    book.sort_by(|a, b| {
        if is_buy {
            a.price
                .partial_cmp(&b.price)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            b.price
                .partial_cmp(&a.price)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let mut covered = 0.0;
    let mut cover_price = None;
    for lvl in &book {
        covered += lvl.qty;
        cover_price = Some(lvl.price);
        if !(qty > 0.0) || covered + 1e-12 >= qty {
            break;
        }
    }

    let raw = match cover_price {
        Some(px) if is_buy => px * (1.0 + DEPTH_PRICE_BUFFER),
        Some(px) => px * (1.0 - DEPTH_PRICE_BUFFER),
        None if is_buy => mark * (1.0 + FALLBACK_MARK_OFFSET),
        None => mark * (1.0 - FALLBACK_MARK_OFFSET),
    };

    let bound = if trading_bound > 0.0 && trading_bound < 1.0 {
        trading_bound
    } else {
        DEFAULT_TRADING_BOUND
    };
    let step = 10_f64.powi(-(price_decimals as i32));
    let max_buy = (mark * (1.0 + bound) - step).max(mark);
    let min_sell = (mark * (1.0 - bound) + step).max(step);
    let clamped = if is_buy {
        raw.min(max_buy).max(step)
    } else {
        raw.max(min_sell).max(step)
    };

    let (mut formatted, mut value) = if is_buy {
        ceil_price_to_decimals(clamped, price_decimals)
    } else {
        floor_price_to_decimals(clamped, price_decimals)
    };
    if is_buy && value > max_buy + 1e-9 {
        let (floored, floored_val) = floor_price_to_decimals(max_buy, price_decimals);
        formatted = floored;
        value = floored_val;
    } else if !is_buy && value + 1e-9 < min_sell {
        let (ceiled, ceiled_val) = ceil_price_to_decimals(min_sell, price_decimals);
        formatted = ceiled;
        value = ceiled_val;
    }
    if value <= 0.0 {
        return ("0".to_string(), 0.0);
    }
    (formatted, value)
}

/// USD margin to commit for a percentage of *spendable* margin.
///
/// 100% keeps a 10% buffer, and at least $0.50 when spendable margin is above $1,
/// so taker fees and the trading-bound slippage check still fit in what is free.
/// Returns `(usd, is_max)`.
pub fn margin_budget_for_percentage(spendable: f64, pct: f64) -> (f64, bool) {
    if !(spendable > 0.0) {
        return (0.0, pct >= 99.0);
    }
    if pct >= 99.0 {
        let safe = if spendable > 1.0 {
            (spendable - 0.50).min(spendable * 0.90)
        } else {
            spendable * 0.80
        };
        (((safe.max(0.0) * 100.0).floor()) / 100.0, true)
    } else {
        (((spendable * (pct / 100.0) * 100.0).floor()) / 100.0, false)
    }
}

/// Largest USD margin a manual amount may use: the 100% budget, buffer included.
pub fn max_margin_budget(spendable: f64) -> f64 {
    margin_budget_for_percentage(spendable, 100.0).0
}

/// Reduce-only close quantity that never exceeds `position_size`.
///
/// `round_qty_to_decimals` rounds half away from zero, so a 100% close of a size
/// that is not an exact lot (for example 0.00325 at 4 decimals → 0.0033) is
/// larger than the position and the engine rejects it with
/// ReduceOnlyOrderExceedsPosition. Flooring leaves at most one lot of dust.
pub fn format_close_qty(position_size: f64, pct: f64, decimals: usize) -> Option<(String, f64)> {
    if !(position_size > 0.0) {
        return None;
    }
    let target = if pct >= 99.9 {
        position_size
    } else {
        position_size * (pct / 100.0).clamp(0.0, 1.0)
    };
    let (formatted, floored) = floor_qty_to_decimals(target.max(0.0), decimals);
    if floored > 0.0 && floored <= position_size + 1e-12 {
        return Some((formatted, floored.min(position_size)));
    }
    if decimals > 0 && floored > position_size {
        let step = 10_f64.powi(-(decimals as i32));
        let stepped = (floored - step).max(0.0);
        let (formatted, floored) = floor_qty_to_decimals(stepped, decimals);
        if floored > 0.0 && floored <= position_size + 1e-12 {
            return Some((formatted, floored));
        }
    }
    None
}

/// One lot below `qty`, used when a reduce-only close is still larger than the position.
pub fn step_down_qty(qty: &str, decimals: usize) -> Option<(String, f64)> {
    let val = qty.parse::<f64>().ok()?;
    if decimals > 18 {
        return None;
    }
    let step = 10_f64.powi(-(decimals as i32));
    let next = val - step;
    if next <= 0.0 {
        return None;
    }
    let (formatted, floored) = floor_qty_to_decimals(next, decimals);
    if floored <= 0.0 || formatted == qty {
        return None;
    }
    Some((formatted, floored))
}

/// True when the engine rejected a reduce-only order for being larger than the position.
pub fn is_reduce_only_exceeds_position(err_str: &str) -> bool {
    let lower = err_str.to_lowercase();
    lower.contains("reduceonlyorderexceedsposition")
        || lower.contains("reduce only order exceeds")
        || lower.contains("exceeds position")
        || lower.contains("exceeds the position")
        || (lower.contains("reduceonly") && lower.contains("exceed"))
        || (lower.contains("reduce only") && lower.contains("exceed"))
}

/// True when the order price was outside the market trading bound.
pub fn is_price_bound_error(err_str: &str) -> bool {
    let lower = err_str.to_lowercase();
    lower.contains("priceboundstopped")
        || lower.contains("price bound")
        || lower.contains("trading bound")
        || lower.contains("outside the bound")
}

/// Floor and format a string quantity according to a market's allowed decimals.
pub fn sanitize_order_quantity_for_market(market: &str, qty: &str) -> (String, usize, f64) {
    let decimals = get_cached_market_decimals(market);
    if let Ok(val) = qty.parse::<f64>() {
        let (formatted, floored) = floor_qty_to_decimals(val, decimals);
        (formatted, decimals, floored)
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
pub fn next_fewer_decimal_qty(
    market: &str,
    current_qty: &str,
    current_decimals: usize,
) -> Option<(String, usize)> {
    if current_decimals == 0 {
        return None;
    }
    let new_decimals = current_decimals - 1;
    if let Ok(val) = current_qty.parse::<f64>() {
        let (formatted, floored) = floor_qty_to_decimals(val, new_decimals);
        if floored > 0.0 {
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
/// Preserves the user's exact price if it already conforms to allowed decimals.
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

/// Determine whether an error string represents an Insufficient Margin error from Real.xyz matching engine.
pub fn is_insufficient_margin_error(err_str: &str) -> bool {
    let lower = err_str.to_lowercase();
    lower.contains("insufficientmargin")
        || lower.contains("insufficient margin")
        || lower.contains("not enough margin")
        || lower.contains("insufficient balance")
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

#[derive(Debug, Clone, Copy)]
struct ParsedMarketRules {
    qty_decimals: usize,
    lot_size: f64,
    price_decimals: usize,
    max_leverage: f64,
    imr: f64,
    taker_fee: f64,
    trading_bound: f64,
}

/// Information on a live market loaded dynamically from Real.xyz indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    pub market_id: String,
    pub symbol: String,      // e.g. "BTC-USDT"
    pub base_asset: String,  // e.g. "BTC"
    pub quote_asset: String, // e.g. "USDT"
    pub short_name: String,  // e.g. "btc"
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
    #[serde(default = "default_market_max_leverage")]
    pub max_leverage: f64,
    /// Initial margin rate (`risk_engine.imr`). Leverage cannot exceed `1/imr`.
    #[serde(default = "default_market_imr")]
    pub imr: f64,
    /// Taker fee rate charged on notional (`fees.taker_rates[0]`).
    #[serde(default = "default_market_taker_fee")]
    pub taker_fee: f64,
    /// Max distance from the mark the engine will accept (`order_rules.trading_bound`).
    #[serde(default = "default_market_trading_bound")]
    pub trading_bound: f64,
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

fn default_market_max_leverage() -> f64 {
    20.0
}

fn default_market_imr() -> f64 {
    0.05
}

fn default_market_taker_fee() -> f64 {
    DEFAULT_TAKER_FEE
}

fn default_market_trading_bound() -> f64 {
    DEFAULT_TRADING_BOUND
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
            max_leverage: 40.0,
            imr: 0.025,
            taker_fee: DEFAULT_TAKER_FEE,
            trading_bound: DEFAULT_TRADING_BOUND,
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
            max_leverage: 25.0,
            imr: 0.04,
            taker_fee: DEFAULT_TAKER_FEE,
            trading_bound: DEFAULT_TRADING_BOUND,
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
            max_leverage: 20.0,
            imr: 0.05,
            taker_fee: DEFAULT_TAKER_FEE,
            trading_bound: DEFAULT_TRADING_BOUND,
        },
        MarketInfo {
            market_id: HYPE_MARKET.to_string(),
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
            max_leverage: 20.0,
            imr: 0.05,
            taker_fee: DEFAULT_TAKER_FEE,
            trading_bound: DEFAULT_TRADING_BOUND,
        },
        MarketInfo {
            market_id: XAUT_MARKET.to_string(),
            symbol: "XAUT-USDT".into(),
            base_asset: "XAUT".into(),
            quote_asset: "USDT".into(),
            short_name: "xaut".into(),
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
            max_leverage: 20.0,
            imr: 0.05,
            taker_fee: DEFAULT_TAKER_FEE,
            trading_bound: DEFAULT_TRADING_BOUND,
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
    /// Indexer `available_balance`. On a current indexer this already excludes
    /// margin locked in positions and resting orders. Older or partial payloads
    /// have been seen to leave order margin inside this number.
    pub available_margin: f64,
    /// Margin reserved by resting orders (`margin_allocated_orders`).
    pub margin_allocated_orders: f64,
    /// Margin reserved by open positions (cross + isolated).
    pub margin_allocated_positions: f64,
}

impl BalanceInfo {
    /// Margin that can collateralize a new order.
    ///
    /// Takes the smaller of the indexer's available balance and
    /// `equity - position margin - order margin`. When `available_balance`
    /// already excludes locked margin the two sides match. When it still
    /// includes a resting order's margin (the 100% BTC case that sized off
    /// ~$7.56 while $3 was locked), the equity identity is the smaller number
    /// and that is what 100% sizes against.
    pub fn spendable_margin(&self) -> f64 {
        let locked = (self.margin_allocated_orders + self.margin_allocated_positions).max(0.0);
        let derived = self.total_equity - locked;
        if self.available_margin > 0.0 && derived > 0.0 {
            self.available_margin.min(derived)
        } else if derived > 0.0 {
            derived
        } else {
            self.available_margin.max(0.0)
        }
    }
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| value.as_f64())
        .or_else(|| value.as_i64().map(|n| n as f64))
        .or_else(|| value.as_u64().map(|n| n as f64))
}

fn json_field_f64(data: &serde_json::Value, keys: &[&str]) -> f64 {
    for key in keys {
        if let Some(value) = data.get(*key) {
            if let Some(n) = json_f64(value) {
                return n;
            }
        }
    }
    0.0
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

/// An open/unfilled limit order from the Real.xyz indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenLimitOrder {
    pub order_id: u128,
    pub order_id_str: String,
    pub market_id: String,
    pub market_symbol: String,
    pub side: String,
    pub price: f64,
    pub quantity: f64,
    pub remaining_quantity: f64,
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

/// Indexer view of one order, keyed by `client_order_id`.
#[derive(Debug, Clone)]
struct IndexerOrderSnap {
    status: String,
    error: Option<String>,
    filled_qty: String,
    filled_qty_f: f64,
    avg_price: String,
    order_id: String,
}

pub fn order_was_not_filled(err: &str) -> bool {
    is_not_filled_message(err)
}

fn is_not_filled_message(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.starts_with("not filled") || lower.contains("did not trade") || lower.contains("no fill")
}

fn indexer_status_is_terminal(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "filled"
            | "cancelled"
            | "canceled"
            | "expired"
            | "killed"
            | "rejected"
            | "liquidationcancelled"
            | "closed"
    )
}

fn json_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else if let Some(n) = value.as_i64() {
        Some(n.to_string())
    } else if let Some(n) = value.as_u64() {
        Some(n.to_string())
    } else if value.is_object() {
        value
            .get("message")
            .or_else(|| value.get("code"))
            .or_else(|| value.get("error"))
            .and_then(json_text)
            .or_else(|| Some(value.to_string()))
    } else {
        None
    }
}

fn order_matches_client_id(order: &serde_json::Value, client_order_id: &str) -> bool {
    let raw = order
        .get("client_order_id")
        .or_else(|| order.get("ci"))
        .and_then(json_text);
    match raw {
        Some(id) => id.eq_ignore_ascii_case(client_order_id),
        None => false,
    }
}

/// Pull the order with `client_order_id` out of an indexer orders payload.
fn indexer_order_from_json(
    val: &serde_json::Value,
    client_order_id: &str,
) -> Option<IndexerOrderSnap> {
    let orders = val
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| val.as_array())
        .or_else(|| val.get("orders").and_then(|o| o.as_array()))?;
    let order = if orders.len() == 1 {
        orders.first()?
    } else {
        orders
            .iter()
            .find(|o| order_matches_client_id(o, client_order_id))?
    };
    // A one-element page is only ours when the id matches or the field is absent
    // (some payloads echo the filter without repeating the id).
    if orders.len() == 1 && !order_matches_client_id(order, client_order_id) {
        let echoed = order
            .get("client_order_id")
            .or_else(|| order.get("ci"))
            .and_then(json_text);
        if echoed.is_some() {
            return None;
        }
    }
    let status = order
        .get("status")
        .or_else(|| order.get("st"))
        .and_then(json_text)
        .unwrap_or_else(|| "Active".to_string());
    let error = order
        .get("error")
        .or_else(|| order.get("er"))
        .and_then(json_text);
    let filled_qty = order
        .get("filled_quantity")
        .or_else(|| order.get("fq"))
        .or_else(|| order.get("filled_size"))
        .and_then(json_text)
        .unwrap_or_else(|| "0".to_string());
    let filled_qty_f = filled_qty.parse::<f64>().unwrap_or(0.0);
    let avg_price = order
        .get("average_filled_price")
        .or_else(|| order.get("afp"))
        .or_else(|| order.get("price"))
        .or_else(|| order.get("p"))
        .and_then(json_text)
        .unwrap_or_default();
    let order_id = order
        .get("id")
        .or_else(|| order.get("i"))
        .or_else(|| order.get("order_id"))
        .and_then(json_text)
        .unwrap_or_default();
    Some(IndexerOrderSnap {
        status,
        error,
        filled_qty,
        filled_qty_f,
        avg_price,
        order_id,
    })
}

/// Fold an indexer snapshot into the submit result.
///
/// Rejected (and an IOC/FOK that ends with no fill) becomes `success = false`
/// so the existing InsufficientMargin / decimal retries can run. A positive
/// fill is copied onto the result when the receipt itself had none.
fn apply_indexer_confirmation(res: &mut OrderResult, snap: &IndexerOrderSnap, immediate: bool) {
    if !snap.order_id.is_empty() && res.order_id.is_none() {
        res.order_id = Some(snap.order_id.clone());
    }
    if snap.filled_qty_f > 0.0 && res.fills.is_empty() {
        let price = if snap.avg_price.is_empty() {
            res.price.clone()
        } else {
            snap.avg_price.clone()
        };
        res.fills.push(FillInfo {
            trade_id: if snap.order_id.is_empty() {
                res.client_order_id.clone()
            } else {
                snap.order_id.clone()
            },
            price,
            quantity: snap.filled_qty.clone(),
        });
    }

    let status = snap.status.to_ascii_lowercase();
    let rejected = status == "rejected" || status == "killed";
    let cancelled_empty = matches!(
        status.as_str(),
        "cancelled" | "canceled" | "expired" | "liquidationcancelled" | "closed"
    ) && snap.filled_qty_f <= 0.0;
    if rejected || (immediate && cancelled_empty) {
        res.success = false;
        res.fills.clear();
        res.error_msg = snap
            .error
            .clone()
            .filter(|err| !err.is_empty())
            .or_else(|| {
                if rejected {
                    Some(snap.status.clone())
                } else {
                    Some(format!("Not filled ({})", snap.status))
                }
            });
        return;
    }
    if status == "filled" || snap.filled_qty_f > 0.0 {
        res.success = true;
        res.error_msg = None;
    }
}

fn interpret_submit_report(
    report: &TransactionReport,
    side: &str,
    quantity: &str,
    price: &str,
    market: &str,
    client_order_id: &str,
) -> (OrderResult, bool, Option<String>) {
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
        let res = OrderResult {
            success: false,
            order_id: None,
            client_order_id: client_order_id.to_string(),
            side: side.to_string(),
            quantity: quantity.to_string(),
            price: price.to_string(),
            market: market.to_string(),
            error_msg: Some(err_msg),
            fills: vec![],
        };
        return (res, false, None);
    }

    for receipt in &report.receipts {
        if let Report::SubmitOrderReport { order, trades } = receipt {
            let mut fills: Vec<FillInfo> = trades
                .iter()
                .map(|t: &TradeExecuted| FillInfo {
                    trade_id: t.id.to_string(),
                    price: t.price.to_string(),
                    quantity: t.volume.to_string(),
                })
                .collect();
            if fills.is_empty() && decimal_is_positive(&order.filled_quantity.to_string()) {
                fills.push(FillInfo {
                    trade_id: order.id.to_string(),
                    price: order
                        .price
                        .as_ref()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| price.to_string()),
                    quantity: order.filled_quantity.to_string(),
                });
            }

            let immediate = matches!(order.tif, TimeInForce::Ioc | TimeInForce::Fok);
            let status_rejected =
                matches!(order.status, OrderStatus::Rejected | OrderStatus::Killed);
            let unfilled_immediate = immediate
                && fills.is_empty()
                && matches!(
                    order.status,
                    OrderStatus::Cancelled
                        | OrderStatus::Expired
                        | OrderStatus::Killed
                        | OrderStatus::Rejected
                        | OrderStatus::LiquidationCancelled
                );
            let error_msg = order.error.as_ref().map(|e| format!("{e:?}")).or_else(|| {
                if status_rejected {
                    Some(format!("{:?}", order.status))
                } else if unfilled_immediate {
                    Some(format!("Not filled ({:?})", order.status))
                } else {
                    None
                }
            });
            let success = error_msg.is_none();
            if !success {
                warn!(
                    client_order_id,
                    status = ?order.status,
                    error = ?error_msg,
                    "order receipt was not an acceptance"
                );
            }
            let res = OrderResult {
                success,
                order_id: Some(order.id.to_string()),
                client_order_id: client_order_id.to_string(),
                side: side.to_string(),
                quantity: quantity.to_string(),
                price: price.to_string(),
                market: market.to_string(),
                error_msg,
                fills,
            };
            return (res, immediate, Some(order.account.to_string()));
        }
    }

    warn!("No SubmitOrderReport receipt in successful transaction");
    let res = OrderResult {
        success: report.success,
        order_id: None,
        client_order_id: client_order_id.to_string(),
        side: side.to_string(),
        quantity: quantity.to_string(),
        price: price.to_string(),
        market: market.to_string(),
        error_msg: None,
        fills: vec![],
    };
    (res, false, None)
}

fn decimal_is_positive(raw: &str) -> bool {
    raw.parse::<f64>().unwrap_or(0.0) > 0.0
}

fn log_order_outcome(res: &OrderResult) {
    if res.success {
        info!(
            market = %res.market,
            side = %res.side,
            quantity = %res.quantity,
            price = %res.price,
            client_order_id = %res.client_order_id,
            order_id = ?res.order_id,
            fills = res.fills.len(),
            "order result: success"
        );
    } else {
        error!(
            market = %res.market,
            side = %res.side,
            quantity = %res.quantity,
            price = %res.price,
            client_order_id = %res.client_order_id,
            order_id = ?res.order_id,
            error = ?res.error_msg,
            "order result: failed"
        );
    }
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
        update_cached_market_lot_sizes(&initial_markets);
        update_cached_market_price_decimals(&initial_markets);
        update_cached_market_max_leverage(&initial_markets);

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .connect_timeout(Duration::from_secs(3))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            config: Arc::new(cfg.clone()),
            http,
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

    /// Retrieve maximum allowed leverage for a market synchronously from cache.
    pub fn get_market_max_leverage(&self, market_id_or_sym: &str) -> f64 {
        get_cached_market_max_leverage(market_id_or_sym)
    }

    /// Retrieve maximum allowed leverage for a market asynchronously, refreshing from indexer if needed.
    pub async fn get_market_max_leverage_async(&self, market_id_or_sym: &str) -> f64 {
        let cached = get_cached_market_max_leverage(market_id_or_sym);
        if cached == 20.0 {
            let _ = self.get_markets().await;
            get_cached_market_max_leverage(market_id_or_sym)
        } else {
            cached
        }
    }

    /// Fetch dynamic list of active markets from Real.xyz Indexer API.
    pub async fn fetch_markets_from_indexer(&self) -> BotResult<Vec<MarketInfo>> {
        // Fetch live order rules from /api/v1/markets to extract size_decimal_places, lot_size & price_decimal_places
        let markets_url = format!("{}/api/v1/markets", self.config.network.indexer_url());
        let mut rules_map: HashMap<String, ParsedMarketRules> = HashMap::new();
        if let Ok(resp) = self.http.get(&markets_url).send().await {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(arr) = val.get("data").and_then(|d| d.as_array()) {
                    for m in arr {
                        let id = m
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let sym = m
                            .get("symbol")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let base = sym.split(['-', '/']).next().unwrap_or("").to_string();
                        let rules = m.get("order_rules");
                        let dec = rules
                            .and_then(|r| r.get("size_decimal_places"))
                            .and_then(|v| {
                                v.as_str()
                                    .and_then(|s| s.parse::<usize>().ok())
                                    .or_else(|| v.as_u64().map(|n| n as usize))
                            });
                        let lot = rules.and_then(|r| r.get("lot_size")).and_then(|v| {
                            v.as_str()
                                .and_then(|s| s.parse::<f64>().ok())
                                .or_else(|| v.as_f64())
                        });
                        let price_dec = rules
                            .and_then(|r| r.get("price_decimal_places"))
                            .and_then(|v| {
                                v.as_str()
                                    .and_then(|s| s.parse::<usize>().ok())
                                    .or_else(|| v.as_u64().map(|n| n as usize))
                            })
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
                                                return Some(
                                                    s[pos + 1..].trim_end_matches('0').len(),
                                                );
                                            }
                                        }
                                    }
                                    None
                                })
                            })
                            .unwrap_or_else(|| fallback_price_decimals(&base));

                        let max_lev = m
                            .get("risk_engine")
                            .and_then(|re| re.get("max_leverage"))
                            .and_then(|v| {
                                if let Some(s) = v.as_str() {
                                    s.parse::<f64>().ok()
                                } else if let Some(n) = v.as_f64() {
                                    Some(n)
                                } else {
                                    None
                                }
                            })
                            .or_else(|| {
                                m.get("risk_engine")
                                    .and_then(|re| re.get("imr"))
                                    .and_then(|v| {
                                        if let Some(s) = v.as_str() {
                                            s.parse::<f64>().ok()
                                        } else if let Some(n) = v.as_f64() {
                                            Some(n)
                                        } else {
                                            None
                                        }
                                    })
                                    .and_then(|imr| {
                                        if imr > 0.0 {
                                            Some((1.0 / imr).round())
                                        } else {
                                            None
                                        }
                                    })
                            })
                            .unwrap_or_else(|| fallback_max_leverage(&base));

                        let imr = m
                            .get("risk_engine")
                            .and_then(|re| re.get("imr"))
                            .and_then(json_f64)
                            .filter(|v| *v > 0.0)
                            .unwrap_or_else(|| {
                                if max_lev > 0.0 {
                                    1.0 / max_lev
                                } else {
                                    default_market_imr()
                                }
                            });
                        let taker_fee = m
                            .get("fees")
                            .and_then(|fees| fees.get("taker_rates"))
                            .and_then(|rates| {
                                rates
                                    .as_array()
                                    .and_then(|arr| arr.iter().find_map(json_f64))
                                    .or_else(|| json_f64(rates))
                            })
                            .filter(|v| *v >= 0.0)
                            .unwrap_or(DEFAULT_TAKER_FEE);
                        let trading_bound = rules
                            .and_then(|r| r.get("trading_bound"))
                            .and_then(json_f64)
                            .filter(|v| *v > 0.0 && *v < 1.0)
                            .unwrap_or(DEFAULT_TRADING_BOUND);

                        if let (Some(d), Some(l)) = (dec, lot) {
                            let parsed = ParsedMarketRules {
                                qty_decimals: d,
                                lot_size: l,
                                price_decimals: price_dec,
                                max_leverage: max_lev,
                                imr,
                                taker_fee,
                                trading_bound,
                            };
                            if !id.is_empty() {
                                rules_map.insert(id, parsed);
                            }
                            if !sym.is_empty() {
                                rules_map.insert(sym.clone(), parsed);
                            }
                            if !base.is_empty() {
                                rules_map.insert(base, parsed);
                            }
                        }
                    }
                }
            }
        }

        let url = format!(
            "{}/api/v1/markets/tickers",
            self.config.network.indexer_url()
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch markets: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse markets JSON: {e}")))?;

        let items = val
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| BotError::Trading("Invalid markets data array from indexer".into()))?;

        let mut markets = Vec::new();
        for item in items {
            let market_id = match item.get("market_id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => id.to_string(),
                _ => continue,
            };

            let symbol = item
                .get("symbol")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if symbol.is_empty() {
                continue;
            }

            let parts: Vec<&str> = symbol.split(|c| c == '-' || c == '/').collect();
            let base_asset = parts
                .first()
                .unwrap_or(&symbol.as_str())
                .trim()
                .to_uppercase();
            let quote_asset = parts.get(1).unwrap_or(&"USDT").trim().to_uppercase();
            let short_name = base_asset.to_lowercase();

            let mark_price = item
                .get("mark_price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("mark_price").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);

            let index_price = item
                .get("index_price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("index_price").and_then(|v| v.as_f64()))
                .unwrap_or(mark_price);

            let last_traded_price = item
                .get("last_traded_price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("last_traded_price").and_then(|v| v.as_f64()));

            let price_change_24h = item
                .get("price_change_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("price_change_24h").and_then(|v| v.as_f64()));

            let high_price_24h = item
                .get("high_price_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("high_price_24h").and_then(|v| v.as_f64()));

            let low_price_24h = item
                .get("low_price_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("low_price_24h").and_then(|v| v.as_f64()));

            let volume_24h = item
                .get("volume_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("volume_24h").and_then(|v| v.as_f64()));

            let turnover_24h = item
                .get("turnover_24h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("turnover_24h").and_then(|v| v.as_f64()));

            let perp_next_funding_rate = item
                .get("perp_next_funding_rate")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| item.get("perp_next_funding_rate").and_then(|v| v.as_f64()));

            let perp_next_funding_time_ms = item
                .get("perp_next_funding_time_ms")
                .and_then(|v| v.as_u64());

            let rules = rules_map
                .get(&market_id.to_lowercase())
                .or_else(|| rules_map.get(&symbol.to_lowercase()))
                .or_else(|| rules_map.get(&base_asset.to_lowercase()))
                .copied();
            let qty_decimals = rules
                .map(|r| r.qty_decimals)
                .unwrap_or_else(|| parse_qty_decimals(item, &base_asset));
            let lot_size = rules
                .map(|r| r.lot_size)
                .unwrap_or_else(|| fallback_market_lot_size(&base_asset));
            let price_decimals = rules
                .map(|r| r.price_decimals)
                .unwrap_or_else(|| fallback_price_decimals(&base_asset));
            let max_leverage = rules
                .map(|r| r.max_leverage)
                .unwrap_or_else(|| fallback_max_leverage(&base_asset));
            let imr = rules.map(|r| r.imr).unwrap_or_else(|| {
                if max_leverage > 0.0 {
                    1.0 / max_leverage
                } else {
                    default_market_imr()
                }
            });
            let taker_fee = rules.map(|r| r.taker_fee).unwrap_or(DEFAULT_TAKER_FEE);
            let trading_bound = rules
                .map(|r| r.trading_bound)
                .unwrap_or(DEFAULT_TRADING_BOUND);

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
                max_leverage,
                imr,
                taker_fee,
                trading_bound,
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
                info!(
                    count = fresh_markets.len(),
                    "Refreshed markets list from Real.xyz indexer"
                );
                update_cached_market_names(&fresh_markets);
                update_cached_market_decimals(&fresh_markets);
                update_cached_market_lot_sizes(&fresh_markets);
                update_cached_market_price_decimals(&fresh_markets);
                update_cached_market_max_leverage(&fresh_markets);
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
            "hype" | "hype-usdt" | "hypeusdt" => Some(
                "0x7fa781f5e1d1da7165cf96e5a6898235f55f2d812dd429d3feed69ff776dfa70".to_string(),
            ),
            _ => None,
        }
    }

    /// Alias for `resolve_market`.
    pub fn get_market_id(&self, input: &str) -> Option<String> {
        self.resolve_market(input)
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
        } else if market_id.eq_ignore_ascii_case(
            "0x7fa781f5e1d1da7165cf96e5a6898235f55f2d812dd429d3feed69ff776dfa70",
        ) {
            "HYPE".into()
        } else if market_id.len() > 10 {
            format!("{}…{}", &market_id[..6], &market_id[market_id.len() - 4..])
        } else {
            market_id.to_string()
        }
    }

    /// Lookup Account ID and on-chain trading account object ID by IOTA wallet address.
    pub async fn lookup_account_by_address(&self, address: &str) -> Option<AccountLookup> {
        let url = format!(
            "{}/api/v1/accounts?f[address]={}",
            self.config.network.indexer_url(),
            address
        );
        if let Ok(Ok(resp)) =
            tokio::time::timeout(Duration::from_secs(4), self.http.get(&url).send()).await
        {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                let list_opt = val
                    .get("data")
                    .and_then(|d| d.as_array())
                    .or_else(|| val.as_array());
                if let Some(list) = list_opt {
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

    /// Resolve indexer internal account UUID from any on-chain object ID, Account ID, or Address.
    pub async fn resolve_user_indexer_id(&self, account_identifier: &str) -> Option<String> {
        let trimmed = account_identifier.trim();
        if trimmed.is_empty() {
            return None;
        }

        // 1. Try f[account_object_id]
        let filtered_url = format!(
            "{}/api/v1/accounts?f[account_object_id]={}",
            self.config.network.indexer_url(),
            trimmed
        );
        if let Ok(Ok(resp)) =
            tokio::time::timeout(Duration::from_secs(3), self.http.get(&filtered_url).send()).await
        {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                let list_opt = val
                    .get("data")
                    .and_then(|d| d.as_array())
                    .or_else(|| val.as_array());
                if let Some(list) = list_opt {
                    for item in list {
                        if let (Some(oid), Some(id)) = (
                            item.get("account_object_id").and_then(|o| o.as_str()),
                            item.get("id").and_then(|i| i.as_str()),
                        ) {
                            if oid.eq_ignore_ascii_case(trimmed) || id.eq_ignore_ascii_case(trimmed)
                            {
                                return Some(id.to_string());
                            }
                        }
                    }
                }
            }
        }

        // 2. Try direct /api/v1/accounts/{id}
        let direct_url = format!(
            "{}/api/v1/accounts/{}",
            self.config.network.indexer_url(),
            trimmed
        );
        if let Ok(Ok(resp)) =
            tokio::time::timeout(Duration::from_secs(3), self.http.get(&direct_url).send()).await
        {
            if resp.status().is_success() {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    let acc = val.get("data").unwrap_or(&val);
                    if let Some(id) = acc.get("id").and_then(|i| i.as_str()) {
                        return Some(id.to_string());
                    }
                }
            }
        }

        // 3. Try f[address] in case an address was passed as identifier
        let addr_url = format!(
            "{}/api/v1/accounts?f[address]={}",
            self.config.network.indexer_url(),
            trimmed
        );
        if let Ok(Ok(resp)) =
            tokio::time::timeout(Duration::from_secs(3), self.http.get(&addr_url).send()).await
        {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                let list_opt = val
                    .get("data")
                    .and_then(|d| d.as_array())
                    .or_else(|| val.as_array());
                if let Some(list) = list_opt {
                    for item in list {
                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                            return Some(id.to_string());
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

        if let Some(id) = self.resolve_user_indexer_id(&self.config.account_id).await {
            let mut guard = self.indexer_account_id.write().await;
            *guard = Some(id.clone());
            return Some(id);
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
        // Full names plus the indexer's compact aliases (`e`, `ab`, `mao`, `macc`, `maic`).
        let total_equity = json_field_f64(data, &["equity", "e"]);
        let available_margin = json_field_f64(data, &["available_balance", "ab"]);
        let margin_allocated_orders = json_field_f64(data, &["margin_allocated_orders", "mao"]);
        let margin_cross = json_field_f64(data, &["margin_allocated_cross_contracts", "macc"]);
        let margin_isolated =
            json_field_f64(data, &["margin_allocated_isolated_contracts", "maic"]);

        let balance = BalanceInfo {
            total_equity,
            available_margin,
            margin_allocated_orders,
            margin_allocated_positions: margin_cross + margin_isolated,
        };
        info!(
            account_id = %indexer_id,
            equity = balance.total_equity,
            available_balance = balance.available_margin,
            margin_allocated_orders = balance.margin_allocated_orders,
            margin_allocated_positions = balance.margin_allocated_positions,
            spendable = balance.spendable_margin(),
            "account margin snapshot"
        );
        Ok(balance)
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
        let url = format!(
            "{}/api/v1/markets/{}/ticker",
            self.config.network.indexer_url(),
            market_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch ticker: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse ticker: {e}")))?;

        let data = val.get("data").unwrap_or(&val);

        let mark_price = data
            .get("mark_price")
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
            Err(BotError::Trading(format!(
                "Price unavailable for market {market_id}"
            )))
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
            _ => self
                .resolve_user_indexer_id(account_id)
                .await
                .or_else(|| Some(account_id.to_string())),
        };

        let base = self.config.network.indexer_url();
        let url = if let Some(ref acc) = indexer_id {
            format!(
                "{}/api/v1/positions/live?f[account_ids]={}&f[market_ids]={}",
                base, acc, market_id
            )
        } else {
            format!("{}/api/v1/positions/live?f[market_ids]={}", base, market_id)
        };

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch positions: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse positions: {e}")))?;

        if let Some(positions) = val.get("data").and_then(|d| d.as_array()) {
            for p in positions {
                let status = p
                    .get("status")
                    .or_else(|| p.get("st"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if status.eq_ignore_ascii_case("closed") {
                    continue;
                }

                let m_id = p
                    .get("market_id")
                    .or_else(|| p.get("m"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !m_id.is_empty() && !m_id.eq_ignore_ascii_case(market_id) {
                    continue;
                }

                let size = p
                    .get("size")
                    .or_else(|| p.get("s"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| p.get("size").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);

                if size <= 0.0 {
                    continue;
                }

                let direction = p
                    .get("direction")
                    .or_else(|| p.get("d"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Long")
                    .to_string();

                let entry_price = p
                    .get("average_entry_price")
                    .or_else(|| p.get("aep"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let mark_price = p
                    .get("live_values")
                    .or_else(|| p.get("pv"))
                    .and_then(|v| v.get("mark_price").or_else(|| v.get("mp")))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let is_long =
                    direction.eq_ignore_ascii_case("long") || direction.eq_ignore_ascii_case("buy");
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
            _ => self
                .resolve_user_indexer_id(account_id)
                .await
                .or_else(|| Some(account_id.to_string())),
        };

        let base = self.config.network.indexer_url();
        let url = if let Some(ref acc) = indexer_id {
            format!("{}/api/v1/positions/live?f[account_ids]={}", base, acc)
        } else {
            format!("{}/api/v1/positions/live", base)
        };

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch positions: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse positions: {e}")))?;

        let mut results = Vec::new();
        if let Some(positions) = val.get("data").and_then(|d| d.as_array()) {
            for p in positions {
                let status = p
                    .get("status")
                    .or_else(|| p.get("st"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if status.eq_ignore_ascii_case("closed") {
                    continue;
                }

                let market_id = p
                    .get("market_id")
                    .or_else(|| p.get("m"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if market_id.is_empty() {
                    continue;
                }

                let size = p
                    .get("size")
                    .or_else(|| p.get("s"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| p.get("size").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);

                if size <= 0.0 {
                    continue;
                }

                let direction = p
                    .get("direction")
                    .or_else(|| p.get("d"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Long")
                    .to_string();

                let entry_price = p
                    .get("average_entry_price")
                    .or_else(|| p.get("aep"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let mark_price = p
                    .get("live_values")
                    .or_else(|| p.get("pv"))
                    .and_then(|v| v.get("mark_price").or_else(|| v.get("mp")))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());

                let is_long =
                    direction.eq_ignore_ascii_case("long") || direction.eq_ignore_ascii_case("buy");
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
        let url = format!(
            "{}/api/v1/markets/{}/ticker",
            self.config.network.indexer_url(),
            market_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to fetch ticker: {e}")))?;

        let val = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| BotError::Trading(format!("Failed to parse ticker: {e}")))?;

        let data = val.get("data").unwrap_or(&val);

        let mark_price = data
            .get("mark_price")
            .or_else(|| data.get("mp"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("mark_price").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);

        let last_price = data
            .get("last_traded_price")
            .or_else(|| data.get("ltp"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("last_traded_price").and_then(|v| v.as_f64()));

        let high_24h = data
            .get("high_price_24h")
            .or_else(|| data.get("hp24h"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("high_price_24h").and_then(|v| v.as_f64()));

        let low_24h = data
            .get("low_price_24h")
            .or_else(|| data.get("lp24h"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("low_price_24h").and_then(|v| v.as_f64()));

        let price_change_24h = data
            .get("price_change_24h")
            .or_else(|| data.get("pc24h"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("price_change_24h").and_then(|v| v.as_f64()));

        let funding_rate = data
            .get("perp_next_funding_rate")
            .or_else(|| data.get("pnfr"))
            .or_else(|| data.get("funding_rate"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| data.get("perp_next_funding_rate").and_then(|v| v.as_f64()));

        let next_funding_time_ms = data
            .get("perp_next_funding_time_ms")
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
                let trade_id = f
                    .get("id")
                    .or_else(|| f.get("trade_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let market_id = f.get("market_id").and_then(|v| v.as_str()).unwrap_or("");
                let market_symbol = self.get_market_name(market_id);

                let side = f
                    .get("side")
                    .or_else(|| f.get("direction"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Trade")
                    .to_string();

                let price = f
                    .get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let size = f
                    .get("volume")
                    .or_else(|| f.get("size"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let notional = if let Some(fv) = f
                    .get("filled_value")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    fv
                } else {
                    price * size
                };

                let timestamp = f
                    .get("created_at_epoch_ms")
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
            if let Ok(vol) = self
                .get_user_volume(&u.account_id, u.indexer_account_id.as_deref())
                .await
            {
                total += vol.total;
            }
        }
        Ok(total)
    }

    /// Fetch open position for a given market from the indexer.
    pub async fn get_open_position(&self, market_id: &str) -> BotResult<Option<OpenPosition>> {
        self.get_user_open_position(&self.config.account_id, None, market_id)
            .await
    }

    /// Fetch open orders directly for an indexer ID or account ID.
    /// Returns:
    /// - Ok(Some(list)): Authoritative account response from indexer (status 200). List contains active orders (may be empty).
    /// - Ok(None): Account not found on indexer (status 404).
    /// - Err(e): Network or timeout error.
    async fn fetch_orders_from_indexer(&self, id: &str) -> BotResult<Option<Vec<OpenLimitOrder>>> {
        let url = format!(
            "{}/api/v1/accounts/{}/orders",
            self.config.network.indexer_url(),
            id
        );

        let fut =
            async {
                let resp =
                    self.http.get(&url).send().await.map_err(|e| {
                        BotError::Trading(format!("Failed to connect to indexer: {e}"))
                    })?;

                if resp.status() == reqwest::StatusCode::NOT_FOUND {
                    debug!("Indexer orders endpoint returned 404 for id {id}");
                    return Ok(None);
                }

                if !resp.status().is_success() {
                    let status = resp.status();
                    warn!("Indexer orders endpoint returned status {status} for id {id}");
                    return Err(BotError::Trading(format!(
                        "Indexer returned status {status}"
                    )));
                }

                let val: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);

                Ok(Some(val))
            };

        let val_opt = match tokio::time::timeout(Duration::from_secs(5), fut).await {
            Ok(Ok(Some(v))) => v,
            Ok(Ok(None)) => return Ok(None),
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(BotError::Trading("Indexer request timed out".to_string())),
        };

        let orders_opt = val_opt
            .get("data")
            .and_then(|d| d.as_array())
            .or_else(|| val_opt.as_array())
            .or_else(|| val_opt.get("orders").and_then(|o| o.as_array()))
            .or_else(|| val_opt.get("result").and_then(|r| r.as_array()));

        let mut list = Vec::new();
        if let Some(orders) = orders_opt {
            for o in orders {
                // Check order status: skip terminal states
                let status = o
                    .get("status")
                    .or_else(|| o.get("st"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Active");

                if status.eq_ignore_ascii_case("filled")
                    || status.eq_ignore_ascii_case("cancelled")
                    || status.eq_ignore_ascii_case("canceled")
                    || status.eq_ignore_ascii_case("closed")
                    || status.eq_ignore_ascii_case("expired")
                    || status.eq_ignore_ascii_case("rejected")
                    || status.eq_ignore_ascii_case("killed")
                {
                    continue;
                }

                // Extract order_id / id from compact or verbose fields
                let id_raw = o
                    .get("order_id")
                    .or_else(|| o.get("id"))
                    .or_else(|| o.get("i"))
                    .or_else(|| o.get("oid"))
                    .or_else(|| o.get("client_order_id"))
                    .or_else(|| o.get("ci"));

                let order_id_str = match id_raw {
                    Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
                    Some(v) if v.is_number() => v.to_string(),
                    _ => String::new(),
                };

                if order_id_str.is_empty() {
                    continue;
                }

                let order_id = order_id_str
                    .parse::<u128>()
                    .ok()
                    .or_else(|| {
                        let clean = order_id_str
                            .trim_start_matches("0x")
                            .trim_start_matches("0X");
                        u128::from_str_radix(clean, 16).ok()
                    })
                    .unwrap_or(0);

                let market_id = o
                    .get("market_id")
                    .or_else(|| o.get("m"))
                    .or_else(|| o.get("market"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let market_symbol = self.get_market_name(&market_id);

                let side = o
                    .get("side")
                    .or_else(|| o.get("s"))
                    .or_else(|| o.get("direction"))
                    .or_else(|| o.get("d"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Buy")
                    .to_string();

                let price = o
                    .get("price")
                    .or_else(|| o.get("p"))
                    .and_then(|v| {
                        if let Some(s) = v.as_str() {
                            s.parse::<f64>().ok()
                        } else if let Some(n) = v.as_f64() {
                            Some(n)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0.0);

                let quantity = o
                    .get("quantity")
                    .or_else(|| o.get("q"))
                    .or_else(|| o.get("volume"))
                    .or_else(|| o.get("size"))
                    .and_then(|v| {
                        if let Some(s) = v.as_str() {
                            s.parse::<f64>().ok()
                        } else if let Some(n) = v.as_f64() {
                            Some(n)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0.0);

                let remaining_quantity = o
                    .get("remaining_quantity")
                    .or_else(|| o.get("rq"))
                    .or_else(|| o.get("remaining_size"))
                    .or_else(|| o.get("remaining_volume"))
                    .or_else(|| o.get("unfilled_size"))
                    .and_then(|v| {
                        if let Some(s) = v.as_str() {
                            s.parse::<f64>().ok()
                        } else if let Some(n) = v.as_f64() {
                            Some(n)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(quantity);

                if remaining_quantity <= 0.0
                    && (status.eq_ignore_ascii_case("partially_filled") || quantity > 0.0)
                {
                    continue;
                }

                let timestamp = o
                    .get("created_at_ms")
                    .or_else(|| o.get("ca_ms"))
                    .or_else(|| o.get("created_at_epoch_ms"))
                    .or_else(|| o.get("timestamp_ms"))
                    .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
                    .or_else(|| {
                        o.get("created_at")
                            .and_then(|v| v.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.timestamp_millis())
                    })
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                list.push(OpenLimitOrder {
                    order_id,
                    order_id_str,
                    market_id,
                    market_symbol,
                    side,
                    price,
                    quantity,
                    remaining_quantity,
                    timestamp,
                });
            }
        }

        list.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        info!(id = %id, count = list.len(), "Fetched open limit orders from indexer");
        Ok(Some(list))
    }

    /// Fetch all open/unfilled limit orders from the Real.xyz indexer for a user.
    /// Prioritizes indexer_id, account_id, and resolves if needed.
    pub async fn get_user_open_orders(
        &self,
        account_id: &str,
        trading_account_obj_id_opt: Option<&str>,
        indexer_id_opt: Option<&str>,
    ) -> BotResult<Vec<OpenLimitOrder>> {
        let mut candidate_ids: Vec<String> = Vec::new();

        // 1. Indexer account ID if provided - prioritize this as it directly indexes orders!
        if let Some(iid) = indexer_id_opt {
            let i = iid.trim();
            if !i.is_empty() && !candidate_ids.iter().any(|c| c.eq_ignore_ascii_case(i)) {
                candidate_ids.push(i.to_string());
            }
        }

        // 2. Account ID (indexer ID e.g. 0x25fe...)
        let acc = account_id.trim();
        if !acc.is_empty() && !candidate_ids.iter().any(|c| c.eq_ignore_ascii_case(acc)) {
            candidate_ids.push(acc.to_string());
        }

        // 3. Trading account object ID (on-chain object id e.g. 0xdbd7...)
        if let Some(toid) = trading_account_obj_id_opt {
            let t = toid.trim();
            if !t.is_empty() && !candidate_ids.iter().any(|c| c.eq_ignore_ascii_case(t)) {
                candidate_ids.push(t.to_string());
            }
        }

        // 4. Try candidates in order. When indexer returns HTTP 200 (Some), return immediately!
        for cid in &candidate_ids {
            match self.fetch_orders_from_indexer(cid).await {
                Ok(Some(orders)) => {
                    // Authoritative response from indexer found!
                    return Ok(orders);
                }
                Ok(None) => {
                    // 404: Account not found under this candidate ID, try next
                }
                Err(e) => {
                    warn!(id = %cid, error = %e, "Candidate indexer order fetch failed");
                }
            }
        }

        // 5. Fallback: If all candidates returned 404, attempt indexer resolution
        let to_resolve = trading_account_obj_id_opt.unwrap_or(acc);
        if !to_resolve.is_empty() {
            if let Some(resolved) = self.resolve_user_indexer_id(to_resolve).await {
                if !candidate_ids
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(&resolved))
                {
                    if let Ok(Some(orders)) = self.fetch_orders_from_indexer(&resolved).await {
                        return Ok(orders);
                    }
                }
            }
        }

        Ok(Vec::new())
    }

    /// Fetch open orders for default/admin config account.
    pub async fn get_open_orders(&self) -> BotResult<Vec<OpenLimitOrder>> {
        let eff_acc = self.get_effective_account_id().await;
        self.get_user_open_orders(&eff_acc, Some(&self.config.account_id), Some(&eff_acc))
            .await
    }

    // ─── Public Order Operations ────────────────────────────────────────────

    /// Fallback sanitization helper if market is not specified (defaults to 3 decimals).
    pub fn sanitize_order_quantity(qty: &str) -> String {
        if let Ok(val) = qty.parse::<f64>() {
            let (formatted, _) = floor_qty_to_decimals(val, 3);
            formatted
        } else {
            qty.to_string()
        }
    }

    /// Risk inputs used by [`size_marketable_qty`]. Falls back to the static
    /// schedule when the indexer cache has not been filled yet.
    pub fn market_risk_params(&self, market: &str) -> MarketRiskParams {
        let markets = self.get_cached_markets();
        let found = markets.iter().find(|m| {
            m.market_id.eq_ignore_ascii_case(market)
                || m.base_asset.eq_ignore_ascii_case(market)
                || m.short_name.eq_ignore_ascii_case(market)
                || m.symbol.eq_ignore_ascii_case(market)
        });
        if let Some(m) = found {
            let max_lev = if m.max_leverage > 0.0 {
                m.max_leverage
            } else {
                get_cached_market_max_leverage(market)
            };
            MarketRiskParams {
                imr: if m.imr > 0.0 {
                    m.imr
                } else {
                    1.0 / max_lev.max(1.0)
                },
                taker_fee: if m.taker_fee >= 0.0 {
                    m.taker_fee
                } else {
                    DEFAULT_TAKER_FEE
                },
                trading_bound: if m.trading_bound > 0.0 {
                    m.trading_bound
                } else {
                    DEFAULT_TRADING_BOUND
                },
                lot_size: if m.lot_size > 0.0 {
                    m.lot_size
                } else {
                    get_cached_market_lot_size(market)
                },
                qty_decimals: m.qty_decimals,
                price_decimals: m.price_decimals,
            }
        } else {
            let max_lev = get_cached_market_max_leverage(market).max(1.0);
            MarketRiskParams {
                imr: 1.0 / max_lev,
                taker_fee: DEFAULT_TAKER_FEE,
                trading_bound: DEFAULT_TRADING_BOUND,
                lot_size: get_cached_market_lot_size(market),
                qty_decimals: get_cached_market_decimals(market),
                price_decimals: get_cached_market_price_decimals(market),
            }
        }
    }

    /// Asks for a buy, bids for a sell. Empty when the book cannot be read.
    async fn fetch_depth_levels(&self, market: &str, is_buy: bool) -> Vec<BookLevel> {
        let url = format!(
            "{}/api/v1/markets/{}/depth",
            self.config.network.indexer_url(),
            market
        );
        let resp = match self.http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                warn!(market, status = %resp.status(), "depth endpoint returned a non-success status");
                return Vec::new();
            }
            Err(e) => {
                warn!(market, error = %e, "depth unavailable; protective price will use the mark fallback");
                return Vec::new();
            }
        };
        let val = match resp.json::<serde_json::Value>().await {
            Ok(val) => val,
            Err(e) => {
                warn!(market, error = %e, "failed to parse depth payload");
                return Vec::new();
            }
        };
        let data = val.get("data").unwrap_or(&val);
        let side_keys: &[&str] = if is_buy {
            &["sells", "asks", "ask"]
        } else {
            &["buys", "bids", "bid"]
        };
        let levels = side_keys
            .iter()
            .find_map(|key| data.get(*key).and_then(|v| v.as_array()));
        let Some(levels) = levels else {
            return Vec::new();
        };
        levels
            .iter()
            .filter_map(|lvl| {
                let price = lvl
                    .get("price_level")
                    .or_else(|| lvl.get("price"))
                    .or_else(|| lvl.get("p"))
                    .and_then(json_f64)
                    .or_else(|| {
                        lvl.as_array()
                            .and_then(|pair| pair.first())
                            .and_then(json_f64)
                    })?;
                let qty = lvl
                    .get("volume")
                    .or_else(|| lvl.get("quantity"))
                    .or_else(|| lvl.get("size"))
                    .or_else(|| lvl.get("q"))
                    .and_then(json_f64)
                    .or_else(|| {
                        lvl.as_array()
                            .and_then(|pair| pair.get(1))
                            .and_then(json_f64)
                    })?;
                if price > 0.0 && qty > 0.0 {
                    Some(BookLevel { price, qty })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Size a marketable order against `budget_usd` at `leverage` (capped at `1/imr`).
    ///
    /// `qty_cap` is an explicit base-asset size from a slash command. The result
    /// is the smaller of that cap and the quantity the budget can margin,
    /// floored to the lot. Price and size are solved together: the book walk
    /// depends on quantity, and the margin formula depends on that price.
    pub async fn resolve_marketable_qty(
        &self,
        market: &str,
        is_buy: bool,
        budget_usd: f64,
        leverage: f64,
        qty_cap: Option<f64>,
    ) -> BotResult<MarketableQuote> {
        if !(budget_usd > 0.0) {
            return Err(BotError::Trading(
                "No margin budget for market order".into(),
            ));
        }
        let _ = self.get_markets().await;
        let mark = self.get_market_mark_price(market).await?;
        let params = self.market_risk_params(market);
        let levels = self.fetch_depth_levels(market, is_buy).await;
        let imr = capped_imr(params.imr, leverage);
        let cap = qty_cap.filter(|q| *q > 0.0);
        let mut qty_guess = cap.unwrap_or_else(|| {
            if mark > 0.0 && imr > 0.0 {
                budget_usd / (mark * imr)
            } else {
                0.0
            }
        });

        let mut quote: Option<MarketableQuote> = None;
        for _ in 0..4 {
            let (price, price_f) = protective_slippage_price(
                mark,
                is_buy,
                params.price_decimals,
                &levels,
                qty_guess,
                params.trading_bound,
            );
            if !(price_f > 0.0) {
                break;
            }
            let (_, sized) = size_marketable_qty(
                budget_usd,
                price_f,
                mark,
                is_buy,
                imr,
                params.taker_fee,
                params.lot_size,
                params.qty_decimals,
            );
            let capped = cap.map(|limit| sized.min(limit)).unwrap_or(sized);
            let (quantity, quantity_f) =
                floor_qty_to_lot(capped, params.lot_size, params.qty_decimals);
            quote = Some(MarketableQuote {
                quantity,
                quantity_f,
                price,
                price_f,
                mark,
            });
            if quantity_f <= 0.0 {
                break;
            }
            let settled = (quantity_f - qty_guess).abs() <= params.lot_size.max(1e-12) * 0.5;
            qty_guess = quantity_f;
            if settled {
                break;
            }
        }

        match quote {
            Some(q) if q.quantity_f > 0.0 && q.price_f > 0.0 => {
                info!(
                    market,
                    is_buy,
                    budget = budget_usd,
                    leverage,
                    imr,
                    mark = q.mark,
                    price = %q.price,
                    quantity = %q.quantity,
                    "resolved marketable order size"
                );
                Ok(q)
            }
            _ => Err(BotError::Trading(
                "Order size is below the lot size for the available margin".into(),
            )),
        }
    }

    /// Cap an explicit base quantity to what `account_id` can margin, and pair
    /// it with a book-walking protective price. Shared by admin
    /// `place_market_*` and per-user `place_user_market_*`.
    pub async fn cap_requested_market_qty(
        &self,
        account_id: &str,
        indexer_id: Option<&str>,
        market: &str,
        is_buy: bool,
        requested_qty: &str,
    ) -> (String, Option<String>) {
        let requested = requested_qty.parse::<f64>().unwrap_or(0.0);
        if !(requested > 0.0) {
            return (requested_qty.to_string(), None);
        }
        let budget = match self.get_user_balance(account_id, indexer_id).await {
            Ok(balance) => balance.spendable_margin(),
            Err(e) => {
                warn!(
                    account_id,
                    market,
                    error = %e,
                    "margin unavailable; submitting the requested market quantity"
                );
                let price = self
                    .market_protective_price(market, is_buy, requested)
                    .await;
                return (requested_qty.to_string(), price);
            }
        };
        if !(budget > 0.0) {
            warn!(
                account_id,
                market, "no spendable margin; submitting the requested market quantity"
            );
            let price = self
                .market_protective_price(market, is_buy, requested)
                .await;
            return (requested_qty.to_string(), price);
        }
        let leverage = self.get_market_max_leverage(market);
        match self
            .resolve_marketable_qty(market, is_buy, budget, leverage, Some(requested))
            .await
        {
            Ok(quote) => (quote.quantity, Some(quote.price)),
            Err(e) => {
                warn!(
                    account_id,
                    market,
                    error = %e,
                    "marketable sizing failed; submitting the requested quantity"
                );
                let price = self
                    .market_protective_price(market, is_buy, requested)
                    .await;
                (requested_qty.to_string(), price)
            }
        }
    }

    /// Protective IOC price for `qty`, from the live book when it is available.
    /// `None` only when the mark itself cannot be read.
    async fn market_protective_price(
        &self,
        market: &str,
        is_buy: bool,
        qty: f64,
    ) -> Option<String> {
        let mark = match self.get_market_mark_price(market).await {
            Ok(mark) if mark > 0.0 => mark,
            Ok(_) => return None,
            Err(e) => {
                warn!(
                    market,
                    error = %e,
                    "mark unavailable; market order will not carry a protective price"
                );
                return None;
            }
        };
        let params = self.market_risk_params(market);
        let levels = self.fetch_depth_levels(market, is_buy).await;
        let (price, value) = protective_slippage_price(
            mark,
            is_buy,
            params.price_decimals,
            &levels,
            qty,
            params.trading_bound,
        );
        if value > 0.0 {
            info!(
                market,
                is_buy,
                mark,
                qty,
                levels = levels.len(),
                protective = %price,
                "computed market protective price"
            );
            Some(price)
        } else {
            None
        }
    }

    async fn submit_parsed(
        &self,
        client: &SigningClient,
        order: SubmitOrderRequest,
        side: &str,
        quantity: &str,
        price: &str,
        market: &str,
        client_order_id: &str,
    ) -> BotResult<OrderResult> {
        let req_id = Uuid::new_v4().to_string();
        let report = match client.submit_order(order, Some(req_id)).await {
            Ok(report) => report,
            Err(e) => {
                error!(
                    market,
                    side,
                    quantity,
                    price,
                    client_order_id,
                    error = %e,
                    "order submit failed"
                );
                return Err(BotError::Trading(format!("submit_order: {e}")));
            }
        };
        self.parse_order_result(report, side, quantity, price, market, client_order_id)
            .await
    }

    fn build_marketable_order(
        market: &str,
        account_id: &str,
        is_buy: bool,
        qty: &str,
        protective: Option<&str>,
        coid: &str,
    ) -> BotResult<SubmitOrderRequest> {
        let mut builder = SubmitOrderRequest::builder();
        builder = builder.market(market).account(account_id);
        builder = if is_buy {
            builder.buy()
        } else {
            builder.sell()
        };
        // IOC limit at the side-correct cap. A naked market order cannot carry a
        // price (the SDK rejects one), so the protective price is a marketable limit.
        builder = match protective {
            Some(price) => builder.limit_order(price),
            None => builder.market_order(),
        };
        builder
            .quantity(qty)
            .immediate_or_cancel()
            .client_order_id(coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build market order: {e}")))
    }

    /// Shared by `place_market_long` / `place_market_short` and the per-user twins.
    /// Same protective price, same Error 114 / 121 retries, same 0.90 then 0.80
    /// InsufficientMargin reductions.
    async fn place_marketable_order(
        &self,
        client: &SigningClient,
        account_id: &str,
        market: &str,
        quantity: &str,
        is_buy: bool,
        mut protective: Option<String>,
    ) -> BotResult<OrderResult> {
        let (clean_qty, decimals, _) = sanitize_order_quantity_for_market(market, quantity);
        let price_decimals = get_cached_market_price_decimals(market);
        let side = if is_buy {
            "Buy (Market)"
        } else {
            "Sell (Market)"
        };
        let price_label = protective.clone().unwrap_or_else(|| "Market".to_string());

        info!(
            market,
            side,
            quantity = %clean_qty,
            protective = %price_label,
            "placing market order"
        );

        let coid = Uuid::new_v4().to_string();
        let order = Self::build_marketable_order(
            market,
            account_id,
            is_buy,
            &clean_qty,
            protective.as_deref(),
            &coid,
        )?;
        let mut res = self
            .submit_parsed(client, order, side, &clean_qty, &price_label, market, &coid)
            .await?;

        if !res.success
            && protective.is_some()
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_price_decimal_error)
        {
            let current = protective.as_deref().unwrap();
            if let Some((retry_price, new_dec)) =
                next_fewer_decimal_price(market, current, price_decimals)
            {
                info!(
                    market,
                    old_price_decimals = price_decimals,
                    new_price_decimals = new_dec,
                    retry_price = %retry_price,
                    "Retrying market order after Error 114"
                );
                protective = Some(retry_price.clone());
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = Self::build_marketable_order(
                    market,
                    account_id,
                    is_buy,
                    &clean_qty,
                    Some(&retry_price),
                    &retry_coid,
                ) {
                    if let Ok(retry_res) = self
                        .submit_parsed(
                            client,
                            retry_order,
                            side,
                            &clean_qty,
                            &retry_price,
                            market,
                            &retry_coid,
                        )
                        .await
                    {
                        res = retry_res;
                    }
                }
            }
        } else if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_decimal_precision_error)
        {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals)
            {
                info!(
                    market,
                    old_decimals = decimals,
                    new_decimals = new_dec,
                    retry_qty = %retry_qty,
                    "Retrying market order after Error 121"
                );
                let retry_label = protective.clone().unwrap_or_else(|| "Market".to_string());
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = Self::build_marketable_order(
                    market,
                    account_id,
                    is_buy,
                    &retry_qty,
                    protective.as_deref(),
                    &retry_coid,
                ) {
                    if let Ok(retry_res) = self
                        .submit_parsed(
                            client,
                            retry_order,
                            side,
                            &retry_qty,
                            &retry_label,
                            market,
                            &retry_coid,
                        )
                        .await
                    {
                        res = retry_res;
                    }
                }
            }
        }

        if !res.success && res.error_msg.as_deref().map_or(false, is_price_bound_error) {
            let bound_qty = clean_qty.parse::<f64>().unwrap_or(0.0);
            if let Some(fresh) = self
                .market_protective_price(market, is_buy, bound_qty)
                .await
            {
                info!(
                    market,
                    retry_price = %fresh,
                    "Retrying market order after price-bound rejection"
                );
                protective = Some(fresh.clone());
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = Self::build_marketable_order(
                    market,
                    account_id,
                    is_buy,
                    &clean_qty,
                    Some(&fresh),
                    &retry_coid,
                ) {
                    if let Ok(retry_res) = self
                        .submit_parsed(
                            client,
                            retry_order,
                            side,
                            &clean_qty,
                            &fresh,
                            market,
                            &retry_coid,
                        )
                        .await
                    {
                        res = retry_res;
                    }
                }
            }
        }

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_insufficient_margin_error)
        {
            if let Ok(val) = clean_qty.parse::<f64>() {
                for reduction in [0.90_f64, 0.80] {
                    let reduced_val = val * reduction;
                    let (retry_qty, floored) = floor_qty_to_decimals(reduced_val, decimals);
                    if floored > 0.0 && retry_qty != clean_qty {
                        if let Some(fresh) =
                            self.market_protective_price(market, is_buy, floored).await
                        {
                            info!(
                                market,
                                old_price = ?protective,
                                retry_price = %fresh,
                                "recomputed protective price from a fresh book before margin retry"
                            );
                            protective = Some(fresh);
                        }
                        info!(
                            market,
                            old_qty = %clean_qty,
                            retry_qty = %retry_qty,
                            reduction,
                            protective = ?protective,
                            "Retrying market order after InsufficientMargin"
                        );
                        let retry_label =
                            protective.clone().unwrap_or_else(|| "Market".to_string());
                        let retry_coid = Uuid::new_v4().to_string();
                        if let Ok(retry_order) = Self::build_marketable_order(
                            market,
                            account_id,
                            is_buy,
                            &retry_qty,
                            protective.as_deref(),
                            &retry_coid,
                        ) {
                            if let Ok(retry_res) = self
                                .submit_parsed(
                                    client,
                                    retry_order,
                                    side,
                                    &retry_qty,
                                    &retry_label,
                                    market,
                                    &retry_coid,
                                )
                                .await
                            {
                                res = retry_res;
                                if res.success {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(res)
    }

    fn build_reduce_only_order(
        market: &str,
        account_id: &str,
        side: &str,
        qty: &str,
        price: Option<&str>,
        coid: &str,
    ) -> BotResult<SubmitOrderRequest> {
        let mut builder = SubmitOrderRequest::builder();
        builder = builder.market(market).account(account_id);
        builder = if side.eq_ignore_ascii_case("buy") {
            builder.buy()
        } else {
            builder.sell()
        };
        builder = if let Some(p) = price {
            builder.limit_order(p).good_till_cancel()
        } else {
            builder.market_order().immediate_or_cancel()
        };
        builder
            .quantity(qty)
            .reduce_only(true)
            .client_order_id(coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build close order: {e}")))
    }

    /// Shared by `place_close_order` and `place_user_close_order`.
    async fn place_reduce_only_order(
        &self,
        client: &SigningClient,
        account_id: &str,
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
        let price_label = clean_price_opt
            .clone()
            .unwrap_or_else(|| "Market".to_string());
        info!(
            market,
            side,
            quantity = %clean_qty,
            price = %price_label,
            "placing close order"
        );

        let coid = Uuid::new_v4().to_string();
        let order = Self::build_reduce_only_order(
            market,
            account_id,
            side,
            &clean_qty,
            clean_price_opt.as_deref(),
            &coid,
        )?;
        let mut res = self
            .submit_parsed(client, order, side, &clean_qty, &price_label, market, &coid)
            .await?;

        if !res.success
            && clean_price_opt.is_some()
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_price_decimal_error)
        {
            let current = clean_price_opt.as_deref().unwrap();
            if let Some((retry_price, new_dec)) =
                next_fewer_decimal_price(market, current, price_decimals)
            {
                info!(
                    market,
                    old_price_decimals = price_decimals,
                    new_price_decimals = new_dec,
                    retry_price = %retry_price,
                    "Retrying close order after Error 114"
                );
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = Self::build_reduce_only_order(
                    market,
                    account_id,
                    side,
                    &clean_qty,
                    Some(&retry_price),
                    &retry_coid,
                ) {
                    if let Ok(retry_res) = self
                        .submit_parsed(
                            client,
                            retry_order,
                            side,
                            &clean_qty,
                            &retry_price,
                            market,
                            &retry_coid,
                        )
                        .await
                    {
                        res = retry_res;
                    }
                }
            }
        } else if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_decimal_precision_error)
        {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals)
            {
                info!(
                    market,
                    old_decimals = decimals,
                    new_decimals = new_dec,
                    retry_qty = %retry_qty,
                    "Retrying close order after Error 121"
                );
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = Self::build_reduce_only_order(
                    market,
                    account_id,
                    side,
                    &retry_qty,
                    clean_price_opt.as_deref(),
                    &retry_coid,
                ) {
                    if let Ok(retry_res) = self
                        .submit_parsed(
                            client,
                            retry_order,
                            side,
                            &retry_qty,
                            &price_label,
                            market,
                            &retry_coid,
                        )
                        .await
                    {
                        res = retry_res;
                    }
                }
            }
        }

        // A close sized with round-half-up can be one lot above the position.
        // Step down and resubmit instead of leaving ReduceOnlyOrderExceedsPosition unlogged.
        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_reduce_only_exceeds_position)
        {
            let mut current_qty = res.quantity.clone();
            if current_qty.is_empty() {
                current_qty = clean_qty.clone();
            }
            for _ in 0..3 {
                let Some((retry_qty, _)) = step_down_qty(&current_qty, decimals) else {
                    break;
                };
                info!(
                    market,
                    old_qty = %current_qty,
                    retry_qty = %retry_qty,
                    "Retrying close order after ReduceOnlyOrderExceedsPosition"
                );
                current_qty = retry_qty.clone();
                let retry_coid = Uuid::new_v4().to_string();
                let Ok(retry_order) = Self::build_reduce_only_order(
                    market,
                    account_id,
                    side,
                    &retry_qty,
                    clean_price_opt.as_deref(),
                    &retry_coid,
                ) else {
                    break;
                };
                match self
                    .submit_parsed(
                        client,
                        retry_order,
                        side,
                        &retry_qty,
                        &price_label,
                        market,
                        &retry_coid,
                    )
                    .await
                {
                    Ok(retry_res) => {
                        let still_exceeds = !retry_res.success
                            && retry_res
                                .error_msg
                                .as_deref()
                                .map_or(false, is_reduce_only_exceeds_position);
                        res = retry_res;
                        if res.success || !still_exceeds {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }

        Ok(res)
    }

    /// Place a market buy (long). Same sizing and protective price as
    /// `place_user_market_long`.
    pub async fn place_market_long(&self, market: &str, quantity: &str) -> BotResult<OrderResult> {
        let account_id = self.get_effective_account_id().await;
        let client = self.client.lock().await;
        self.place_marketable_for_account(&client, &account_id, None, market, quantity, true, None)
            .await
    }

    /// Place a market sell (short). Same sizing and protective price as
    /// `place_user_market_short`.
    pub async fn place_market_short(&self, market: &str, quantity: &str) -> BotResult<OrderResult> {
        let account_id = self.get_effective_account_id().await;
        let client = self.client.lock().await;
        self.place_marketable_for_account(&client, &account_id, None, market, quantity, false, None)
            .await
    }

    /// Shared admin/user market entry. Caps `quantity` with [`size_marketable_qty`]
    /// against spendable margin and submits at a book-walking protective price.
    async fn place_marketable_for_account(
        &self,
        client: &SigningClient,
        account_id: &str,
        indexer_id: Option<&str>,
        market: &str,
        quantity: &str,
        is_buy: bool,
        protective_override: Option<String>,
    ) -> BotResult<OrderResult> {
        let requested = quantity.parse::<f64>().unwrap_or(0.0);
        let (sized_qty, sized_price) = self
            .cap_requested_market_qty(account_id, indexer_id, market, is_buy, quantity)
            .await;
        let sized_f = sized_qty.parse::<f64>().unwrap_or(0.0);
        if !(sized_f > 0.0) {
            return Err(BotError::Trading(
                "Order size is below the lot size for the available margin".into(),
            ));
        }
        // Keep a caller-supplied price only when sizing did not have to shrink
        // the quantity. A shrink means the book price and the size were solved
        // together and must stay paired.
        let shrunk = requested > 0.0 && sized_f + 1e-12 < requested;
        let protective = if !shrunk {
            protective_override.or(sized_price)
        } else {
            sized_price.or(protective_override)
        };
        self.place_marketable_order(client, account_id, market, &sized_qty, is_buy, protective)
            .await
    }

    /// Place a reduce-only order to close an open position (market or limit).
    /// Same path as `place_user_close_order` (`place_reduce_only_order`).
    pub async fn place_close_order(
        &self,
        market: &str,
        side: &str,
        quantity: &str,
        price: Option<&str>,
    ) -> BotResult<OrderResult> {
        let account_id = self.get_effective_account_id().await;
        let client = self.client.lock().await;
        self.place_reduce_only_order(&client, &account_id, market, side, quantity, price)
            .await
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
            .post_only(true)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        if order.order_type != OrderType::Limit {
            return Err(BotError::Trading(
                "Order invariant violated: order_type is not Limit".into(),
            ));
        }
        if order.limit_price.as_deref() != Some(clean_price.as_str()) {
            return Err(BotError::Trading(format!(
                "Limit price invariant violated: expected {}, got {:?}",
                clean_price, order.limit_price
            )));
        }

        info!(
            market = %market,
            side = "Buy",
            order_type = ?order.order_type,
            limit_price = %clean_price,
            quantity = %clean_qty,
            client_order_id = %coid,
            "Placing Limit BUY order to Real.xyz matching engine"
        );

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| {
                error!(market = %market, error = %e, "order submit failed");
                BotError::Trading(format!("submit_order: {e}"))
            })?;
        drop(client);

        let mut res = self
            .parse_order_result(report, "Buy", &clean_qty, &clean_price, market, &coid)
            .await?;

        info!(
            market = %market,
            side = "Buy",
            order_type = "Limit",
            limit_price = %clean_price,
            client_order_id = %coid,
            order_id = ?res.order_id,
            success = res.success,
            fills_count = res.fills.len(),
            error = ?res.error_msg,
            "Limit BUY order response received from matching engine"
        );

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_price_decimal_error)
        {
            if let Some((retry_price, new_p_dec)) =
                next_fewer_decimal_price(market, &clean_price, price_decimals)
            {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying limit buy with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .buy()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        drop(client);
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Buy",
                                &clean_qty,
                                &retry_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_decimal_precision_error)
        {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals)
            {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying limit buy with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .buy()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        drop(client);
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Buy",
                                &retry_qty,
                                &clean_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_insufficient_margin_error)
        {
            if let Ok(val) = clean_qty.parse::<f64>() {
                for reduction in [0.90, 0.80] {
                    let reduced_val = val * reduction;
                    let (retry_qty, floored) = floor_qty_to_decimals(reduced_val, decimals);
                    if floored > 0.0 && retry_qty != clean_qty {
                        info!(market = %market, old_qty = %clean_qty, retry_qty = %retry_qty, reduction = %reduction, "Retrying limit buy after InsufficientMargin");
                        let retry_coid = Uuid::new_v4().to_string();
                        if let Ok(retry_order) = SubmitOrderRequest::builder()
                            .market(market)
                            .account(&account_id)
                            .buy()
                            .limit_order(&clean_price)
                            .quantity(&retry_qty)
                            .post_only(true)
                            .good_till_cancel()
                            .client_order_id(&retry_coid)
                            .build()
                        {
                            let retry_req = Uuid::new_v4().to_string();
                            let client = self.client.lock().await;
                            let retry_submit =
                                client.submit_order(retry_order, Some(retry_req)).await;
                            if let Err(ref retry_err) = retry_submit {
                                error!(market = %market, error = %retry_err, "retry order submit failed");
                            }
                            if let Ok(retry_rep) = retry_submit {
                                drop(client);
                                if let Ok(retry_res) = self
                                    .parse_order_result(
                                        retry_rep,
                                        "Buy",
                                        &retry_qty,
                                        &clean_price,
                                        market,
                                        &retry_coid,
                                    )
                                    .await
                                {
                                    res = retry_res;
                                    if res.success {
                                        break;
                                    }
                                }
                            }
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
            .post_only(true)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        if order.order_type != OrderType::Limit {
            return Err(BotError::Trading(
                "Order invariant violated: order_type is not Limit".into(),
            ));
        }
        if order.limit_price.as_deref() != Some(clean_price.as_str()) {
            return Err(BotError::Trading(format!(
                "Limit price invariant violated: expected {}, got {:?}",
                clean_price, order.limit_price
            )));
        }

        info!(
            market = %market,
            side = "Sell",
            order_type = ?order.order_type,
            limit_price = %clean_price,
            quantity = %clean_qty,
            client_order_id = %coid,
            "Placing Limit SELL order to Real.xyz matching engine"
        );

        let req_id = Uuid::new_v4().to_string();
        let client = self.client.lock().await;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| {
                error!(market = %market, error = %e, "order submit failed");
                BotError::Trading(format!("submit_order: {e}"))
            })?;
        drop(client);

        let mut res = self
            .parse_order_result(report, "Sell", &clean_qty, &clean_price, market, &coid)
            .await?;

        info!(
            market = %market,
            side = "Sell",
            order_type = "Limit",
            limit_price = %clean_price,
            client_order_id = %coid,
            order_id = ?res.order_id,
            success = res.success,
            fills_count = res.fills.len(),
            error = ?res.error_msg,
            "Limit SELL order response received from matching engine"
        );

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_price_decimal_error)
        {
            if let Some((retry_price, new_p_dec)) =
                next_fewer_decimal_price(market, &clean_price, price_decimals)
            {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying limit sell with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .sell()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        drop(client);
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Sell",
                                &clean_qty,
                                &retry_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_decimal_precision_error)
        {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals)
            {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying limit sell with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(&account_id)
                    .sell()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let client = self.client.lock().await;
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        drop(client);
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Sell",
                                &retry_qty,
                                &clean_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_insufficient_margin_error)
        {
            if let Ok(val) = clean_qty.parse::<f64>() {
                for reduction in [0.90, 0.80] {
                    let reduced_val = val * reduction;
                    let (retry_qty, floored) = floor_qty_to_decimals(reduced_val, decimals);
                    if floored > 0.0 && retry_qty != clean_qty {
                        info!(market = %market, old_qty = %clean_qty, retry_qty = %retry_qty, reduction = %reduction, "Retrying limit sell after InsufficientMargin");
                        let retry_coid = Uuid::new_v4().to_string();
                        if let Ok(retry_order) = SubmitOrderRequest::builder()
                            .market(market)
                            .account(&account_id)
                            .sell()
                            .limit_order(&clean_price)
                            .quantity(&retry_qty)
                            .post_only(true)
                            .good_till_cancel()
                            .client_order_id(&retry_coid)
                            .build()
                        {
                            let retry_req = Uuid::new_v4().to_string();
                            let client = self.client.lock().await;
                            let retry_submit =
                                client.submit_order(retry_order, Some(retry_req)).await;
                            if let Err(ref retry_err) = retry_submit {
                                error!(market = %market, error = %retry_err, "retry order submit failed");
                            }
                            if let Ok(retry_rep) = retry_submit {
                                drop(client);
                                if let Ok(retry_res) = self
                                    .parse_order_result(
                                        retry_rep,
                                        "Sell",
                                        &retry_qty,
                                        &clean_price,
                                        market,
                                        &retry_coid,
                                    )
                                    .await
                                {
                                    res = retry_res;
                                    if res.success {
                                        break;
                                    }
                                }
                            }
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
        protective_price: Option<&str>,
    ) -> BotResult<OrderResult> {
        let protective = protective_price
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string());
        let client = self.create_user_signing_client(decrypted_key)?;
        self.place_marketable_for_account(
            &client, account_id, None, market, quantity, true, protective,
        )
        .await
    }

    pub async fn place_user_market_short(
        &self,
        account_id: &str,
        decrypted_key: &str,
        market: &str,
        quantity: &str,
        protective_price: Option<&str>,
    ) -> BotResult<OrderResult> {
        let protective = protective_price
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string());
        let client = self.create_user_signing_client(decrypted_key)?;
        self.place_marketable_for_account(
            &client, account_id, None, market, quantity, false, protective,
        )
        .await
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
            .post_only(true)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        if order.order_type != OrderType::Limit {
            return Err(BotError::Trading(
                "Order invariant violated: order_type is not Limit".into(),
            ));
        }
        if order.limit_price.as_deref() != Some(clean_price.as_str()) {
            return Err(BotError::Trading(format!(
                "Limit price invariant violated: expected {}, got {:?}",
                clean_price, order.limit_price
            )));
        }

        info!(
            market = %market,
            side = "Buy",
            order_type = ?order.order_type,
            limit_price = %clean_price,
            quantity = %clean_qty,
            account_id = %account_id,
            client_order_id = %coid,
            "Placing user Limit BUY order to Real.xyz matching engine"
        );

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| {
                error!(market = %market, error = %e, "order submit failed");
                BotError::Trading(format!("submit_order: {e}"))
            })?;

        let mut res = self
            .parse_order_result(report, "Buy", &clean_qty, &clean_price, market, &coid)
            .await?;

        info!(
            market = %market,
            side = "Buy",
            order_type = "Limit",
            limit_price = %clean_price,
            client_order_id = %coid,
            order_id = ?res.order_id,
            success = res.success,
            fills_count = res.fills.len(),
            error = ?res.error_msg,
            "User Limit BUY order response received from matching engine"
        );

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_price_decimal_error)
        {
            if let Some((retry_price, new_p_dec)) =
                next_fewer_decimal_price(market, &clean_price, price_decimals)
            {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying user limit buy with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .buy()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Buy",
                                &clean_qty,
                                &retry_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_decimal_precision_error)
        {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals)
            {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user limit buy with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .buy()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Buy",
                                &retry_qty,
                                &clean_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_insufficient_margin_error)
        {
            if let Ok(val) = clean_qty.parse::<f64>() {
                for reduction in [0.90, 0.80] {
                    let reduced_val = val * reduction;
                    let (retry_qty, floored) = floor_qty_to_decimals(reduced_val, decimals);
                    if floored > 0.0 && retry_qty != clean_qty {
                        info!(market = %market, old_qty = %clean_qty, retry_qty = %retry_qty, reduction = %reduction, "Retrying user limit buy after InsufficientMargin");
                        let retry_coid = Uuid::new_v4().to_string();
                        if let Ok(retry_order) = SubmitOrderRequest::builder()
                            .market(market)
                            .account(account_id)
                            .buy()
                            .limit_order(&clean_price)
                            .quantity(&retry_qty)
                            .post_only(true)
                            .good_till_cancel()
                            .client_order_id(&retry_coid)
                            .build()
                        {
                            let retry_req = Uuid::new_v4().to_string();
                            let retry_submit =
                                client.submit_order(retry_order, Some(retry_req)).await;
                            if let Err(ref retry_err) = retry_submit {
                                error!(market = %market, error = %retry_err, "retry order submit failed");
                            }
                            if let Ok(retry_rep) = retry_submit {
                                if let Ok(retry_res) = self
                                    .parse_order_result(
                                        retry_rep,
                                        "Buy",
                                        &retry_qty,
                                        &clean_price,
                                        market,
                                        &retry_coid,
                                    )
                                    .await
                                {
                                    res = retry_res;
                                    if res.success {
                                        break;
                                    }
                                }
                            }
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
            .post_only(true)
            .good_till_cancel()
            .client_order_id(&coid)
            .build()
            .map_err(|e| BotError::Trading(format!("Build order: {e}")))?;

        if order.order_type != OrderType::Limit {
            return Err(BotError::Trading(
                "Order invariant violated: order_type is not Limit".into(),
            ));
        }
        if order.limit_price.as_deref() != Some(clean_price.as_str()) {
            return Err(BotError::Trading(format!(
                "Limit price invariant violated: expected {}, got {:?}",
                clean_price, order.limit_price
            )));
        }

        info!(
            market = %market,
            side = "Sell",
            order_type = ?order.order_type,
            limit_price = %clean_price,
            quantity = %clean_qty,
            account_id = %account_id,
            client_order_id = %coid,
            "Placing user Limit SELL order to Real.xyz matching engine"
        );

        let req_id = Uuid::new_v4().to_string();
        let client = self.create_user_signing_client(decrypted_key)?;
        let report = client
            .submit_order(order, Some(req_id))
            .await
            .map_err(|e| {
                error!(market = %market, error = %e, "order submit failed");
                BotError::Trading(format!("submit_order: {e}"))
            })?;

        let mut res = self
            .parse_order_result(report, "Sell", &clean_qty, &clean_price, market, &coid)
            .await?;

        info!(
            market = %market,
            side = "Sell",
            order_type = "Limit",
            limit_price = %clean_price,
            client_order_id = %coid,
            order_id = ?res.order_id,
            success = res.success,
            fills_count = res.fills.len(),
            error = ?res.error_msg,
            "User Limit SELL order response received from matching engine"
        );

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_price_decimal_error)
        {
            if let Some((retry_price, new_p_dec)) =
                next_fewer_decimal_price(market, &clean_price, price_decimals)
            {
                info!(market = %market, old_price_decimals = price_decimals, new_price_decimals = new_p_dec, retry_price = %retry_price, "Retrying user limit sell with 1 fewer price decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .sell()
                    .limit_order(&retry_price)
                    .quantity(&clean_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Sell",
                                &clean_qty,
                                &retry_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        } else if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_decimal_precision_error)
        {
            if let Some((retry_qty, new_dec)) = next_fewer_decimal_qty(market, &clean_qty, decimals)
            {
                info!(market = %market, old_decimals = decimals, new_decimals = new_dec, retry_qty = %retry_qty, "Retrying user limit sell with 1 fewer decimal place");
                let retry_coid = Uuid::new_v4().to_string();
                if let Ok(retry_order) = SubmitOrderRequest::builder()
                    .market(market)
                    .account(account_id)
                    .sell()
                    .limit_order(&clean_price)
                    .quantity(&retry_qty)
                    .post_only(true)
                    .good_till_cancel()
                    .client_order_id(&retry_coid)
                    .build()
                {
                    let retry_req = Uuid::new_v4().to_string();
                    let retry_submit = client.submit_order(retry_order, Some(retry_req)).await;
                    if let Err(ref retry_err) = retry_submit {
                        error!(market = %market, error = %retry_err, "retry order submit failed");
                    }
                    if let Ok(retry_rep) = retry_submit {
                        if let Ok(retry_res) = self
                            .parse_order_result(
                                retry_rep,
                                "Sell",
                                &retry_qty,
                                &clean_price,
                                market,
                                &retry_coid,
                            )
                            .await
                        {
                            res = retry_res;
                        }
                    }
                }
            }
        }

        if !res.success
            && res
                .error_msg
                .as_deref()
                .map_or(false, is_insufficient_margin_error)
        {
            if let Ok(val) = clean_qty.parse::<f64>() {
                for reduction in [0.90, 0.80] {
                    let reduced_val = val * reduction;
                    let (retry_qty, floored) = floor_qty_to_decimals(reduced_val, decimals);
                    if floored > 0.0 && retry_qty != clean_qty {
                        info!(market = %market, old_qty = %clean_qty, retry_qty = %retry_qty, reduction = %reduction, "Retrying user limit sell after InsufficientMargin");
                        let retry_coid = Uuid::new_v4().to_string();
                        if let Ok(retry_order) = SubmitOrderRequest::builder()
                            .market(market)
                            .account(account_id)
                            .sell()
                            .limit_order(&clean_price)
                            .quantity(&retry_qty)
                            .post_only(true)
                            .good_till_cancel()
                            .client_order_id(&retry_coid)
                            .build()
                        {
                            let retry_req = Uuid::new_v4().to_string();
                            let retry_submit =
                                client.submit_order(retry_order, Some(retry_req)).await;
                            if let Err(ref retry_err) = retry_submit {
                                error!(market = %market, error = %retry_err, "retry order submit failed");
                            }
                            if let Ok(retry_rep) = retry_submit {
                                if let Ok(retry_res) = self
                                    .parse_order_result(
                                        retry_rep,
                                        "Sell",
                                        &retry_qty,
                                        &clean_price,
                                        market,
                                        &retry_coid,
                                    )
                                    .await
                                {
                                    res = retry_res;
                                    if res.success {
                                        break;
                                    }
                                }
                            }
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
        let client = self.create_user_signing_client(decrypted_key)?;
        self.place_reduce_only_order(&client, account_id, market, side, quantity, price)
            .await
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

    async fn parse_order_result(
        &self,
        report: TransactionReport,
        side: &str,
        quantity: &str,
        price: &str,
        market: &str,
        client_order_id: &str,
    ) -> BotResult<OrderResult> {
        let (mut res, immediate, account_id) =
            interpret_submit_report(&report, side, quantity, price, market, client_order_id);

        // The matching engine can admit an order and only then reject it.
        // The submit receipt says success with zero fills; the indexer shows
        // Rejected / InsufficientMargin a few hundred milliseconds later.
        let known_engine_error = !res.success
            && res
                .error_msg
                .as_deref()
                .map(|err| !is_not_filled_message(err))
                .unwrap_or(false);
        if !known_engine_error {
            if let Some(account_id) = account_id.as_deref() {
                match self
                    .poll_indexer_order(account_id, client_order_id, immediate)
                    .await
                {
                    Some(snap) => {
                        let terminal = indexer_status_is_terminal(&snap.status);
                        apply_indexer_confirmation(&mut res, &snap, immediate);
                        // An IOC that is still working when the poll budget
                        // ends did not trade. Do not report that as a fill.
                        if immediate && !terminal && res.fills.is_empty() {
                            res.success = false;
                            res.error_msg =
                                Some(format!("Not filled (order still {})", snap.status));
                        }
                    }
                    None if immediate && res.fills.is_empty() => {
                        res.success = false;
                        res.error_msg =
                            Some("Not filled (order was not confirmed by the indexer)".to_string());
                    }
                    None => {}
                }
            } else if immediate && res.fills.is_empty() && res.success {
                res.success = false;
                res.error_msg =
                    Some("Not filled (order was not confirmed by the indexer)".to_string());
            }
        }

        log_order_outcome(&res);
        Ok(res)
    }

    /// Poll `GET /api/v1/accounts/{id}/orders?f[client_order_id]=` until the
    /// order is terminal, or until a resting order is still active after a
    /// short grace period. Interval 250ms, budget about 3s.
    async fn poll_indexer_order(
        &self,
        account_id: &str,
        client_order_id: &str,
        immediate: bool,
    ) -> Option<IndexerOrderSnap> {
        let mut account_ids = vec![account_id.to_string()];
        if let Some(resolved) = self.resolve_user_indexer_id(account_id).await {
            if !account_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&resolved))
            {
                account_ids.push(resolved);
            }
        }

        let started = tokio::time::Instant::now();
        let budget = Duration::from_millis(3000);
        let interval = Duration::from_millis(250);
        let resting_grace = Duration::from_millis(500);
        let mut last = None;

        loop {
            for id in &account_ids {
                if let Some(snap) = self.fetch_order_by_client_id(id, client_order_id).await {
                    if indexer_status_is_terminal(&snap.status) {
                        info!(
                            account_id = %id,
                            client_order_id,
                            status = %snap.status,
                            error = ?snap.error,
                            filled = %snap.filled_qty,
                            "indexer order reached a terminal status"
                        );
                        return Some(snap);
                    }
                    if !immediate && started.elapsed() >= resting_grace {
                        info!(
                            account_id = %id,
                            client_order_id,
                            status = %snap.status,
                            "indexer order is resting"
                        );
                        return Some(snap);
                    }
                    last = Some(snap);
                    break;
                }
            }
            if started.elapsed() >= budget {
                if let Some(ref snap) = last {
                    warn!(
                        client_order_id,
                        status = %snap.status,
                        "indexer order was still non-terminal when confirmation timed out"
                    );
                } else {
                    warn!(
                        client_order_id,
                        "indexer never returned the submitted order"
                    );
                }
                return last;
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn fetch_order_by_client_id(
        &self,
        account_id: &str,
        client_order_id: &str,
    ) -> Option<IndexerOrderSnap> {
        let url = format!(
            "{}/api/v1/accounts/{}/orders?f[client_order_id]={}",
            self.config.network.indexer_url(),
            account_id,
            client_order_id
        );
        let resp = self.http.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let val = resp.json::<serde_json::Value>().await.ok()?;
        indexer_order_from_json(&val, client_order_id)
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
                let ids: Vec<String> = orders.order_ids.iter().map(|id| id.to_string()).collect();
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

// Symmetrical trading parity: admin place_market_* and per-user place_user_market_*
// both go through place_marketable_for_account (size_marketable_qty + book price)
// and place_marketable_order (Error 114 / 121, InsufficientMargin 0.90 then 0.80
// with a fresh book price on each margin retry). Limit and close paths share
// parse_order_result, which confirms the indexer before reporting success.
// Admin and per-user closes share place_reduce_only_order.

#[cfg(test)]
mod tests {
    use super::*;

    fn bal(equity: f64, available: f64, orders: f64, positions: f64) -> BalanceInfo {
        BalanceInfo {
            total_equity: equity,
            available_margin: available,
            margin_allocated_orders: orders,
            margin_allocated_positions: positions,
        }
    }

    #[test]
    fn spendable_subtracts_order_margin_when_available_balance_still_includes_it() {
        // Equity 7.949, $3 locked in a resting order, indexer available left near equity.
        let info = bal(7.949, 7.56, 3.0, 0.0);
        let spendable = info.spendable_margin();
        assert!((spendable - 4.949).abs() < 1e-9);
        let (usd, is_max) = margin_budget_for_percentage(spendable, 100.0);
        assert!(is_max);
        assert!((usd - 4.44).abs() < 1e-9);
    }

    #[test]
    fn spendable_matches_available_when_indexer_already_netted_locks() {
        // Live indexer identity: equity = available + orders + positions.
        let info = bal(
            1217.204735660901,
            943.7277918306023,
            48.6136565,
            224.86328733029877,
        );
        let spendable = info.spendable_margin();
        assert!((spendable - info.available_margin).abs() < 1e-6);
    }

    #[test]
    fn spendable_falls_back_to_available_when_equity_is_missing() {
        let info = bal(0.0, 4.949, 0.0, 0.0);
        assert!((info.spendable_margin() - 4.949).abs() < 1e-9);
    }

    #[test]
    fn protective_price_fallback_is_a_tight_offset_inside_the_bound() {
        let mark = 81373.044;
        let (buy_s, buy) = protective_slippage_price(mark, true, 1, &[], 0.001, 0.01);
        let (sell_s, sell) = protective_slippage_price(mark, false, 1, &[], 0.001, 0.01);
        assert!(buy > mark, "long protective {buy_s} should be above {mark}");
        assert!(
            sell < mark,
            "short protective {sell_s} should be below {mark}"
        );
        assert!(buy < mark * 1.01);
        assert!(sell > mark * 0.99);
        // Empty book uses mark ± 0.3%, rounded to the tick.
        assert!((buy - mark * 1.003).abs() < 0.2, "buy {buy_s}");
        assert!((sell - mark * 0.997).abs() < 0.2, "sell {sell_s}");
    }

    #[test]
    fn protective_price_walks_the_book_then_caps_at_the_trading_bound() {
        let mark = 80000.0;
        let levels = [
            BookLevel {
                price: 80100.0,
                qty: 0.001,
            },
            BookLevel {
                price: 80200.0,
                qty: 0.01,
            },
        ];
        // 0.002 needs the second ask. 80200 * 1.0015 = 80320.3
        let (_, buy) = protective_slippage_price(mark, true, 1, &levels, 0.002, 0.01);
        assert!((buy - 80320.3).abs() < 0.05, "buy {buy}");

        let bids = [
            BookLevel {
                price: 79900.0,
                qty: 0.001,
            },
            BookLevel {
                price: 79800.0,
                qty: 0.01,
            },
        ];
        // 79800 * 0.9985 = 79680.3, floored to 1 decimal.
        let (_, sell) = protective_slippage_price(mark, false, 1, &bids, 0.002, 0.01);
        assert!((sell - 79680.3).abs() < 0.05, "sell {sell}");

        // A thin book that only quotes outside the bound is pulled back inside it.
        let far = [BookLevel {
            price: 90000.0,
            qty: 5.0,
        }];
        let (_, capped) = protective_slippage_price(mark, true, 1, &far, 1.0, 0.01);
        assert!(capped < mark * 1.01, "capped {capped}");
        assert!(capped > mark);
    }

    #[test]
    fn size_marketable_qty_includes_the_buy_premium_above_mark() {
        // The order that was logged success/fills=0 and then rejected
        // InsufficientMargin: 0.0033 BTC buy at 81433.4, mark 80707.06, 40x.
        let mark = 80707.06;
        let price = 81433.4;
        let budget = 6.80;
        let imr = 0.025;
        let fee = 0.00019;
        let (qty, q) = size_marketable_qty(budget, price, mark, true, imr, fee, 0.0001, 4);
        assert!(q > 0.0, "{qty}");
        assert!(
            q < 0.0033,
            "old size 0.0033 must not fit; helper returned {qty}"
        );
        let per = marketable_margin_per_unit(price, mark, true, imr, fee);
        assert!(q * per <= budget + 1e-6, "{qty} costs {}", q * per);
        assert!((q + 0.0001) * per > budget);
        // Required margin of the rejected 0.0033 exceeds the $7.56 that was free.
        let rejected = 0.0033 * per;
        assert!(rejected > 7.56, "rejected order margin {rejected}");
    }

    #[test]
    fn size_marketable_qty_mirrors_the_premium_for_sells() {
        let mark = 80707.06;
        let price = 79980.0;
        let budget = 6.80;
        let imr = 0.025;
        let fee = 0.00019;
        let (_, sell) = size_marketable_qty(budget, price, mark, false, imr, fee, 0.0001, 4);
        let per = marketable_margin_per_unit(price, mark, false, imr, fee);
        assert!(sell > 0.0);
        assert!(sell * per <= budget + 1e-6);
        assert!((sell + 0.0001) * per > budget);
        // A sell priced above the mark pays no extra slippage margin.
        let above = marketable_margin_per_unit(81000.0, mark, false, imr, fee);
        let no_slip = 81000.0 * imr + 81000.0 * fee;
        assert!((above - no_slip).abs() < 1e-9);
    }

    #[test]
    fn leverage_is_capped_at_the_inverse_of_imr() {
        assert!((capped_imr(0.025, 100.0) - 0.025).abs() < 1e-12);
        assert!((capped_imr(0.025, 40.0) - 0.025).abs() < 1e-12);
        assert!((capped_imr(0.025, 10.0) - 0.1).abs() < 1e-12);
    }

    #[test]
    fn parses_rejected_indexer_order_by_client_id() {
        let raw = r#"{"data":[{"id":"324556885173281310454344958","client_order_id":"abc","status":"Rejected","error":"InsufficientMargin","filled_quantity":"0","average_filled_price":"0"},{"id":"other","client_order_id":"nope","status":"Filled","filled_quantity":"1"}]}"#;
        let val: serde_json::Value = serde_json::from_str(raw).unwrap();
        let snap = indexer_order_from_json(&val, "abc").unwrap();
        assert_eq!(snap.status, "Rejected");
        assert_eq!(snap.error.as_deref(), Some("InsufficientMargin"));
        assert!(snap.filled_qty_f <= 0.0);
        assert!(indexer_order_from_json(&val, "missing").is_none());
    }

    #[test]
    fn indexer_rejection_clears_a_false_success() {
        let mut res = OrderResult {
            success: true,
            order_id: Some("1".into()),
            client_order_id: "cid".into(),
            side: "Buy (Market)".into(),
            quantity: "0.0033".into(),
            price: "81433.4".into(),
            market: "btc".into(),
            error_msg: None,
            fills: vec![],
        };
        let snap = IndexerOrderSnap {
            status: "Rejected".into(),
            error: Some("InsufficientMargin".into()),
            filled_qty: "0".into(),
            filled_qty_f: 0.0,
            avg_price: "0".into(),
            order_id: "324556885173281310454344958".into(),
        };
        apply_indexer_confirmation(&mut res, &snap, true);
        assert!(!res.success);
        assert_eq!(res.error_msg.as_deref(), Some("InsufficientMargin"));
        assert!(res.fills.is_empty());
        assert!(is_insufficient_margin_error(
            res.error_msg.as_deref().unwrap()
        ));
    }

    #[test]
    fn indexer_fill_is_copied_when_the_receipt_had_none() {
        let mut res = OrderResult {
            success: true,
            order_id: None,
            client_order_id: "cid".into(),
            side: "Buy (Market)".into(),
            quantity: "0.0024".into(),
            price: "80796.6".into(),
            market: "btc".into(),
            error_msg: None,
            fills: vec![],
        };
        let snap = IndexerOrderSnap {
            status: "Filled".into(),
            error: None,
            filled_qty: "0.0024".into(),
            filled_qty_f: 0.0024,
            avg_price: "80680.1".into(),
            order_id: "99".into(),
        };
        apply_indexer_confirmation(&mut res, &snap, true);
        assert!(res.success);
        assert_eq!(res.fills.len(), 1);
        assert_eq!(res.fills[0].quantity, "0.0024");
        assert_eq!(res.fills[0].price, "80680.1");
        assert_eq!(res.order_id.as_deref(), Some("99"));
    }

    #[test]
    fn ioc_cancel_with_no_fill_is_not_a_success() {
        let mut res = OrderResult {
            success: true,
            order_id: Some("7".into()),
            client_order_id: "cid".into(),
            side: "Sell (Market)".into(),
            quantity: "0.01".into(),
            price: "100".into(),
            market: "sol".into(),
            error_msg: None,
            fills: vec![],
        };
        let snap = IndexerOrderSnap {
            status: "Cancelled".into(),
            error: None,
            filled_qty: "0".into(),
            filled_qty_f: 0.0,
            avg_price: String::new(),
            order_id: "7".into(),
        };
        apply_indexer_confirmation(&mut res, &snap, true);
        assert!(!res.success);
        assert!(order_was_not_filled(res.error_msg.as_deref().unwrap()));
    }

    #[test]
    fn round_half_up_exceeds_position_floor_does_not() {
        // Documents why closes must not use round_qty_to_decimals.
        let (rounded, rounded_val) = round_qty_to_decimals(0.00325, 4);
        assert_eq!(rounded, "0.0033");
        assert!(rounded_val > 0.00325);
        let (floored, floored_val) = format_close_qty(0.00325, 100.0, 4).unwrap();
        assert_eq!(floored, "0.0032");
        assert!(floored_val <= 0.00325);
    }

    #[test]
    fn close_100_percent_floors_instead_of_rounding_past_the_position() {
        // round_qty_to_decimals(0.00325, 4) is 0.0033, which exceeds the position.
        let (qty, floored) = format_close_qty(0.00325, 100.0, 4).unwrap();
        assert_eq!(qty, "0.0032");
        assert!(floored <= 0.00325);
        assert!(floored > 0.0);

        // Exact lot is unchanged.
        let (qty, _) = format_close_qty(0.0032, 100.0, 4).unwrap();
        assert_eq!(qty, "0.0032");

        // Partial close also floors rather than rounding up through the position.
        let (qty, floored) = format_close_qty(0.0033, 50.0, 4).unwrap();
        assert!(floored <= 0.0033 * 0.5 + 1e-12);
        assert_eq!(qty, "0.0016");
    }

    #[test]
    fn step_down_qty_moves_one_lot() {
        let (qty, value) = step_down_qty("0.0033", 4).unwrap();
        assert_eq!(qty, "0.0032");
        assert!((value - 0.0032).abs() < 1e-12);
    }
}
