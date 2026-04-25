use serde::{Serialize, Serializer};

use crate::common::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct ChannelName {
    pub instrument_name: String,
    pub kind: String,
    pub interval: String,
}

impl ChannelName {
    pub fn new(instrument_name: &str, kind: &str, interval: &str) -> Self {
        Self {
            instrument_name: instrument_name.to_string(),
            kind: kind.to_string(),
            interval: interval.to_string(),
        }
    }

    pub fn parse(s: &str) -> AppResult<Self> {
        let mut parts = s.splitn(3, '.');
        let kind = parts
            .next()
            .ok_or_else(|| AppError::InternalError("Missing kind in channel".into()))?;
        let instrument_name = parts
            .next()
            .ok_or_else(|| AppError::InternalError("Missing instrument in channel".into()))?;
        let interval = parts
            .next()
            .ok_or_else(|| AppError::InternalError("Missing interval in channel".into()))?;

        Ok(Self::new(instrument_name, kind, interval))
    }

    pub fn ticker(instrument_name: &str) -> Self {
        Self::new(instrument_name, "ticker", "100ms")
    }

    pub fn book(instrument_name: &str) -> Self {
        Self::new(instrument_name, "book", "100ms")
    }
}

impl std::fmt::Display for ChannelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}",
            self.kind, self.instrument_name, self.interval
        )
    }
}

impl From<ChannelName> for String {
    fn from(value: ChannelName) -> Self {
        value.to_string()
    }
}

impl TryFrom<&str> for ChannelName {
    type Error = AppError;

    fn try_from(value: &str) -> AppResult<Self> {
        ChannelName::parse(value)
    }
}

impl Serialize for ChannelName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
