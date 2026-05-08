use std::collections::BTreeMap;
use std::fmt;

use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::deribit::models::OrderBookUpdate;

/// Scale factor for fixed-point price representation: 1e8 (8 decimal places).
/// Matches Bitcoin's satoshi precision and covers all Deribit instruments:
/// USD-quoted perpetuals (e.g. 65432.50 → 6_543_250_000_000) and
/// BTC-quoted options (e.g. 0.00230000 → 230_000).
const PRICE_SCALE: u64 = 100_000_000;

/// Scale factor for fixed-point quantity representation: 1e8.
/// Deribit sizes can be fractional (e.g. 0.1 contracts for options).
const QUANTITY_SCALE: u64 = 100_000_000;

/// A price stored as a fixed-point integer (value × PRICE_SCALE).
/// Using integers avoids float comparison issues and non-deterministic
/// rounding — critical for order book key correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(u64);

impl Price {
    pub fn from_f64(value: f64) -> Self {
        Self((value * PRICE_SCALE as f64).round() as u64)
    }

    pub fn to_f64(self) -> f64 {
        self.0 as f64 / PRICE_SCALE as f64
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_f64())
    }
}

impl Serialize for Price {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.to_f64())
    }
}

/// A quantity stored as a fixed-point integer (value × QUANTITY_SCALE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quantity(u64);

impl Quantity {
    pub fn from_f64(value: f64) -> Self {
        Self((value * QUANTITY_SCALE as f64).round() as u64)
    }

    pub fn to_f64(self) -> f64 {
        self.0 as f64 / QUANTITY_SCALE as f64
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_f64())
    }
}

impl Serialize for Quantity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.to_f64())
    }
}

/// An L2 (Level 2) order book: aggregated price levels from the exchange.
/// Each price maps to the total quantity resting at that level.
/// Asks are stored in ascending price order; bids in ascending price order
/// (iterate in reverse to get highest-bid-first).
#[derive(Clone, Debug, Default)]
pub struct Book {
    pub asks: BTreeMap<Price, Quantity>,
    pub bids: BTreeMap<Price, Quantity>,
    pub change_id: u64,
}

impl Book {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: &OrderBookUpdate) -> Self {
        let asks = snapshot
            .asks
            .iter()
            .map(|l| (Price::from_f64(l.price), Quantity::from_f64(l.size)))
            .collect();

        let bids = snapshot
            .bids
            .iter()
            .map(|l| (Price::from_f64(l.price), Quantity::from_f64(l.size)))
            .collect();

        Self {
            asks,
            bids,
            change_id: snapshot.change_id,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Serialize)]
pub struct WalkResult {
    pub avg_price: Price,
    pub filled: Quantity,
    pub unfilled: Quantity,
}

impl Book {
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    pub fn spread(&self) -> Option<Price> {
        let ask = self.best_ask()?;
        let bid = self.best_bid()?;
        Some(Price(ask.0.saturating_sub(bid.0)))
    }

    pub fn mid_price(&self) -> Option<Price> {
        let ask = self.best_ask()?;
        let bid = self.best_bid()?;
        Some(Price(((ask.0 as u128 + bid.0 as u128) / 2) as u64))
    }

    /// Simulates filling a market order of `quantity` on the given `side`.
    /// Walks price levels in aggressive order (asks ascending for Buy, bids descending for Sell).
    /// Returns None if the book on that side is empty.
    pub fn walk_book(&self, side: Side, quantity: Quantity) -> Option<WalkResult> {
        let mut remaining = quantity.raw();
        let mut notional: u128 = 0;
        let mut filled: u64 = 0;

        let levels: Box<dyn Iterator<Item = (&Price, &Quantity)>> = match side {
            Side::Buy => Box::new(self.asks.iter()),
            Side::Sell => Box::new(self.bids.iter().rev()),
        };

        for (price, qty) in levels {
            if remaining == 0 {
                break;
            }
            let take = remaining.min(qty.raw());
            notional += price.raw() as u128 * take as u128;
            filled += take;
            remaining -= take;
        }

        if filled == 0 {
            return None;
        }

        Some(WalkResult {
            avg_price: Price((notional / filled as u128) as u64),
            filled: Quantity(filled),
            unfilled: Quantity(remaining),
        })
    }
}


/// Serializes the book as the standard order book format:
/// { asks: [[price, qty], ...], bids: [[price, qty], ...], change_id: N }
/// Asks are ascending (lowest ask first); bids are descending (highest bid first).
impl Serialize for Book {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let asks: Vec<[f64; 2]> = self
            .asks
            .iter()
            .map(|(p, q)| [p.to_f64(), q.to_f64()])
            .collect();

        let bids: Vec<[f64; 2]> = self
            .bids
            .iter()
            .rev()
            .map(|(p, q)| [p.to_f64(), q.to_f64()])
            .collect();

        let mut state = serializer.serialize_struct("Book", 3)?;
        state.serialize_field("asks", &asks)?;
        state.serialize_field("bids", &bids)?;
        state.serialize_field("change_id", &self.change_id)?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deribit::models::{BookLevel, BookUpdateType, OrderBookUpdate};

    fn make_book() -> Book {
        let mut book = Book::new();
        book.bids.insert(Price::from_f64(100.0), Quantity::from_f64(1.0));
        book.bids.insert(Price::from_f64(99.0), Quantity::from_f64(2.0));
        book.bids.insert(Price::from_f64(98.0), Quantity::from_f64(3.0));
        book.asks.insert(Price::from_f64(101.0), Quantity::from_f64(1.5));
        book.asks.insert(Price::from_f64(102.0), Quantity::from_f64(2.5));
        book.asks.insert(Price::from_f64(103.0), Quantity::from_f64(3.5));
        book
    }

    fn snapshot(asks: Vec<(f64, f64)>, bids: Vec<(f64, f64)>) -> OrderBookUpdate {
        OrderBookUpdate {
            instrument_name: "BTC-PERPETUAL".into(),
            timestamp: 0,
            change_id: 42,
            prev_change_id: None,
            update_type: BookUpdateType::Snapshot,
            asks: asks.into_iter().map(|(p, s)| BookLevel { action: "new".into(), price: p, size: s }).collect(),
            bids: bids.into_iter().map(|(p, s)| BookLevel { action: "new".into(), price: p, size: s }).collect(),
        }
    }

    #[test]
    fn price_roundtrip() {
        assert_eq!(Price::from_f64(80258.5).to_f64(), 80258.5);
        assert_eq!(Price::from_f64(0.0023).to_f64(), 0.0023);
    }

    #[test]
    fn price_distinct_fractional() {
        // fixed-point must distinguish these — old truncation bug would make them equal
        assert_ne!(Price::from_f64(80258.0), Price::from_f64(80258.5));
    }

    #[test]
    fn price_ordering() {
        assert!(Price::from_f64(100.0) < Price::from_f64(100.5));
    }

    #[test]
    fn from_snapshot_populates_book() {
        let book = Book::from_snapshot(&snapshot(vec![(101.0, 5.0)], vec![(99.0, 3.0)]));
        assert_eq!(book.change_id, 42);
        assert_eq!(book.best_ask(), Some(Price::from_f64(101.0)));
        assert_eq!(book.best_bid(), Some(Price::from_f64(99.0)));
    }

    #[test]
    fn analytics_empty_book() {
        let book = Book::new();
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.spread(), None);
        assert_eq!(book.mid_price(), None);
    }

    #[test]
    fn best_bid_ask() {
        let book = make_book();
        assert_eq!(book.best_bid(), Some(Price::from_f64(100.0)));
        assert_eq!(book.best_ask(), Some(Price::from_f64(101.0)));
    }

    #[test]
    fn spread() {
        let book = make_book();
        assert_eq!(book.spread(), Some(Price::from_f64(1.0)));
    }

    #[test]
    fn mid_price() {
        let book = make_book();
        assert_eq!(book.mid_price(), Some(Price::from_f64(100.5)));
    }

    #[test]
    fn walk_book_buy_single_level() {
        let book = make_book();
        let r = book.walk_book(Side::Buy, Quantity::from_f64(1.5)).unwrap();
        assert_eq!(r.avg_price, Price::from_f64(101.0));
        assert_eq!(r.filled, Quantity::from_f64(1.5));
        assert_eq!(r.unfilled, Quantity::from_f64(0.0));
    }

    #[test]
    fn walk_book_buy_multi_level_slippage() {
        let book = make_book();
        // 1.5@101 + 2.5@102 = 406.5 notional / 4.0 qty = 101.625 avg
        let r = book.walk_book(Side::Buy, Quantity::from_f64(4.0)).unwrap();
        assert_eq!(r.avg_price, Price::from_f64(101.625));
        assert_eq!(r.filled, Quantity::from_f64(4.0));
        assert_eq!(r.unfilled, Quantity::from_f64(0.0));
    }

    #[test]
    fn walk_book_buy_exceeds_depth() {
        let book = make_book();
        // total ask depth = 1.5 + 2.5 + 3.5 = 7.5
        let r = book.walk_book(Side::Buy, Quantity::from_f64(100.0)).unwrap();
        assert_eq!(r.filled, Quantity::from_f64(7.5));
        assert_eq!(r.unfilled, Quantity::from_f64(92.5));
    }

    #[test]
    fn walk_book_sell_single_level() {
        let book = make_book();
        let r = book.walk_book(Side::Sell, Quantity::from_f64(1.0)).unwrap();
        assert_eq!(r.avg_price, Price::from_f64(100.0));
        assert_eq!(r.filled, Quantity::from_f64(1.0));
        assert_eq!(r.unfilled, Quantity::from_f64(0.0));
    }

    #[test]
    fn walk_book_empty_returns_none() {
        assert!(Book::new().walk_book(Side::Buy, Quantity::from_f64(1.0)).is_none());
    }
}

