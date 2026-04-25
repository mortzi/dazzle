use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Utf8Bytes;

use crate::common::error::{AppError, AppResult};

#[derive(Debug, Serialize)]
pub struct Request {
    jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    pub params: serde_json::Value,
}

impl Request {
    pub fn new(id: u64, method: &'static str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }

    pub fn to_utf8bytes(&self) -> AppResult<Utf8Bytes> {
        let bytes = serde_json::to_vec(&self)
            .map_err(|e| AppError::InternalError(format!("Serialization failed: {}", e)))?;
        Utf8Bytes::try_from(bytes)
            .map_err(|_| AppError::InternalError("Invalid UTF-8 from JSON serializer".into()))
    }
}

#[derive(Debug, Serialize)]
pub struct RequestPayload {
    method: String,
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct Response<T> {
    id: u64,
    pub result: T,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Instrument {
    pub instrument_name: String,
    pub instrument_id: u64,
    pub instrument_type: String,
    pub kind: String,
    pub state: String,
    pub is_active: bool,
    pub base_currency: String,
    pub counter_currency: String,
    pub quote_currency: String,
    pub price_index: String,
    pub tick_size: f64,
    pub tick_size_steps: Vec<serde_json::Value>,
    pub contract_size: f64,
    pub min_trade_amount: f64,
    pub taker_commission: f64,
    pub maker_commission: f64,
    pub block_trade_commission: Option<f64>,
    pub block_trade_min_trade_amount: Option<f64>,
    pub block_trade_tick_size: Option<f64>,
    pub expiration_timestamp: u64,
    pub creation_timestamp: u64,
    pub settlement_period: Option<String>,   // absent on spot
    pub settlement_currency: Option<String>, // absent on spot
    pub future_type: Option<String>,         // only on futures
    pub max_leverage: Option<f64>,           // only on futures
    pub max_liquidation_commission: Option<f64>, // only on futures
    pub max_non_default_leverage: Option<f64>, // only on futures
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TickerStats {
    pub high: f64,
    pub low: f64,
    pub price_change: Option<f64>,
    pub volume: f64,
    pub volume_usd: f64,
    pub volume_notional: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Ticker {
    pub instrument_name: String,
    pub timestamp: u64,
    pub state: String,
    pub best_ask_price: f64,
    pub best_ask_amount: f64,
    pub best_bid_price: f64,
    pub best_bid_amount: f64,
    pub last_price: f64,
    pub mark_price: f64,
    pub index_price: f64,
    pub settlement_price: f64,
    pub open_interest: f64,
    pub min_price: f64,
    pub max_price: f64,
    pub estimated_delivery_price: f64,
    pub interest_value: f64,
    pub current_funding: f64,
    pub funding_8h: f64,
    pub stats: TickerStats,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderBook {
    pub instrument_name: String,
    pub timestamp: u64,
    pub state: String,
    pub change_id: u64,
    pub bids: Vec<[f64; 2]>, // [price, amount]
    pub asks: Vec<[f64; 2]>, // [price, amount]
    pub mark_price: f64,
    pub index_price: f64,
    pub last_price: f64,
    pub best_bid_price: f64,
    pub best_bid_amount: f64,
    pub best_ask_price: f64,
    pub best_ask_amount: f64,
    pub open_interest: f64,
    pub min_price: f64,
    pub max_price: f64,
    pub settlement_price: f64,
    pub current_funding: f64,
    pub funding_8h: f64,
}

// ! not yet checked !
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum BookUpdateType {
    Snapshot,
    Change,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BookLevel {
    pub action: String, // "new", "change", "delete"
    pub price: f64,
    pub amount: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderBookUpdate {
    pub instrument_name: String,
    pub timestamp: u64,
    pub change_id: u64,
    pub prev_change_id: Option<u64>, // None on snapshot
    #[serde(rename = "type")]
    pub update_type: BookUpdateType,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}
