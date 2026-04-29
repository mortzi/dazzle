use std::collections::BTreeMap;

use serde::{Serialize};

use crate::deribit::models::OrderBookUpdate;

// todo! fix the f64 to u64 conversion
pub(super) type PriceLevel = u64;
pub(super) type LevelAmount = u64;

#[derive(Clone, Debug, Serialize)]
pub struct Book {
    pub asks: BTreeMap<PriceLevel, LevelAmount>,
    pub bids: BTreeMap<PriceLevel, LevelAmount>,
    pub change_id: u64,
}

impl Book {
    pub fn new() -> Self {
        Self {
            asks: BTreeMap::new(),
            bids: BTreeMap::new(),
            change_id: 0,
        }
    }

    pub fn from_snapshot(snapshot: OrderBookUpdate) -> Self {
        let mut asks = BTreeMap::new();
        let mut bids = BTreeMap::new();

        for level in &snapshot.asks {
            asks.insert(level.price as u64, level.size as u64);
        }
        for level in &snapshot.bids {
            bids.insert(level.price as u64, level.size as u64);
        }

        Self {
            asks,
            bids,
            change_id: snapshot.change_id,
        }
    }
}
