use std::{
    fmt::Display,
    hash::{Hash, Hasher},
};

use serde::{Serialize, Serializer};

use crate::common::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct Channel {
    pub full: String,          // "book.BTC-PERPETUAL.100ms"
    kind_end: usize,       // index of first '.'
    instrument_end: usize, // index of second '.'
}

impl Channel {
    pub fn new(instrument: &str, kind: &str, interval: &str) -> Self {
        let full = format!("{}.{}.{}", kind, instrument, interval);
        let kind_end = kind.len();
        let instrument_end = kind_end + 1 + instrument.len();
        Self {
            full,
            kind_end,
            instrument_end,
        }
    }

    pub fn kind(&self) -> &str {
        &self.full[..self.kind_end]
    }

    pub fn instrument(&self) -> &str {
        &self.full[self.kind_end + 1..self.instrument_end]
    }

    pub fn interval(&self) -> &str {
        &self.full[self.instrument_end + 1..]
    }

    pub fn as_str(&self) -> &str {
        &self.full
    }

    pub fn parse(s: &str) -> AppResult<Self> {
        let mut parts = s.splitn(3, '.');

        let kind_end = parts
            .next()
            .ok_or_else(|| AppError::InternalError(format!("Invalid channel: {}", s)))?
            .len();
        let instrument_end = kind_end
            + 1
            + parts
                .next()
                .ok_or_else(|| AppError::InternalError(format!("Invalid channel: {}", s)))?
                .len();
        parts
            .next()
            .ok_or_else(|| AppError::InternalError(format!("Invalid channel: {}", s)))?;

        Ok(Self {
            full: s.into(),
            kind_end,
            instrument_end,
        })
    }

    pub fn ticker(instrument_name: &str) -> Self {
        Self::new(instrument_name, "ticker", "100ms")
    }

    pub fn book(instrument_name: &str) -> Self {
        Self::new(instrument_name, "book", "100ms")
    }
}

impl PartialEq for Channel {
    fn eq(&self, other: &Self) -> bool {
        self.full == other.full
    }
}

impl Eq for Channel {}

impl Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.full)
    }
}

impl Hash for Channel {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.full.hash(state)
    }
}

impl Serialize for Channel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_splits_correctly() {
        let ch = Channel::new("BTC-PERPETUAL", "book", "100ms");
        assert_eq!(ch.kind(), "book");
        assert_eq!(ch.instrument(), "BTC-PERPETUAL");
        assert_eq!(ch.interval(), "100ms");
        assert_eq!(ch.as_str(), "book.BTC-PERPETUAL.100ms");
    }

    #[test]
    fn ticker_factory() {
        let ch = Channel::ticker("ETH-PERPETUAL");
        assert_eq!(ch.kind(), "ticker");
        assert_eq!(ch.instrument(), "ETH-PERPETUAL");
        assert_eq!(ch.interval(), "100ms");
    }

    #[test]
    fn parse_valid() {
        let ch = Channel::parse("book.BTC-PERPETUAL.100ms").unwrap();
        assert_eq!(ch.kind(), "book");
        assert_eq!(ch.instrument(), "BTC-PERPETUAL");
        assert_eq!(ch.interval(), "100ms");
    }

    #[test]
    fn parse_missing_interval_errors() {
        assert!(Channel::parse("book.BTC-PERPETUAL").is_err());
    }

    #[test]
    fn parse_missing_instrument_errors() {
        assert!(Channel::parse("book").is_err());
    }

    #[test]
    fn equality_based_on_full_string() {
        let a = Channel::new("BTC-PERPETUAL", "book", "100ms");
        let b = Channel::parse("book.BTC-PERPETUAL.100ms").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn display() {
        assert_eq!(Channel::book("BTC-PERPETUAL").to_string(), "book.BTC-PERPETUAL.100ms");
    }
}
